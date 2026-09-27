use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use globset::Glob;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use serde_json::json;
use sley::GitObjectType;
use uuid::Uuid;
use yaml_serde::{Mapping, Value};

use crate::{
    config::ActionsSettings,
    entity::{action_job, action_job_need, action_run, repository, user},
    identity::ApiError,
    repository::read_git,
};

use super::{
    ActionsState,
    matrix::{self, StoredMatrix},
};

const WORKFLOW_DIRECTORIES: [&str; 3] = [
    ".forgejo/workflows",
    ".gitea/workflows",
    ".github/workflows",
];
const MAX_WORKFLOWS: usize = 64;
const MAX_WORKFLOW_BYTES: usize = 1_048_576;
const MAX_JOBS: usize = 128;
const MAX_EDGES: usize = 256;
/// Limits after matrix expansion.
const MAX_EXPANDED_JOBS: usize = 256;
const MAX_EXPANDED_EDGES: usize = 4_096;

#[derive(Debug)]
pub(crate) struct WorkflowPlan {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) root: Mapping,
    jobs: Vec<JobPlan>,
}

/// One schedulable job. Matrix jobs expand to one plan per combination,
/// all sharing `base`, the job ID from the workflow file.
#[derive(Debug)]
struct JobPlan {
    /// Unique within the run: `base`, or `base:N` for matrix expansions.
    key: String,
    base: String,
    name: String,
    labels: Vec<String>,
    /// Workflow job IDs this job needs; every expansion of each must finish.
    needs: Vec<String>,
    /// The job body sent to the runner, with its matrix pinned.
    body: Mapping,
    matrix: Option<StoredMatrix>,
}

#[derive(Debug)]
pub(crate) struct WorkflowDiagnostic {
    pub(crate) path: String,
    pub(crate) kind: &'static str,
    pub(crate) summary: String,
}

/// What caused a run; stored on the run and exposed as the `github` context.
pub(crate) struct RunTrigger<'a> {
    pub(crate) event: &'static str,
    pub(crate) reference: &'a str,
    pub(crate) before: &'a str,
    pub(crate) after: &'a str,
    pub(crate) actor: Option<&'a user::Model>,
    pub(crate) event_json: serde_json::Value,
}

impl WorkflowPlan {
    /// Whether the workflow's `on` declares `event`.
    pub(crate) fn has_trigger(&self, event: &str) -> bool {
        has_trigger(&self.root, event)
    }

    /// The `on.<event>` configuration, when `on` is a mapping.
    pub(crate) fn trigger_config(&self, event: &str) -> Option<&Value> {
        get(&self.root, "on")
            .and_then(Value::as_mapping)
            .and_then(|triggers| get(triggers, event))
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "push fields mirror the external Git event contract"
)]
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
    let after = workflow_commit_oid(state, repository, reference, after).await?;
    let plans = discover(state, repository, after.clone()).await?;
    let actor = user::Entity::find_by_id(actor_id)
        .one(state.repository().identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    for discovered in plans {
        match discovered {
            Ok(plan) => {
                if matches_push(&plan.root, reference, &changed_paths).unwrap_or(false) {
                    let trigger = RunTrigger {
                        event: "push",
                        reference,
                        before,
                        after: &after,
                        actor: Some(&actor),
                        event_json: json!({
                            "ref": reference,
                            "before": before,
                            "after": after,
                            "actor": actor.username,
                        }),
                    };
                    enqueue_plan(
                        state.repository().identity().database(),
                        repository,
                        &trigger,
                        plan,
                    )
                    .await?;
                }
            }
            Err(diagnostic) => {
                record_failure(
                    state,
                    repository,
                    Some(actor_id),
                    reference,
                    before,
                    &after,
                    diagnostic,
                )
                .await?;
            }
        }
    }
    if repository
        .default_branch
        .as_deref()
        .is_some_and(|branch| reference.strip_prefix("refs/heads/") == Some(branch))
        && let Err(error) = super::schedule::sync_repository(state, repository).await
    {
        tracing::warn!(%error, repository_id = %repository.id, "could not refresh Actions schedules");
    }
    Ok(())
}

async fn workflow_commit_oid(
    state: &ActionsState,
    repository: &repository::Model,
    reference: &str,
    after: &str,
) -> Result<String, ApiError> {
    if !reference.starts_with("refs/tags/") {
        return Ok(after.to_owned());
    }
    let path = state.repository().repository_path(repository);
    let after = after.to_owned();
    read_git(path, move |git| {
        let oid = git.peel_to_commit_oid(git.rev_parse(&after)?)?;
        Ok(oid.to_hex())
    })
    .await
}

/// Reads and validates every workflow file at `revision`.
pub(crate) async fn discover(
    state: &ActionsState,
    repository: &repository::Model,
    revision: String,
) -> Result<Vec<Result<WorkflowPlan, WorkflowDiagnostic>>, ApiError> {
    let path = state.repository().repository_path(repository);
    let files = read_git(path, move |git| {
        let mut selected = None;
        for directory in WORKFLOW_DIRECTORIES {
            if let Ok(resolved) = git.resolve_path(&revision, directory)
                && resolved.object_type == GitObjectType::Tree
            {
                selected = Some((directory, resolved.oid));
                break;
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
        .map(|(path, bytes)| validate_workflow(state.settings(), path, bytes))
        .collect())
}

fn validate_workflow(
    settings: &ActionsSettings,
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
        yaml_serde::from_str(text).map_err(|error| fail("workflow_invalid", error.to_string()))?;
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
    let mut graph = BTreeMap::new();
    let mut keys = BTreeSet::new();
    let mut edge_count = 0;
    for (key, body) in jobs_value {
        let Some(key) = key.as_str() else {
            return Err(fail(
                "workflow_invalid",
                "workflow job IDs must be strings".to_owned(),
            ));
        };
        if !valid_job_key(key) || !keys.insert(key.to_owned()) {
            return Err(fail(
                "workflow_invalid",
                format!("invalid or duplicate job ID `{key}`"),
            ));
        }
        let body = body
            .as_mapping()
            .ok_or_else(|| fail("workflow_invalid", format!("job `{key}` must be a mapping")))?;
        if get(body, "uses").is_some() {
            return Err(fail(
                "workflow_unsupported",
                format!("job `{key}` uses a deferred reusable workflow"),
            ));
        }
        let strategy = matrix::parse_strategy(body)
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
        validate_execution_sources(settings, body)
            .map_err(|summary| fail("workflow_unsupported", format!("job `{key}` {summary}")))?;
        let explicit_name = get(body, "name").and_then(Value::as_str);
        let name = explicit_name.unwrap_or(key).to_owned();
        graph.insert(key.to_owned(), needs.clone());
        let unsupported =
            |summary: String| fail("workflow_unsupported", format!("job `{key}` {summary}"));
        match strategy {
            None => {
                let labels = static_labels(get(body, "runs-on")).map_err(unsupported)?;
                jobs.push(JobPlan {
                    key: key.to_owned(),
                    base: key.to_owned(),
                    name,
                    labels,
                    needs,
                    body: body.clone(),
                    matrix: None,
                });
            }
            Some(strategy) => {
                let original = get(body, "strategy")
                    .and_then(Value::as_mapping)
                    .cloned()
                    .unwrap_or_default();
                for (index, combination) in strategy.combinations.iter().enumerate() {
                    let labels = string_list(get(body, "runs-on"))
                        .map_err(&unsupported)?
                        .iter()
                        .map(|label| matrix::substitute(label, combination))
                        .collect::<Option<Vec<_>>>()
                        .filter(|labels| {
                            !labels.is_empty() && labels.iter().all(|label| !label.is_empty())
                        })
                        .ok_or_else(|| {
                            unsupported(
                                "must use static `runs-on` labels or matrix values".to_owned(),
                            )
                        })?;
                    let expanded_name =
                        matrix::job_name(&name, explicit_name.is_some(), combination);
                    let mut expanded = body.clone();
                    expanded.insert(
                        Value::String("strategy".to_owned()),
                        Value::Mapping(matrix::pinned_strategy(&original, combination)),
                    );
                    expanded.insert(
                        Value::String("name".to_owned()),
                        Value::String(expanded_name.clone()),
                    );
                    jobs.push(JobPlan {
                        key: format!("{key}:{index}"),
                        base: key.to_owned(),
                        name: expanded_name,
                        labels,
                        needs: needs.clone(),
                        body: expanded,
                        matrix: Some(matrix::stored(&strategy, combination)),
                    });
                }
            }
        }
    }
    for (key, needs) in &graph {
        for needed in needs {
            if !keys.contains(needed) {
                return Err(fail(
                    "workflow_invalid",
                    format!("job `{key}` needs missing job `{needed}`"),
                ));
            }
        }
    }
    detect_cycle(&graph).map_err(|summary| fail("workflow_invalid", summary))?;
    if jobs.len() > MAX_EXPANDED_JOBS {
        return Err(fail(
            "workflow_invalid",
            format!("workflow expands to more than {MAX_EXPANDED_JOBS} jobs"),
        ));
    }
    let expanded_edges: usize = jobs
        .iter()
        .flat_map(|job| &job.needs)
        .map(|needed| jobs.iter().filter(|job| &job.base == needed).count())
        .sum();
    if expanded_edges > MAX_EXPANDED_EDGES {
        return Err(fail(
            "workflow_invalid",
            format!("workflow expands to more than {MAX_EXPANDED_EDGES} dependency edges"),
        ));
    }
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
    const SIMPLE: [&str; 2] = ["push", "workflow_dispatch"];
    const CONFIGURABLE: [&str; 3] = ["push", "workflow_dispatch", "schedule"];
    match trigger {
        Value::String(value) if SIMPLE.contains(&value.as_str()) => Ok(()),
        Value::Sequence(values)
            if values
                .iter()
                .all(|value| value.as_str().is_some_and(|value| SIMPLE.contains(&value))) =>
        {
            Ok(())
        }
        Value::Mapping(map)
            if map
                .keys()
                .all(|key| key.as_str().is_some_and(|key| CONFIGURABLE.contains(&key))) =>
        {
            validate_push_filters(get(map, "push"))?;
            if let Some(dispatch) = get(map, "workflow_dispatch") {
                super::dispatch::parse_inputs(Some(dispatch))?;
            }
            if let Some(schedule) = get(map, "schedule") {
                super::schedule::parse_crons(schedule)?;
            }
            Ok(())
        }
        _ => {
            Err("only the push, workflow_dispatch, and schedule triggers are supported".to_owned())
        }
    }
}

fn has_trigger(root: &Mapping, event: &str) -> bool {
    match get(root, "on") {
        Some(Value::String(value)) => value == event,
        Some(Value::Sequence(values)) => values.iter().any(|value| value.as_str() == Some(event)),
        Some(Value::Mapping(map)) => map.contains_key(event),
        _ => false,
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
        .any(|key| key.as_str().is_none_or(|key| !FILTERS.contains(&key)))
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
    if !has_trigger(root, "push") {
        return Ok(false);
    }
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

fn validate_execution_sources(settings: &ActionsSettings, job: &Mapping) -> Result<(), String> {
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
                validate_uses(settings, uses)?;
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

fn validate_uses(settings: &ActionsSettings, uses: &str) -> Result<(), String> {
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
    if !settings
        .allowed_action_origins
        .iter()
        .any(|origin| origin == &settings.default_actions_origin)
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

fn detect_cycle(graph: &BTreeMap<String, Vec<String>>) -> Result<(), String> {
    fn visit<'a>(
        key: &'a str,
        graph: &'a BTreeMap<String, Vec<String>>,
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
        let cyclic = graph.get(key).is_some_and(|needs| {
            needs
                .iter()
                .any(|needed| visit(needed, graph, visiting, visited))
        });
        visiting.remove(key);
        visited.insert(key);
        cyclic
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    if graph
        .keys()
        .any(|key| visit(key, graph, &mut visiting, &mut visited))
    {
        Err("workflow dependency graph contains a cycle".to_owned())
    } else {
        Ok(())
    }
}

pub(crate) async fn enqueue_plan(
    database: &DatabaseConnection,
    repository: &repository::Model,
    trigger: &RunTrigger<'_>,
    plan: WorkflowPlan,
) -> Result<action_run::Model, ApiError> {
    let transaction = database.begin().await?;
    let number = action_run::Entity::find()
        .filter(action_run::Column::RepositoryId.eq(repository.id))
        .order_by_desc(action_run::Column::Number)
        .one(&transaction)
        .await?
        .map_or(1, |run| run.number + 1);
    let run_id = Uuid::new_v4();
    let now = Utc::now();
    let run = action_run::ActiveModel {
        id: Set(run_id),
        repository_id: Set(repository.id),
        number: Set(number),
        workflow_path: Set(plan.path.clone()),
        workflow_name: Set(plan.name),
        event: Set(trigger.event.to_owned()),
        ref_name: Set(trigger.reference.to_owned()),
        before_sha: Set(trigger.before.to_owned()),
        after_sha: Set(trigger.after.to_owned()),
        actor_id: Set(trigger.actor.map(|actor| actor.id)),
        status: Set("queued".to_owned()),
        failure_kind: Set(None),
        failure_summary: Set(None),
        diagnostic: Set(None),
        event_json: Set(trigger.event_json.to_string()),
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
        let mut one = Mapping::new();
        one.insert(
            Value::String(job.base.clone()),
            Value::Mapping(job.body.clone()),
        );
        root.insert(Value::String("jobs".to_owned()), Value::Mapping(one));
        let payload = yaml_serde::to_string(&Value::Mapping(root))
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
            matrix_json: Set(job
                .matrix
                .as_ref()
                .map(|matrix| serde_json::to_string(matrix).expect("matrix serializes"))),
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
            for dependency in plan.jobs.iter().filter(|other| &other.base == needed) {
                action_job_need::ActiveModel {
                    job_id: Set(ids[&job.key]),
                    needed_job_id: Set(ids[&dependency.key]),
                    needed_job_key: Set(needed.clone()),
                }
                .insert(&transaction)
                .await?;
            }
        }
    }
    transaction.commit().await?;
    Ok(run)
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
    mapping.get(key)
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
        let graph = BTreeMap::from([
            ("a".to_owned(), vec!["b".to_owned()]),
            ("b".to_owned(), vec!["a".to_owned()]),
        ]);
        assert!(detect_cycle(&graph).is_err());
        let graph = BTreeMap::from([
            ("a".to_owned(), vec![]),
            ("b".to_owned(), vec!["a".to_owned()]),
        ]);
        assert!(detect_cycle(&graph).is_ok());
    }

    #[test]
    fn branch_and_path_filters_match() {
        let root: Value = yaml_serde::from_str(
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
            yaml_serde::from_str("on:\n  push:\n    tags: ['v*']\njobs: {}\n").unwrap();
        assert!(!matches_push(tags.as_mapping().unwrap(), "refs/heads/main", &[]).unwrap());
        assert!(matches_push(tags.as_mapping().unwrap(), "refs/tags/v1.0.0", &[]).unwrap());

        let branches: Value =
            yaml_serde::from_str("on:\n  push:\n    branches: [main]\njobs: {}\n").unwrap();
        assert!(!matches_push(branches.as_mapping().unwrap(), "refs/tags/v1.0.0", &[]).unwrap());
    }

    #[test]
    fn invalid_or_ambiguous_push_filters_are_rejected() {
        for source in [
            "on:\n  push:\n    branches: ['[']\n",
            "on:\n  push:\n    paths: [src/**]\n    paths-ignore: [docs/**]\n",
            "on:\n  push:\n    unsupported: true\n",
        ] {
            let root: Value = yaml_serde::from_str(source).unwrap();
            assert!(validate_trigger(root.as_mapping().unwrap()).is_err());
        }
    }

    #[test]
    fn dispatch_and_schedule_triggers_are_accepted_but_do_not_match_pushes() {
        for source in [
            "on: workflow_dispatch\n",
            "on: [push, workflow_dispatch]\n",
            "on:\n  workflow_dispatch:\n    inputs:\n      name:\n        default: x\n  schedule:\n    - cron: '0 3 * * 1'\n",
        ] {
            let root: Value = yaml_serde::from_str(source).unwrap();
            validate_trigger(root.as_mapping().unwrap()).unwrap();
        }
        for (source, pushes) in [
            ("on: workflow_dispatch\n", false),
            ("on: [push, workflow_dispatch]\n", true),
            ("on:\n  schedule:\n    - cron: '0 3 * * *'\n", false),
            ("on:\n  push:\n  schedule:\n    - cron: '0 3 * * *'\n", true),
        ] {
            let root: Value = yaml_serde::from_str(source).unwrap();
            assert_eq!(
                matches_push(root.as_mapping().unwrap(), "refs/heads/main", &[]).unwrap(),
                pushes,
                "{source}"
            );
        }
        for source in [
            "on: schedule\n",
            "on: pull_request\n",
            "on:\n  schedule:\n    - cron: 'every day'\n",
            "on:\n  workflow_dispatch:\n    inputs:\n      x:\n        type: environment\n",
        ] {
            let root: Value = yaml_serde::from_str(source).unwrap();
            assert!(
                validate_trigger(root.as_mapping().unwrap()).is_err(),
                "{source}"
            );
        }
    }

    const MATRIX_WORKFLOW: &str = "name: CI\non: push\njobs:\n  build:\n    runs-on: ${{ matrix.os }}\n    strategy:\n      max-parallel: 1\n      matrix:\n        os: [docker, podman]\n        node: [18, 20]\n        exclude:\n          - os: podman\n            node: 18\n    steps:\n      - run: echo ${{ matrix.node }}\n  report:\n    needs: build\n    runs-on: docker\n    steps:\n      - run: echo done\n";

    fn plan(source: &str) -> WorkflowPlan {
        validate_workflow(
            &ActionsSettings::default(),
            ".forgejo/workflows/ci.yml".to_owned(),
            source.as_bytes().to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn matrix_jobs_expand_with_names_labels_and_pinned_payloads() {
        let plan = plan(MATRIX_WORKFLOW);
        let names: Vec<_> = plan.jobs.iter().map(|job| job.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "build (docker, 18)",
                "build (docker, 20)",
                "build (podman, 20)",
                "report"
            ]
        );
        assert_eq!(plan.jobs[2].key, "build:2");
        assert_eq!(plan.jobs[2].labels, ["podman"]);
        assert_eq!(plan.jobs[3].needs, ["build"]);
        let pinned = plan.jobs[1]
            .body
            .get("strategy")
            .and_then(Value::as_mapping)
            .and_then(|strategy| strategy.get("matrix"))
            .and_then(Value::as_mapping)
            .unwrap();
        assert_eq!(
            yaml_serde::to_string(&Value::Mapping(pinned.clone())).unwrap(),
            "os:\n- docker\nnode:\n- 20\n"
        );
        let stored = plan.jobs[0].matrix.as_ref().unwrap();
        assert!(stored.fail_fast);
        assert_eq!(stored.max_parallel, Some(1));
    }

    #[test]
    fn dynamic_labels_outside_a_matrix_are_rejected() {
        let error = validate_workflow(
            &ActionsSettings::default(),
            "ci.yml".to_owned(),
            b"on: push\njobs:\n  a:\n    runs-on: ${{ matrix.os }}\n    steps: []\n".to_vec(),
        )
        .unwrap_err();
        assert_eq!(error.kind, "workflow_unsupported");
    }

    #[tokio::test]
    async fn matrix_needs_wait_for_every_expansion_and_fail_fast_cancels_siblings() {
        use crate::actions::{runs, test_support};
        use sea_orm::ConnectionTrait as _;

        let database = test_support::database().await;
        let owner = test_support::owner(&database, "alice").await;
        let repository = test_support::repository(&database, &owner, "project").await;
        let before = "0".repeat(40);
        let after = "1".repeat(40);
        let trigger = RunTrigger {
            event: "push",
            reference: "refs/heads/main",
            before: &before,
            after: &after,
            actor: Some(&owner),
            event_json: json!({}),
        };
        enqueue_plan(&database, &repository, &trigger, plan(MATRIX_WORKFLOW))
            .await
            .unwrap();
        let jobs = action_job::Entity::find()
            .order_by_asc(action_job::Column::Id)
            .all(&database)
            .await
            .unwrap();
        assert_eq!(jobs.len(), 4);
        let report = &jobs[3];
        assert_eq!(report.status, "waiting");
        let edges = action_job_need::Entity::find()
            .filter(action_job_need::Column::JobId.eq(report.id))
            .all(&database)
            .await
            .unwrap();
        assert_eq!(edges.len(), 3);
        assert!(edges.iter().all(|edge| edge.needed_job_key == "build"));

        // Pretend a runner leased the first expansion, then fail it.
        let mut leased: action_job::ActiveModel = jobs[0].clone().into();
        leased.status = Set("running".to_owned());
        leased.runner_id = Set(None);
        let first = leased.update(&database).await.unwrap();
        database
            .execute_unprepared(&format!(
                "INSERT INTO action_runners (uuid, namespace, name, token_hash, approved_labels, version, created_at) VALUES ('r', 'alice', 'r', 'h', '[]', '13.0.0', '2026-01-01T00:00:00Z'); UPDATE action_jobs SET runner_id = last_insert_rowid() WHERE id = {}",
                first.id
            ))
            .await
            .unwrap();
        let runner_id = action_job::Entity::find_by_id(first.id)
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .runner_id
            .unwrap();
        runs::update_task(
            &database,
            runner_id,
            first.id,
            runs::TaskResult::Failure,
            BTreeMap::new(),
            "[]".to_owned(),
            300,
        )
        .await
        .unwrap();

        let jobs = action_job::Entity::find()
            .order_by_asc(action_job::Column::Id)
            .all(&database)
            .await
            .unwrap();
        let statuses: Vec<_> = jobs.iter().map(|job| job.status.as_str()).collect();
        assert_eq!(statuses, ["failure", "cancelled", "cancelled", "queued"]);
        assert_eq!(jobs[1].failure_kind.as_deref(), Some("fail_fast"));
    }
}
