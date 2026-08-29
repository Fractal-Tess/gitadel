use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use globset::Glob;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use serde_json::json;
use serde_yml::{Mapping, Value};
use sley::GitObjectType;
use uuid::Uuid;

use crate::{
    entity::{action_job, action_job_need, action_run, repository, user},
    identity::ApiError,
    repository::read_git,
};

use super::ActionsState;

const WORKFLOW_DIRECTORIES: [&str; 3] = [
    ".forgejo/workflows",
    ".gitea/workflows",
    ".github/workflows",
];
const MAX_WORKFLOWS: usize = 64;
const MAX_WORKFLOW_BYTES: usize = 1_048_576;
const MAX_JOBS: usize = 128;
const MAX_EDGES: usize = 256;

#[derive(Debug)]
struct WorkflowPlan {
    path: String,
    name: String,
    root: Mapping,
    jobs: Vec<JobPlan>,
}

#[derive(Debug)]
struct JobPlan {
    key: String,
    name: String,
    labels: Vec<String>,
    needs: Vec<String>,
}

#[derive(Debug)]
struct WorkflowDiagnostic {
    path: String,
    kind: &'static str,
    summary: String,
}

pub(crate) async fn ingest_push(
    state: &ActionsState,
    repository: &repository::Model,
    actor_id: Uuid,
    reference: &str,
    before: &str,
    after: &str,
    zero: &str,
    changed_paths: Vec<String>,
) -> Result<(), ApiError> {
    if after == zero
        || (!reference.starts_with("refs/heads/") && !reference.starts_with("refs/tags/"))
    {
        return Ok(());
    }
    let plans = discover(state, repository, after.to_owned()).await?;
    let actor = user::Entity::find_by_id(actor_id)
        .one(state.repository().identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    for discovered in plans {
        match discovered {
            Ok(plan) => {
                if matches_push(&plan.root, reference, &changed_paths).unwrap_or(false) {
                    enqueue_plan(state, repository, &actor, reference, before, after, plan).await?;
                }
            }
            Err(diagnostic) => {
                record_failure(
                    state,
                    repository,
                    Some(actor_id),
                    reference,
                    before,
                    after,
                    diagnostic,
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn discover(
    state: &ActionsState,
    repository: &repository::Model,
    revision: String,
) -> Result<Vec<Result<WorkflowPlan, WorkflowDiagnostic>>, ApiError> {
    let path = state.repository().repository_path(repository);
    let files = read_git(path, move |git| {
        let mut selected = None;
        for directory in WORKFLOW_DIRECTORIES {
            if let Ok(resolved) = git.resolve_path(&revision, directory) {
                if resolved.object_type == GitObjectType::Tree {
                    selected = Some((directory, resolved.oid));
                    break;
                }
            }
        }
        let Some((directory, tree_oid)) = selected else {
            return Ok(Vec::new());
        };
        let tree = git.read_tree(&tree_oid)?;
        let mut files = Vec::new();
        for entry in tree.entries {
            if files.len() >= MAX_WORKFLOWS {
                break;
            }
            let Ok(name) = std::str::from_utf8(entry.name.as_bytes()) else {
                continue;
            };
            if entry.is_tree() || entry.is_symlink() || entry.is_gitlink() {
                continue;
            }
            if !(name.ends_with(".yml") || name.ends_with(".yaml")) {
                continue;
            }
            let bytes = git.blobs().read(entry.oid)?;
            files.push((format!("{directory}/{name}"), bytes));
        }
        Ok(files)
    })
    .await?;
    Ok(files
        .into_iter()
        .map(|(path, bytes)| validate_workflow(state, path, bytes))
        .collect())
}

fn validate_workflow(
    state: &ActionsState,
    path: String,
    bytes: Vec<u8>,
) -> Result<WorkflowPlan, WorkflowDiagnostic> {
    let fail = |kind, summary: String| WorkflowDiagnostic {
        path: path.clone(),
        kind,
        summary,
    };
    if bytes.len() > MAX_WORKFLOW_BYTES {
        return Err(fail(
            "workflow_invalid",
            "workflow exceeds the 1 MiB limit".to_owned(),
        ));
    }
    if bytes.contains(&0) {
        return Err(fail(
            "workflow_invalid",
            "workflow contains a NUL byte".to_owned(),
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| fail("workflow_invalid", "workflow is not UTF-8".to_owned()))?;
    if text.contains("\n<<:") || text.contains("\n  <<:") {
        return Err(fail(
            "workflow_unsupported",
            "YAML merge keys are not supported".to_owned(),
        ));
    }
    let value: Value =
        serde_yml::from_str(text).map_err(|error| fail("workflow_invalid", error.to_string()))?;
    let root = value.as_mapping().cloned().ok_or_else(|| {
        fail(
            "workflow_invalid",
            "workflow root must be a mapping".to_owned(),
        )
    })?;
    validate_trigger(&root).map_err(|summary| fail("workflow_unsupported", summary))?;
    let jobs_value = get(&root, "jobs")
        .and_then(Value::as_mapping)
        .ok_or_else(|| {
            fail(
                "workflow_invalid",
                "workflow jobs must be a mapping".to_owned(),
            )
        })?;
    if jobs_value.is_empty() || jobs_value.len() > MAX_JOBS {
        return Err(fail(
            "workflow_invalid",
            format!("workflow must contain 1 to {MAX_JOBS} jobs"),
        ));
    }
    let mut jobs = Vec::with_capacity(jobs_value.len());
    let mut keys = BTreeSet::new();
    let mut edge_count = 0;
    for (key, body) in jobs_value {
        let key = key
            .as_str()
            .ok_or_else(|| fail("workflow_invalid", "job IDs must be strings".to_owned()))?;
        if !valid_job_key(key) || !keys.insert(key.to_owned()) {
            return Err(fail(
                "workflow_invalid",
                format!("invalid or duplicate job ID `{key}`"),
            ));
        }
        let body = body
            .as_mapping()
            .ok_or_else(|| fail("workflow_invalid", format!("job `{key}` must be a mapping")))?;
        if get(body, "uses").is_some()
            || get(body, "strategy")
                .and_then(Value::as_mapping)
                .and_then(|map| get(map, "matrix"))
                .is_some()
        {
            return Err(fail(
                "workflow_unsupported",
                format!("job `{key}` uses a deferred reusable workflow or matrix"),
            ));
        }
        let labels = static_labels(get(body, "runs-on"))
            .map_err(|summary| fail("workflow_unsupported", format!("job `{key}` {summary}")))?;
        let needs = string_list(get(body, "needs"))
            .map_err(|summary| fail("workflow_invalid", format!("job `{key}` {summary}")))?;
        edge_count += needs.len();
        if edge_count > MAX_EDGES {
            return Err(fail(
                "workflow_invalid",
                format!("workflow exceeds {MAX_EDGES} dependency edges"),
            ));
        }
        validate_execution_sources(state, body)
            .map_err(|summary| fail("workflow_unsupported", format!("job `{key}` {summary}")))?;
        let name = get(body, "name")
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_owned();
        jobs.push(JobPlan {
            key: key.to_owned(),
            name,
            labels,
            needs,
        });
    }
    for job in &jobs {
        for needed in &job.needs {
            if !keys.contains(needed) {
                return Err(fail(
                    "workflow_invalid",
                    format!("job `{}` needs missing job `{needed}`", job.key),
                ));
            }
        }
    }
    detect_cycle(&jobs).map_err(|summary| fail("workflow_invalid", summary))?;
    let name = get(&root, "name")
        .and_then(Value::as_str)
        .unwrap_or(&path)
        .to_owned();
    Ok(WorkflowPlan {
        path,
        name,
        root,
        jobs,
    })
}

fn validate_trigger(root: &Mapping) -> Result<(), String> {
    let Some(trigger) = get(root, "on") else {
        return Err("workflow must declare an `on` trigger".to_owned());
    };
    match trigger {
        Value::String(value) if value == "push" => Ok(()),
        Value::Sequence(values) if values.iter().all(|value| value.as_str() == Some("push")) => {
            Ok(())
        }
        Value::Mapping(map) if map.keys().all(|key| key.as_str() == Some("push")) => {
            validate_push_filters(get(map, "push"))
        }
        _ => Err("only the push trigger is supported".to_owned()),
    }
}

fn validate_push_filters(push: Option<&Value>) -> Result<(), String> {
    let Some(push) = push else {
        return Ok(());
    };
    if push.is_null() {
        return Ok(());
    }
    let config = push
        .as_mapping()
        .ok_or_else(|| "push trigger configuration must be a mapping".to_owned())?;
    const FILTERS: [&str; 6] = [
        "branches",
        "branches-ignore",
        "tags",
        "tags-ignore",
        "paths",
        "paths-ignore",
    ];
    if config
        .keys()
        .any(|key| !key.as_str().is_some_and(|key| FILTERS.contains(&key)))
    {
        return Err("push trigger contains an unsupported filter".to_owned());
    }
    for (include, exclude) in [
        ("branches", "branches-ignore"),
        ("tags", "tags-ignore"),
        ("paths", "paths-ignore"),
    ] {
        let included = string_list(get(config, include))?;
        let excluded = string_list(get(config, exclude))?;
        if !included.is_empty() && !excluded.is_empty() {
            return Err(format!(
                "push trigger cannot combine `{include}` and `{exclude}`"
            ));
        }
        for pattern in included.iter().chain(&excluded) {
            Glob::new(pattern)
                .map_err(|error| format!("invalid push glob `{pattern}`: {error}"))?;
        }
    }
    Ok(())
}

fn matches_push(root: &Mapping, reference: &str, changed_paths: &[String]) -> Result<bool, String> {
    let trigger = get(root, "on").ok_or_else(|| "missing trigger".to_owned())?;
    let Value::Mapping(triggers) = trigger else {
        return Ok(true);
    };
    let Some(push) = get(triggers, "push") else {
        return Ok(false);
    };
    let Some(config) = push.as_mapping() else {
        return Ok(true);
    };
    let (kind, name) = if let Some(name) = reference.strip_prefix("refs/heads/") {
        ("branches", name)
    } else if let Some(name) = reference.strip_prefix("refs/tags/") {
        ("tags", name)
    } else {
        return Ok(false);
    };
    let other_kind = if kind == "branches" {
        "tags"
    } else {
        "branches"
    };
    if get(config, kind).is_none()
        && (get(config, other_kind).is_some()
            || get(config, &format!("{other_kind}-ignore")).is_some())
    {
        return Ok(false);
    }
    let included = string_list(get(config, kind))?;
    let excluded = string_list(get(config, &format!("{kind}-ignore")))?;
    if (!included.is_empty() && !glob_any(&included, name)) || glob_any(&excluded, name) {
        return Ok(false);
    }
    let paths = string_list(get(config, "paths"))?;
    let ignored = string_list(get(config, "paths-ignore"))?;
    if !paths.is_empty() && !changed_paths.iter().any(|path| glob_any(&paths, path)) {
        return Ok(false);
    }
    if paths.is_empty()
        && !ignored.is_empty()
        && changed_paths.iter().all(|path| glob_any(&ignored, path))
    {
        return Ok(false);
    }
    Ok(true)
}

fn validate_execution_sources(state: &ActionsState, job: &Mapping) -> Result<(), String> {
    if let Some(steps) = get(job, "steps") {
        let steps = steps
            .as_sequence()
            .ok_or_else(|| "`steps` must be a sequence".to_owned())?;
        if steps.len() > 256 {
            return Err("exceeds 256 steps".to_owned());
        }
        for step in steps {
            let step = step
                .as_mapping()
                .ok_or_else(|| "every step must be a mapping".to_owned())?;
            if let Some(uses) = get(step, "uses") {
                let uses = uses
                    .as_str()
                    .ok_or_else(|| "action references must be static strings".to_owned())?;
                validate_uses(state, uses)?;
            }
        }
    }
    if let Some(container) = get(job, "container") {
        validate_container(container)?;
    }
    if let Some(services) = get(job, "services") {
        let services = services
            .as_mapping()
            .ok_or_else(|| "`services` must be a mapping".to_owned())?;
        for service in services.values() {
            validate_container(service)?;
        }
    }
    Ok(())
}

fn glob_any(patterns: &[String], value: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| Glob::new(pattern).is_ok_and(|glob| glob.compile_matcher().is_match(value)))
}

fn static_labels(value: Option<&Value>) -> Result<Vec<String>, String> {
    let labels = string_list(value)?;
    if labels.is_empty() || labels.iter().any(|label| label.contains("${{")) {
        return Err("must use static nonempty `runs-on` labels".to_owned());
    }
    Ok(labels)
}

fn string_list(value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(value)) => Ok(vec![value.clone()]),
        Some(Value::Sequence(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "must contain only strings".to_owned())
            })
            .collect(),
        Some(_) => Err("must be a string or string list".to_owned()),
    }
}

fn validate_uses(state: &ActionsState, uses: &str) -> Result<(), String> {
    if uses.starts_with("./") && !uses.split('/').any(|part| part == "..") {
        return Ok(());
    }
    if let Some(image) = uses.strip_prefix("docker://") {
        return validate_image(image);
    }
    if uses.contains("${{") || uses.contains("://") {
        return Err("action references must be static repository references".to_owned());
    }
    let Some((repository, revision)) = uses.rsplit_once('@') else {
        return Err("remote actions must be pinned to a full commit".to_owned());
    };
    if repository.split('/').count() < 2
        || revision.len() != 40
        || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("remote actions must be pinned to a full 40-character commit".to_owned());
    }
    if !state
        .settings()
        .allowed_action_origins
        .iter()
        .any(|origin| origin == &state.settings().default_actions_origin)
    {
        return Err("remote actions are disabled by operator policy".to_owned());
    }
    Ok(())
}

fn validate_container(value: &Value) -> Result<(), String> {
    let image = value
        .as_str()
        .or_else(|| {
            value
                .as_mapping()
                .and_then(|map| get(map, "image"))
                .and_then(Value::as_str)
        })
        .ok_or_else(|| "container image must be a static string".to_owned())?;
    validate_image(image)
}

fn validate_image(image: &str) -> Result<(), String> {
    let Some((_, digest)) = image.rsplit_once("@sha256:") else {
        return Err("container images must be pinned by sha256 digest".to_owned());
    };
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("container image has an invalid sha256 digest".to_owned());
    }
    Ok(())
}

fn detect_cycle(jobs: &[JobPlan]) -> Result<(), String> {
    fn visit<'a>(
        key: &'a str,
        jobs: &'a [JobPlan],
        visiting: &mut BTreeSet<&'a str>,
        visited: &mut BTreeSet<&'a str>,
    ) -> bool {
        if visiting.contains(key) {
            return true;
        }
        if visited.contains(key) {
            return false;
        }
        visiting.insert(key);
        let cyclic = jobs.iter().find(|job| job.key == key).is_some_and(|job| {
            job.needs
                .iter()
                .any(|needed| visit(needed, jobs, visiting, visited))
        });
        visiting.remove(key);
        visited.insert(key);
        cyclic
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    if jobs
        .iter()
        .any(|job| visit(&job.key, jobs, &mut visiting, &mut visited))
    {
        Err("workflow dependency graph contains a cycle".to_owned())
    } else {
        Ok(())
    }
}

async fn enqueue_plan(
    state: &ActionsState,
    repository: &repository::Model,
    actor: &user::Model,
    reference: &str,
    before: &str,
    after: &str,
    plan: WorkflowPlan,
) -> Result<(), ApiError> {
    let database = state.repository().identity().database();
    let transaction = database.begin().await?;
    let number = action_run::Entity::find()
        .filter(action_run::Column::RepositoryId.eq(repository.id))
        .order_by_desc(action_run::Column::Number)
        .one(&transaction)
        .await?
        .map_or(1, |run| run.number + 1);
    let run_id = Uuid::new_v4();
    let now = Utc::now();
    action_run::ActiveModel {
        id: Set(run_id),
        repository_id: Set(repository.id),
        number: Set(number),
        workflow_path: Set(plan.path.clone()),
        workflow_name: Set(plan.name),
        event: Set("push".to_owned()),
        ref_name: Set(reference.to_owned()),
        before_sha: Set(before.to_owned()),
        after_sha: Set(after.to_owned()),
        actor_id: Set(Some(actor.id)),
        status: Set("queued".to_owned()),
        failure_kind: Set(None),
        failure_summary: Set(None),
        diagnostic: Set(None),
        event_json: Set(
            json!({"ref":reference,"before":before,"after":after,"actor":actor.username})
                .to_string(),
        ),
        cancel_requested_at: Set(None),
        cancelled_by: Set(None),
        created_at: Set(now),
        started_at: Set(None),
        completed_at: Set(None),
    }
    .insert(&transaction)
    .await?;
    let mut ids = BTreeMap::new();
    for job in &plan.jobs {
        let mut root = plan.root.clone();
        let all_jobs = get(&root, "jobs")
            .and_then(Value::as_mapping)
            .expect("validated jobs");
        let selected = all_jobs
            .get(Value::String(job.key.clone()))
            .expect("validated job")
            .clone();
        let mut one = Mapping::new();
        one.insert(Value::String(job.key.clone()), selected);
        root.insert(Value::String("jobs".to_owned()), Value::Mapping(one));
        let payload = serde_yml::to_string(&Value::Mapping(root))
            .map_err(ApiError::internal)?
            .into_bytes();
        let stored = action_job::ActiveModel {
            id: Default::default(),
            run_id: Set(run_id),
            job_key: Set(job.key.clone()),
            name: Set(job.name.clone()),
            required_labels: Set(serde_json::to_string(&job.labels).expect("labels serialize")),
            workflow_payload: Set(payload),
            status: Set(if job.needs.is_empty() {
                "queued"
            } else {
                "waiting"
            }
            .to_owned()),
            result: Set(None),
            runner_id: Set(None),
            request_key: Set(None),
            lease_generation: Set(0),
            lease_deadline: Set(None),
            last_report_at: Set(None),
            attempt: Set(0),
            step_state: Set("[]".to_owned()),
            outputs: Set("{}".to_owned()),
            expected_log_index: Set(0),
            log_bytes: Set(0),
            log_truncated: Set(false),
            failure_kind: Set(None),
            failure_summary: Set(None),
            created_at: Set(now),
            started_at: Set(None),
            completed_at: Set(None),
        }
        .insert(&transaction)
        .await?;
        ids.insert(job.key.clone(), stored.id);
    }
    for job in &plan.jobs {
        for needed in &job.needs {
            action_job_need::ActiveModel {
                job_id: Set(ids[&job.key]),
                needed_job_id: Set(ids[needed]),
                needed_job_key: Set(needed.clone()),
            }
            .insert(&transaction)
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}

async fn record_failure(
    state: &ActionsState,
    repository: &repository::Model,
    actor_id: Option<Uuid>,
    reference: &str,
    before: &str,
    after: &str,
    diagnostic: WorkflowDiagnostic,
) -> Result<(), ApiError> {
    let database = state.repository().identity().database();
    let number = action_run::Entity::find()
        .filter(action_run::Column::RepositoryId.eq(repository.id))
        .order_by_desc(action_run::Column::Number)
        .one(database)
        .await?
        .map_or(1, |run| run.number + 1);
    let now = Utc::now();
    action_run::ActiveModel {
        id: Set(Uuid::new_v4()),
        repository_id: Set(repository.id),
        number: Set(number),
        workflow_path: Set(diagnostic.path.clone()),
        workflow_name: Set(diagnostic.path),
        event: Set("push".to_owned()),
        ref_name: Set(reference.to_owned()),
        before_sha: Set(before.to_owned()),
        after_sha: Set(after.to_owned()),
        actor_id: Set(actor_id),
        status: Set("failure".to_owned()),
        failure_kind: Set(Some(diagnostic.kind.to_owned())),
        failure_summary: Set(Some(diagnostic.summary.clone())),
        diagnostic: Set(Some(diagnostic.summary)),
        event_json: Set("{}".to_owned()),
        cancel_requested_at: Set(None),
        cancelled_by: Set(None),
        created_at: Set(now),
        started_at: Set(None),
        completed_at: Set(Some(now)),
    }
    .insert(database)
    .await?;
    Ok(())
}

fn get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_owned()))
}

fn valid_job_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 100
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_cycles_are_rejected() {
        let jobs = vec![
            JobPlan {
                key: "a".to_owned(),
                name: "a".to_owned(),
                labels: vec!["docker".to_owned()],
                needs: vec!["b".to_owned()],
            },
            JobPlan {
                key: "b".to_owned(),
                name: "b".to_owned(),
                labels: vec!["docker".to_owned()],
                needs: vec!["a".to_owned()],
            },
        ];
        assert!(detect_cycle(&jobs).is_err());
    }

    #[test]
    fn branch_and_path_filters_match() {
        let root: Value = serde_yml::from_str(
            "on:\n  push:\n    branches: [main]\n    paths: [src/**]\njobs: {}\n",
        )
        .unwrap();
        assert!(
            matches_push(
                root.as_mapping().unwrap(),
                "refs/heads/main",
                &["src/lib.rs".to_owned()]
            )
            .unwrap()
        );
    }

    #[test]
    fn branch_and_tag_filters_do_not_cross_trigger() {
        let tags: Value =
            serde_yml::from_str("on:\n  push:\n    tags: ['v*']\njobs: {}\n").unwrap();
        assert!(!matches_push(tags.as_mapping().unwrap(), "refs/heads/main", &[]).unwrap());
        assert!(matches_push(tags.as_mapping().unwrap(), "refs/tags/v1.0.0", &[]).unwrap());

        let branches: Value =
            serde_yml::from_str("on:\n  push:\n    branches: [main]\njobs: {}\n").unwrap();
        assert!(!matches_push(branches.as_mapping().unwrap(), "refs/tags/v1.0.0", &[]).unwrap());
    }

    #[test]
    fn invalid_or_ambiguous_push_filters_are_rejected() {
        for source in [
            "on:\n  push:\n    branches: ['[']\n",
            "on:\n  push:\n    paths: [src/**]\n    paths-ignore: [docs/**]\n",
            "on:\n  push:\n    unsupported: true\n",
        ] {
            let root: Value = serde_yml::from_str(source).unwrap();
            assert!(validate_trigger(root.as_mapping().unwrap()).is_err());
        }
    }
}
