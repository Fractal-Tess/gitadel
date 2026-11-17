//! Webhook events beyond `push`: issues, comments, releases, and workflow runs.
//!
//! Payloads follow the GitHub/Gitea shapes where the data model allows, so
//! existing receivers can parse them. Emission never fails the request that
//! triggered it: errors are logged and the triggering change stands.

use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    RepositoryState,
    webhooks::{dispatch_event, public_url, repository_payload, user_payload},
};
use crate::{
    entity::{
        action_run, issue_comment, issue_label, issue_label_assignment, release_asset, repository,
        repository_issue, repository_release, user,
    },
    identity::ApiError,
};

/// Actions reported through the `issues` event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IssueAction {
    Opened,
    Edited,
    Closed,
    Reopened,
    Deleted,
    Assigned,
    Unassigned,
    Labeled,
    Unlabeled,
}

impl IssueAction {
    const fn name(self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::Edited => "edited",
            Self::Closed => "closed",
            Self::Reopened => "reopened",
            Self::Deleted => "deleted",
            Self::Assigned => "assigned",
            Self::Unassigned => "unassigned",
            Self::Labeled => "labeled",
            Self::Unlabeled => "unlabeled",
        }
    }
}

/// Actions reported through the `issue_comment` and `release` events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChangeAction {
    Created,
    Edited,
    Deleted,
}

impl ChangeAction {
    const fn comment_name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Edited => "edited",
            Self::Deleted => "deleted",
        }
    }

    const fn release_name(self) -> &'static str {
        match self {
            Self::Created => "published",
            Self::Edited => "updated",
            Self::Deleted => "deleted",
        }
    }
}

/// Emit one `issues` event. `extra` is merged into the top-level payload
/// (for example `changes`, `label`, or `assignee`).
pub(crate) async fn issue(
    state: &RepositoryState,
    repository: &repository::Model,
    actor: &user::Model,
    issue: &repository_issue::Model,
    action: IssueAction,
    extra: Value,
) {
    let result = dispatch_event(state, repository.id, "issues", || async {
        let mut payload = json!({
            "action": action.name(),
            "number": issue.number,
            "issue": issue_payload(state, repository, issue).await?,
            "repository": repository_payload(state, repository),
            "sender": user_payload(actor),
        });
        merge(&mut payload, extra);
        Ok(payload)
    })
    .await;
    log_failure(result, "issues", repository.id);
}

/// Build an `issues` payload for a deletion before the rows disappear.
///
/// Returns `None` when no hook subscribes, so nothing is loaded needlessly.
pub(crate) async fn prepare_issue_deleted(
    state: &RepositoryState,
    repository: &repository::Model,
    actor: &user::Model,
    issue: &repository_issue::Model,
) -> Option<Value> {
    match super::webhooks::has_subscribers(state, repository.id, "issues").await {
        Ok(true) => {}
        Ok(false) => return None,
        Err(error) => {
            log_failure(Err(error), "issues", repository.id);
            return None;
        }
    }
    match issue_payload(state, repository, issue).await {
        Ok(issue_value) => Some(json!({
            "action": IssueAction::Deleted.name(),
            "number": issue.number,
            "issue": issue_value,
            "repository": repository_payload(state, repository),
            "sender": user_payload(actor),
        })),
        Err(error) => {
            log_failure(Err(error), "issues", repository.id);
            None
        }
    }
}

/// Deliver a payload prepared earlier, such as by [`prepare_issue_deleted`].
pub(crate) async fn send_prepared(
    state: &RepositoryState,
    repository_id: Uuid,
    event: &str,
    payload: Option<Value>,
) {
    let Some(payload) = payload else {
        return;
    };
    let result = dispatch_event(state, repository_id, event, || async { Ok(payload) }).await;
    log_failure(result, event, repository_id);
}

/// Emit one `issue_comment` event.
pub(crate) async fn issue_comment(
    state: &RepositoryState,
    repository: &repository::Model,
    actor: &user::Model,
    issue: &repository_issue::Model,
    comment: &issue_comment::Model,
    action: ChangeAction,
    previous_body: Option<&str>,
) {
    let result = dispatch_event(state, repository.id, "issue_comment", || async {
        let mut payload = json!({
            "action": action.comment_name(),
            "issue": issue_payload(state, repository, issue).await?,
            "comment": comment_payload(state, repository, issue, comment).await?,
            "is_pull": false,
            "repository": repository_payload(state, repository),
            "sender": user_payload(actor),
        });
        if let Some(previous) = previous_body {
            payload["changes"] = json!({ "body": { "from": previous } });
        }
        Ok(payload)
    })
    .await;
    log_failure(result, "issue_comment", repository.id);
}

/// Build a `release` payload. Deletions call this before removing the release
/// so its assets are still listed.
pub(crate) async fn prepare_release(
    state: &RepositoryState,
    repository: &repository::Model,
    actor_id: Uuid,
    release: &repository_release::Model,
    action: ChangeAction,
) -> Option<Value> {
    let build = async {
        if !super::webhooks::has_subscribers(state, repository.id, "release").await? {
            return Ok(None);
        }
        let actor = find_user(state, actor_id).await?;
        Ok::<_, ApiError>(Some(json!({
            "action": action.release_name(),
            "release": release_payload(state, repository, release).await?,
            "repository": repository_payload(state, repository),
            "sender": actor.as_ref().map(user_payload),
        })))
    };
    match build.await {
        Ok(payload) => payload,
        Err(error) => {
            log_failure(Err(error), "release", repository.id);
            None
        }
    }
}

/// Emit one `release` event.
pub(crate) async fn release(
    state: &RepositoryState,
    repository: &repository::Model,
    actor_id: Uuid,
    release: &repository_release::Model,
    action: ChangeAction,
) {
    let payload = prepare_release(state, repository, actor_id, release, action).await;
    send_prepared(state, repository.id, "release", payload).await;
}

/// Emit a `workflow_run` event (`requested` or `completed`) for a stored run.
pub(crate) async fn workflow_run(state: &RepositoryState, run: &action_run::Model, action: &str) {
    let result = async {
        let Some(repository) = repository::Entity::find_by_id(run.repository_id)
            .one(state.identity().database())
            .await?
        else {
            return Ok(());
        };
        dispatch_event(state, repository.id, "workflow_run", || async {
            let actor = match run.actor_id {
                Some(id) => find_user(state, id).await?,
                None => None,
            };
            Ok(json!({
                "action": action,
                "workflow_run": workflow_run_payload(state, &repository, run),
                "workflow": {
                    "name": run.workflow_name,
                    "path": run.workflow_path,
                },
                "repository": repository_payload(state, &repository),
                "sender": actor.as_ref().map(user_payload),
            }))
        })
        .await
    }
    .await;
    log_failure(result, "workflow_run", run.repository_id);
}

fn merge(payload: &mut Value, extra: Value) {
    if let (Some(target), Value::Object(extra)) = (payload.as_object_mut(), extra) {
        target.extend(extra);
    }
}

fn log_failure(result: Result<(), ApiError>, event: &str, repository_id: Uuid) {
    if let Err(error) = result {
        tracing::warn!(%error, event, %repository_id, "could not queue webhook event");
    }
}

async fn find_user(state: &RepositoryState, id: Uuid) -> Result<Option<user::Model>, ApiError> {
    Ok(user::Entity::find_by_id(id)
        .one(state.identity().database())
        .await?)
}

/// Browser URL for a repository view, matching the SvelteKit query scheme.
fn view_url(state: &RepositoryState, repository: &repository::Model, query: &str) -> String {
    let mut url = url::Url::parse(&public_url(
        state,
        &format!("/{}/{}", repository.namespace, repository.name),
    ))
    .expect("public URL is valid");
    url.set_query(Some(query));
    url.to_string()
}

fn api_url(state: &RepositoryState, repository: &repository::Model, suffix: &str) -> String {
    public_url(
        state,
        &format!(
            "/api/v1/repositories/{}/{}/{suffix}",
            repository.namespace, repository.name
        ),
    )
}

pub(super) fn issue_html_url(
    state: &RepositoryState,
    repository: &repository::Model,
    number: i64,
) -> String {
    view_url(state, repository, &format!("view=issues&issue={number}"))
}

async fn issue_payload(
    state: &RepositoryState,
    repository: &repository::Model,
    issue: &repository_issue::Model,
) -> Result<Value, ApiError> {
    let database = state.identity().database();
    let author = find_user(state, issue.author_user_id).await?;
    let assignee = match issue.assignee_user_id {
        Some(id) => find_user(state, id).await?,
        None => None,
    };
    let label_ids = issue_label_assignment::Entity::find()
        .filter(issue_label_assignment::Column::IssueId.eq(issue.id))
        .all(database)
        .await?
        .into_iter()
        .map(|assignment| assignment.label_id)
        .collect::<Vec<_>>();
    let labels = if label_ids.is_empty() {
        Vec::new()
    } else {
        issue_label::Entity::find()
            .filter(issue_label::Column::Id.is_in(label_ids))
            .order_by_asc(issue_label::Column::Name)
            .all(database)
            .await?
    };
    let comments = issue_comment::Entity::find()
        .filter(issue_comment::Column::IssueId.eq(issue.id))
        .count(database)
        .await?;
    let assignee = assignee.as_ref().map(user_payload);
    Ok(json!({
        "id": issue.id,
        "number": issue.number,
        "title": issue.title,
        "body": issue.body,
        "state": issue.state,
        "html_url": issue_html_url(state, repository, issue.number),
        "url": api_url(state, repository, &format!("issues/{}", issue.number)),
        "user": author.as_ref().map(user_payload),
        "assignees": assignee.iter().cloned().collect::<Vec<_>>(),
        "assignee": assignee,
        "labels": labels.iter().map(label_payload).collect::<Vec<_>>(),
        "comments": comments,
        "created_at": issue.created_at,
        "updated_at": issue.updated_at,
        "closed_at": issue.closed_at,
    }))
}

pub(crate) fn label_payload(label: &issue_label::Model) -> Value {
    json!({
        "id": label.id,
        "name": label.name,
        "color": label.color.trim_start_matches('#'),
        "description": label.description,
    })
}

async fn comment_payload(
    state: &RepositoryState,
    repository: &repository::Model,
    issue: &repository_issue::Model,
    comment: &issue_comment::Model,
) -> Result<Value, ApiError> {
    let author = find_user(state, comment.author_user_id).await?;
    Ok(json!({
        "id": comment.id,
        "body": comment.body,
        "user": author.as_ref().map(user_payload),
        "html_url": issue_html_url(state, repository, issue.number),
        "issue_url": api_url(state, repository, &format!("issues/{}", issue.number)),
        "created_at": comment.created_at,
        "updated_at": comment.updated_at,
    }))
}

async fn release_payload(
    state: &RepositoryState,
    repository: &repository::Model,
    release: &repository_release::Model,
) -> Result<Value, ApiError> {
    let author = find_user(state, release.author_user_id).await?;
    let assets = release_asset::Entity::find()
        .filter(release_asset::Column::ReleaseId.eq(release.id))
        .order_by_asc(release_asset::Column::Name)
        .all(state.identity().database())
        .await?;
    Ok(json!({
        "id": release.id,
        "tag_name": release.target_revision,
        "target_commitish": release.target_oid,
        "name": release.title,
        "body": release.body,
        "draft": false,
        "prerelease": release.prerelease,
        "created_at": release.created_at,
        "published_at": release.published_at,
        "html_url": view_url(state, repository, "view=releases"),
        "url": api_url(state, repository, &format!("releases/{}", release.id)),
        "author": author.as_ref().map(user_payload),
        "assets": assets
            .iter()
            .map(|asset| json!({
                "id": asset.id,
                "name": asset.name,
                "content_type": asset.content_type,
                "size": asset.size_bytes,
                "download_count": asset.download_count,
                "browser_download_url": api_url(
                    state,
                    repository,
                    &format!("releases/{}/assets/{}", release.id, asset.id),
                ),
            }))
            .collect::<Vec<_>>(),
    }))
}

/// GitHub reports `status` (queued/in_progress/completed) and a separate
/// `conclusion` once complete.
pub(crate) fn workflow_status(status: &str) -> (&'static str, Option<&'static str>) {
    match status {
        "success" => ("completed", Some("success")),
        "failure" => ("completed", Some("failure")),
        "cancelled" => ("completed", Some("cancelled")),
        "running" => ("in_progress", None),
        _ => ("queued", None),
    }
}

fn workflow_run_payload(
    state: &RepositoryState,
    repository: &repository::Model,
    run: &action_run::Model,
) -> Value {
    let (status, conclusion) = workflow_status(&run.status);
    let head_branch = run
        .ref_name
        .strip_prefix("refs/heads/")
        .or_else(|| run.ref_name.strip_prefix("refs/tags/"))
        .unwrap_or(&run.ref_name);
    json!({
        "id": run.id,
        "name": run.workflow_name,
        "path": run.workflow_path,
        "run_number": run.number,
        "event": run.event,
        "status": status,
        "conclusion": conclusion,
        "head_branch": head_branch,
        "head_sha": run.after_sha,
        "html_url": run_html_url(state, repository, run.id),
        "url": api_url(state, repository, &format!("actions/runs/{}", run.id)),
        "failure_summary": run.failure_summary,
        "created_at": run.created_at,
        "run_started_at": run.started_at,
        "updated_at": run.completed_at.or(run.started_at).unwrap_or(run.created_at),
    })
}

pub(crate) fn run_html_url(
    state: &RepositoryState,
    repository: &repository::Model,
    run_id: Uuid,
) -> String {
    view_url(state, repository, &format!("view=actions&run={run_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_status_maps_terminal_states_to_conclusions() {
        assert_eq!(workflow_status("queued"), ("queued", None));
        assert_eq!(workflow_status("waiting"), ("queued", None));
        assert_eq!(workflow_status("running"), ("in_progress", None));
        assert_eq!(workflow_status("failure"), ("completed", Some("failure")));
        assert_eq!(
            workflow_status("cancelled"),
            ("completed", Some("cancelled"))
        );
    }

    #[tokio::test]
    async fn events_reach_only_subscribed_hooks() {
        use chrono::Utc;
        use sea_orm::{ActiveModelTrait, ConnectOptions, Database, Set};
        use sea_orm_migration::MigratorTrait;

        use crate::{
            config::{AuthSettings, StorageSettings},
            entity::{repository_webhook, repository_webhook_delivery},
            identity::IdentityState,
            migration::Migrator,
        };

        let root = std::env::temp_dir().join(format!("gitadel-webhook-events-{}", Uuid::new_v4()));
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let database = Database::connect(options).await.unwrap();
        Migrator::up(&database, None).await.unwrap();
        let public_url = url::Url::parse("https://gitadel.test").unwrap();
        let identity = IdentityState::new(
            database.clone(),
            AuthSettings {
                session_lifetime_hours: 24,
                invitation_lifetime_hours: 24,
            },
            public_url.clone(),
        )
        .unwrap();
        let state = RepositoryState::new(
            identity,
            StorageSettings {
                repository_root: root.join("repositories"),
                lfs_root: root.join("lfs"),
                registry_root: root.join("registry"),
                actions_artifact_root: root.join("actions-artifacts"),
            },
            public_url,
            22,
        )
        .await
        .unwrap();
        let owner = crate::identity::bootstrap_admin(
            &database,
            "hook-owner",
            "webhook-test-password".to_owned(),
        )
        .await
        .unwrap();
        let now = Utc::now();
        let repository = repository::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set(owner.username.clone()),
            name: Set("events".to_owned()),
            description: Set(None),
            website_url: Set(None),
            visibility: Set("private".to_owned()),
            object_format: Set("sha1".to_owned()),
            mirrored: Set(false),
            default_branch: Set(Some("main".to_owned())),
            issue_counter: Set(1),
            storage_key: Set(Uuid::new_v4()),
            created_by: Set(owner.id),
            archived_at: Set(None),
            deleted_at: Set(None),
            icon_updated_at: Set(None),
            icon_source: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        let hook = repository_webhook::ActiveModel {
            id: Set(Uuid::new_v4()),
            repository_id: Set(repository.id),
            // Deliveries to this address fail; the test inspects the outbox.
            url: Set("http://127.0.0.1:9/hook".to_owned()),
            secret: Set(None),
            active: Set(true),
            created_at: Set(now),
            updated_at: Set(now),
            last_delivery_at: Set(None),
            last_response_status: Set(None),
            last_response_message: Set(None),
            events: Set("issues,issue_comment".to_owned()),
        }
        .insert(&database)
        .await
        .unwrap();
        let issue = repository_issue::ActiveModel {
            id: Set(Uuid::new_v4()),
            repository_id: Set(repository.id),
            number: Set(1),
            author_user_id: Set(owner.id),
            title: Set("Broken build".to_owned()),
            body: Set("It fails.".to_owned()),
            state: Set("open".to_owned()),
            assignee_user_id: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            closed_at: Set(None),
            external_source: Set(None),
            external_id: Set(None),
            external_url: Set(None),
            external_author: Set(None),
            external_author_url: Set(None),
            external_updated_at: Set(None),
        }
        .insert(&database)
        .await
        .unwrap();

        super::issue(
            &state,
            &repository,
            &owner,
            &issue,
            IssueAction::Opened,
            json!({}),
        )
        .await;
        let run = action_run::Model {
            id: Uuid::new_v4(),
            repository_id: repository.id,
            number: 1,
            workflow_path: ".forgejo/workflows/ci.yml".to_owned(),
            workflow_name: "CI".to_owned(),
            event: "push".to_owned(),
            ref_name: "refs/heads/main".to_owned(),
            before_sha: "0".repeat(40),
            after_sha: "1".repeat(40),
            actor_id: Some(owner.id),
            status: "failure".to_owned(),
            failure_kind: None,
            failure_summary: None,
            diagnostic: None,
            event_json: "{}".to_owned(),
            cancel_requested_at: None,
            cancelled_by: None,
            rerun_of: None,
            run_attempt: 1,
            created_at: now,
            started_at: None,
            completed_at: Some(now),
        };
        super::workflow_run(&state, &run, "completed").await;

        let deliveries = repository_webhook_delivery::Entity::find()
            .filter(repository_webhook_delivery::Column::WebhookId.eq(hook.id))
            .filter(repository_webhook_delivery::Column::Event.ne("ping"))
            .all(&database)
            .await
            .unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].event, "issues");
        let payload: Value = serde_json::from_str(&deliveries[0].payload).unwrap();
        assert_eq!(payload["action"], "opened");
        assert_eq!(payload["issue"]["number"], 1);
        assert_eq!(payload["issue"]["title"], "Broken build");
        assert_eq!(payload["sender"]["login"], "hook-owner");
        assert_eq!(
            payload["issue"]["html_url"],
            "https://gitadel.test/hook-owner/events?view=issues&issue=1"
        );
        state.close_tasks();
        state.wait_for_tasks().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn extra_fields_merge_into_the_payload() {
        let mut payload = json!({ "action": "labeled" });
        merge(&mut payload, json!({ "label": { "name": "bug" } }));
        assert_eq!(payload["label"]["name"], "bug");
        assert_eq!(payload["action"], "labeled");
    }
}
