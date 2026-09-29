//! Email notifications for issue activity and failed workflow runs.
//!
//! Each entry point returns immediately: recipients are resolved and mail is
//! queued on the repository task tracker. Nothing happens when SMTP is not
//! configured. Only enabled users with a verified address, the matching
//! preference switched on, and read access to the repository are mailed, and
//! the person who caused the event is never notified about it.

use std::collections::BTreeSet;

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{
    Permission, RepositoryState,
    webhook_events::{issue_html_url, run_html_url},
};
use crate::{
    entity::{
        action_run, issue_comment, namespace, organization_member, repository, repository_issue,
        user,
    },
    identity::{ApiError, NotificationKind, notification_preferences, verified_mailbox},
    mail::Email,
};

/// Longest body excerpt quoted in a notification.
const EXCERPT_LIMIT: usize = 2_000;

pub(crate) fn issue_opened(
    state: &RepositoryState,
    repository: &repository::Model,
    actor: &user::Model,
    issue: &repository_issue::Model,
) {
    if !state.identity().mailer().is_enabled() {
        return;
    }
    let (task_state, repository, actor, issue) = (
        state.clone(),
        repository.clone(),
        actor.clone(),
        issue.clone(),
    );
    state.spawn_task(async move {
        let result = async {
            let mut recipients = owners(&task_state, &repository).await?;
            recipients.extend(issue.assignee_user_id);
            let subject = format!(
                "[{}/{}] Issue #{}: {}",
                repository.namespace, repository.name, issue.number, issue.title
            );
            let body = format!(
                "{} opened issue #{} in {}/{}.\n\n{}\n\n{}\n",
                actor.username,
                issue.number,
                repository.namespace,
                repository.name,
                excerpt(&issue.body),
                issue_html_url(&task_state, &repository, issue.number),
            );
            send(
                &task_state,
                &repository,
                actor.id,
                recipients,
                NotificationKind::Issues,
                &subject,
                &body,
            )
            .await
        }
        .await;
        log_failure(result, "issue opened");
    });
}

pub(crate) fn issue_assigned(
    state: &RepositoryState,
    repository: &repository::Model,
    actor: &user::Model,
    issue: &repository_issue::Model,
) {
    let Some(assignee) = issue.assignee_user_id else {
        return;
    };
    if !state.identity().mailer().is_enabled() {
        return;
    }
    let (task_state, repository, actor, issue) = (
        state.clone(),
        repository.clone(),
        actor.clone(),
        issue.clone(),
    );
    state.spawn_task(async move {
        let subject = format!(
            "[{}/{}] Issue #{} assigned to you: {}",
            repository.namespace, repository.name, issue.number, issue.title
        );
        let body = format!(
            "{} assigned you issue #{} in {}/{}.\n\n{}\n\n{}\n",
            actor.username,
            issue.number,
            repository.namespace,
            repository.name,
            excerpt(&issue.body),
            issue_html_url(&task_state, &repository, issue.number),
        );
        let result = send(
            &task_state,
            &repository,
            actor.id,
            BTreeSet::from([assignee]),
            NotificationKind::Issues,
            &subject,
            &body,
        )
        .await;
        log_failure(result, "issue assigned");
    });
}

pub(crate) fn issue_commented(
    state: &RepositoryState,
    repository: &repository::Model,
    actor: &user::Model,
    issue: &repository_issue::Model,
    comment: &issue_comment::Model,
) {
    if !state.identity().mailer().is_enabled() {
        return;
    }
    let (task_state, repository, actor, issue, comment) = (
        state.clone(),
        repository.clone(),
        actor.clone(),
        issue.clone(),
        comment.clone(),
    );
    state.spawn_task(async move {
        let result = async {
            let mut recipients = owners(&task_state, &repository).await?;
            recipients.extend(issue.assignee_user_id);
            recipients.insert(issue.author_user_id);
            let subject = format!(
                "Re: [{}/{}] Issue #{}: {}",
                repository.namespace, repository.name, issue.number, issue.title
            );
            let body = format!(
                "{} commented on issue #{} in {}/{}.\n\n{}\n\n{}\n",
                actor.username,
                issue.number,
                repository.namespace,
                repository.name,
                excerpt(&comment.body),
                issue_html_url(&task_state, &repository, issue.number),
            );
            send(
                &task_state,
                &repository,
                actor.id,
                recipients,
                NotificationKind::IssueComments,
                &subject,
                &body,
            )
            .await
        }
        .await;
        log_failure(result, "issue comment");
    });
}

/// Tell the user who triggered a run that it failed. Runs they cancelled
/// themselves, or that succeeded, send nothing.
pub(crate) async fn run_failed(state: &RepositoryState, run: &action_run::Model) {
    if run.status != "failure" || !state.identity().mailer().is_enabled() {
        return;
    }
    let Some(actor_id) = run.actor_id else {
        return;
    };
    let result = async {
        let Some(repository) = repository::Entity::find_by_id(run.repository_id)
            .one(state.identity().database())
            .await?
        else {
            return Ok(());
        };
        let reference = run
            .ref_name
            .strip_prefix("refs/heads/")
            .or_else(|| run.ref_name.strip_prefix("refs/tags/"))
            .unwrap_or(&run.ref_name);
        let short_sha = run.after_sha.get(..12).unwrap_or(&run.after_sha);
        let subject = format!(
            "[{}/{}] Run #{} failed: {}",
            repository.namespace, repository.name, run.number, run.workflow_name
        );
        let summary = run
            .failure_summary
            .as_deref()
            .map(|summary| format!("\n{}\n", excerpt(summary)))
            .unwrap_or_default();
        let body = format!(
            "Workflow \"{}\" failed for {} at {} in {}/{}.\n{}\n{}\n",
            run.workflow_name,
            reference,
            short_sha,
            repository.namespace,
            repository.name,
            summary,
            run_html_url(state, &repository, run.id),
        );
        // The pusher is the audience here, so their own action still counts.
        send(
            state,
            &repository,
            Uuid::nil(),
            BTreeSet::from([actor_id]),
            NotificationKind::ActionFailures,
            &subject,
            &body,
        )
        .await
    }
    .await;
    log_failure(result, "failed run");
}

/// Users who own the repository: the personal owner or organization owners.
async fn owners(
    state: &RepositoryState,
    repository: &repository::Model,
) -> Result<BTreeSet<Uuid>, ApiError> {
    let database = state.identity().database();
    let Some(namespace) = namespace::Entity::find_by_id(&repository.namespace)
        .one(database)
        .await?
    else {
        return Ok(BTreeSet::new());
    };
    let mut owners = BTreeSet::new();
    owners.extend(namespace.user_id);
    if let Some(organization_id) = namespace.organization_id {
        owners.extend(
            organization_member::Entity::find()
                .filter(organization_member::Column::OrganizationId.eq(organization_id))
                .filter(organization_member::Column::Role.eq("owner"))
                .all(database)
                .await?
                .into_iter()
                .map(|member| member.user_id),
        );
    }
    Ok(owners)
}

async fn send(
    state: &RepositoryState,
    repository: &repository::Model,
    actor_id: Uuid,
    recipients: BTreeSet<Uuid>,
    kind: NotificationKind,
    subject: &str,
    body: &str,
) -> Result<(), ApiError> {
    let database = state.identity().database();
    let settings_url = {
        let mut url = state.public_url().clone();
        url.set_path("/-/account/profile");
        url
    };
    for user_id in recipients.into_iter().filter(|id| *id != actor_id) {
        if !notification_preferences(database, user_id)
            .await?
            .allows(kind)
        {
            continue;
        }
        if !state
            .can_access(repository, Some(user_id), Permission::Read)
            .await?
        {
            continue;
        }
        let Some((_, mailbox)) = verified_mailbox(database, user_id).await? else {
            continue;
        };
        state.identity().mailer().enqueue(Email {
            to: mailbox,
            subject: subject.to_owned(),
            body: format!(
                "{body}\n--\nYou can change which emails Gitadel sends you at {settings_url}\n"
            ),
        });
    }
    Ok(())
}

fn excerpt(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return "(No description.)".to_owned();
    }
    match text.char_indices().nth(EXCERPT_LIMIT) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

fn log_failure(result: Result<(), ApiError>, notification: &str) {
    if let Err(error) = result {
        tracing::warn!(%error, notification, "could not queue notification email");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn repository_owner_is_emailed_when_someone_opens_an_issue() {
        use chrono::Utc;
        use sea_orm::{ActiveModelTrait, ConnectOptions, Database, Set};
        use sea_orm_migration::MigratorTrait;

        use crate::{
            config::{AuthSettings, StorageSettings},
            entity::user_email,
            identity::IdentityState,
            mail::{fake_smtp_server, test_mailer},
            migration::Migrator,
        };

        let (port, server) = fake_smtp_server().await;
        let root = std::env::temp_dir().join(format!("gitadel-notify-{}", Uuid::new_v4()));
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
        .unwrap()
        .with_mailer(test_mailer(port));
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
            "alice",
            "notification-password".to_owned(),
        )
        .await
        .unwrap();
        let now = Utc::now();
        user_email::ActiveModel {
            user_id: Set(owner.id),
            email: Set("alice@example.com".to_owned()),
            verified_at: Set(Some(now)),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        let reporter = user::ActiveModel {
            id: Set(Uuid::new_v4()),
            username: Set("bob".to_owned()),
            password_hash: Set("unused".to_owned()),
            is_admin: Set(false),
            default_repository_visibility: Set("private".to_owned()),
            theme_preference: Set("system".to_owned()),
            disabled_at: Set(None),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        let repository = repository::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set("alice".to_owned()),
            name: Set("tools".to_owned()),
            description: Set(None),
            website_url: Set(None),
            visibility: Set("public".to_owned()),
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
        let issue = repository_issue::Model {
            id: Uuid::new_v4(),
            repository_id: repository.id,
            number: 1,
            author_user_id: reporter.id,
            title: "Crash on start".to_owned(),
            body: "It crashes.".to_owned(),
            state: "open".to_owned(),
            assignee_user_id: None,
            created_at: now,
            updated_at: now,
            closed_at: None,
            external_source: None,
            external_id: None,
            external_url: None,
            external_author: None,
            external_author_url: None,
            external_updated_at: None,
        };

        issue_opened(&state, &repository, &reporter, &issue);

        let data = tokio::time::timeout(std::time::Duration::from_secs(10), server)
            .await
            .expect("the owner is emailed")
            .unwrap()
            // Undo the quoted-printable soft breaks and `=` escapes.
            .replace("=\n", "")
            .replace("=3D", "=");
        assert!(data.contains("To: alice <alice@example.com>"), "{data}");
        assert!(data.contains("Subject: [alice/tools] Issue #1: Crash on start"));
        assert!(data.contains("bob opened issue #1"));
        assert!(
            data.contains("https://gitadel.test/alice/tools?view=issues&issue=1"),
            "{data}"
        );
        state.close_tasks();
        state.wait_for_tasks().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preferences_gate_each_kind() {
        let preferences = crate::identity::NotificationPreferences {
            issues: false,
            issue_comments: true,
            action_failures: false,
        };
        assert!(!preferences.allows(NotificationKind::Issues));
        assert!(preferences.allows(NotificationKind::IssueComments));
        assert!(!preferences.allows(NotificationKind::ActionFailures));
    }

    #[test]
    fn excerpts_are_bounded_and_never_empty() {
        assert_eq!(excerpt("  "), "(No description.)");
        assert_eq!(excerpt("short"), "short");
        let long = "é".repeat(EXCERPT_LIMIT + 10);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), EXCERPT_LIMIT + 1);
        assert!(cut.ends_with('…'));
    }
}
