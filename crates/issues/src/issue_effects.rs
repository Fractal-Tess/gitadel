//! Explicit issue side effects; the application supplies delivery wiring.
use crate::issues::IssueService;
use gitadel_db::entity::{issue_comment, issue_label, repository, repository_issue, user};
use serde_json::Value;
use uuid::Uuid;
/// Actions reported through the `issues` event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IssueAction {
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
    pub const fn name(self) -> &'static str {
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
pub enum ChangeAction {
    Created,
    Edited,
    Deleted,
}

impl ChangeAction {
    pub const fn comment_name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Edited => "edited",
            Self::Deleted => "deleted",
        }
    }

    pub const fn release_name(self) -> &'static str {
        match self {
            Self::Created => "published",
            Self::Edited => "updated",
            Self::Deleted => "deleted",
        }
    }
}

#[async_trait::async_trait]
pub trait IssueEffects: Send + Sync {
    async fn issue(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
        action: IssueAction,
        extra: Value,
    );
    async fn comment(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
        comment: &issue_comment::Model,
        action: ChangeAction,
        previous_body: Option<&str>,
    );
    async fn prepare_deleted(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
    ) -> Option<Value>;
    async fn send_deleted(&self, repository_id: Uuid, payload: Option<Value>);
    fn opened(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
    );
    fn assigned(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
    );
    fn commented(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
        comment: &issue_comment::Model,
    );
}
pub async fn issue(
    state: &IssueService<'_>,
    repository: &repository::Model,
    actor: &user::Model,
    issue: &repository_issue::Model,
    action: IssueAction,
    extra: Value,
) {
    state
        .effects
        .issue(repository, actor, issue, action, extra)
        .await;
}
pub async fn issue_comment(
    state: &IssueService<'_>,
    repository: &repository::Model,
    actor: &user::Model,
    issue: &repository_issue::Model,
    comment: &issue_comment::Model,
    action: ChangeAction,
    previous_body: Option<&str>,
) {
    state
        .effects
        .comment(repository, actor, issue, comment, action, previous_body)
        .await;
}
pub async fn prepare_issue_deleted(
    state: &IssueService<'_>,
    repository: &repository::Model,
    actor: &user::Model,
    issue: &repository_issue::Model,
) -> Option<Value> {
    state
        .effects
        .prepare_deleted(repository, actor, issue)
        .await
}
pub async fn send_prepared(
    state: &IssueService<'_>,
    repository_id: Uuid,
    _event: &str,
    payload: Option<Value>,
) {
    state.effects.send_deleted(repository_id, payload).await;
}
pub fn user_payload(actor: &user::Model) -> Value {
    serde_json::json!({"id": actor.id, "login": actor.username, "type": "User"})
}
pub fn label_payload(label: &issue_label::Model) -> Value {
    serde_json::json!({"id": label.id, "name": label.name, "color": label.color.trim_start_matches('#'), "description": label.description})
}
