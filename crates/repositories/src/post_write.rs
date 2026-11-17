//! Prepared effects for committed Git refs, independent of transport success.
use crate::{
    content::{RepositoryContent, read_git},
    webhook_outbox::WebhookOutbox,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gitadel_db::entity::{repository, repository_webhook, user};
use gitadel_identity::IdentityError;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use sley::{DiffNameStatusOptions, ReferenceTarget};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use url::Url;
use uuid::Uuid;

pub type RefSnapshot = BTreeMap<String, String>;

/// Server coordination is explicit: Actions failures are logged while durable
/// webhook failures stop later effects, matching the committed-write contract.
/// Implementations start deployment tasks on the server's supervised tracker.
#[async_trait]
pub trait PostWriteEffects: Send + Sync {
    async fn deployments_enabled(&self, repository_id: Uuid) -> Result<bool, IdentityError>;
    async fn actions(
        &self,
        repository: &repository::Model,
        actor_id: Uuid,
        change: &PreparedRef,
    ) -> Result<(), String>;
    fn deploy(&self, repository: &repository::Model, change: &PreparedRef);
}

/// Native outbound presentation, shared by every transport producing a push.
pub struct PushPresentation {
    pub public_url: Url,
    pub http_clone_url: String,
    pub ssh_clone_url: String,
}
impl PushPresentation {
    fn repository_payload(&self, repository: &repository::Model) -> Value {
        let full_name = format!("{}/{}", repository.namespace, repository.name);
        let address = |path: &str| {
            let mut url = self.public_url.clone();
            url.set_path(path);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        };
        json!({
            "id":repository.id,"name":repository.name,"full_name":full_name,
            "private":repository.visibility=="private",
            "owner":{"login":repository.namespace,"type":"User"},
            "html_url":address(&format!("/{full_name}")),
            "url":address(&format!("/api/v1/repos/{full_name}")),
            "clone_url":self.http_clone_url,"ssh_url":self.ssh_clone_url,
            "default_branch":repository.default_branch,"archived":repository.archived_at.is_some()
        })
    }
}

/// Ref data is captured after Git commits. Delivery must still run when the
/// transport cannot send its response; it must never infer success from HTTP.
pub struct PreparedRef {
    pub reference: String,
    pub before: String,
    pub after: String,
    pub zero: String,
    pub changed_paths: Vec<String>,
    pub payload: Value,
    ref_event: Option<(&'static str, Value)>,
}
impl PreparedRef {
    pub fn deleted(&self) -> bool {
        self.after == self.zero
    }
}
pub struct PreparedPostWrite {
    repository: repository::Model,
    actor_id: Uuid,
    hooks: Vec<repository_webhook::Model>,
    deployments_enabled: bool,
    refs: Vec<PreparedRef>,
}
#[derive(Clone)]
pub struct PostWriteService {
    database: DatabaseConnection,
    content: RepositoryContent,
    outbox: WebhookOutbox,
}
impl PostWriteService {
    /// Pure construction; no worker, scan or recovery is started.
    pub fn new(
        database: DatabaseConnection,
        content: RepositoryContent,
        outbox: WebhookOutbox,
    ) -> Self {
        Self {
            database,
            content,
            outbox,
        }
    }
    pub async fn prepare(
        &self,
        repository: &repository::Model,
        actor_id: Uuid,
        before: RefSnapshot,
        context: &PushPresentation,
        effects: &dyn PostWriteEffects,
    ) -> Result<PreparedPostWrite, IdentityError> {
        let hooks = repository_webhook::Entity::find()
            .filter(repository_webhook::Column::RepositoryId.eq(repository.id))
            .filter(repository_webhook::Column::Active.eq(true))
            .all(&self.database)
            .await?;
        let deployments_enabled = effects.deployments_enabled(repository.id).await?;
        let path = self.content.repository_path(repository);
        let after = snapshot_refs(&path).await?;
        let actor = user::Entity::find_by_id(actor_id)
            .one(&self.database)
            .await?
            .ok_or_else(IdentityError::not_found)?;
        let zero = if repository.object_format == "sha256" {
            "0".repeat(64)
        } else {
            "0".repeat(40)
        };
        let mut refs = Vec::new();
        for (reference, old_oid, new_oid) in changed_refs(&before, &after, &zero) {
            let commits = commit_entries(&path, old_oid, new_oid, &zero).await;
            let changed_paths = pushed_paths(&commits);
            let pushed = PushedRef {
                reference,
                before: old_oid,
                after: new_oid,
                zero: &zero,
            };
            let payload = push_payload(context, repository, &actor, &pushed, commits);
            let event = if old_oid == zero {
                Some("create")
            } else if new_oid == zero {
                Some("delete")
            } else {
                None
            };
            let ref_event = event.and_then(|event| {
                ref_event_payload(context, repository, &actor, reference, new_oid, event)
                    .map(|payload| (event, payload))
            });
            refs.push(PreparedRef {
                reference: reference.to_owned(),
                before: old_oid.to_owned(),
                after: new_oid.to_owned(),
                zero: zero.clone(),
                changed_paths,
                payload,
                ref_event,
            });
        }
        Ok(PreparedPostWrite {
            repository: repository.clone(),
            actor_id,
            hooks,
            deployments_enabled,
            refs,
        })
    }
    /// Consume a prepared operation once. There is deliberately no generic
    /// event bus and no dependency on Actions or integration implementations.
    pub async fn deliver(
        &self,
        prepared: PreparedPostWrite,
        effects: &dyn PostWriteEffects,
    ) -> Result<(), IdentityError> {
        for change in prepared.refs {
            if let Err(error) = effects
                .actions(&prepared.repository, prepared.actor_id, &change)
                .await
            {
                tracing::error!(%error,repository_id=%prepared.repository.id,reference=%change.reference,commit=%change.after,"could not enqueue Actions workflows");
            }
            for hook in prepared
                .hooks
                .iter()
                .filter(|hook| subscribes(hook, "push"))
            {
                self.outbox
                    .insert_delivery(hook, "push", &change.payload)
                    .await?;
            }
            if let Some((event, payload)) = &change.ref_event {
                for hook in prepared.hooks.iter().filter(|hook| subscribes(hook, event)) {
                    self.outbox.insert_delivery(hook, event, payload).await?;
                }
            }
            if prepared.deployments_enabled {
                effects.deploy(&prepared.repository, &change);
            }
        }
        self.outbox.process_pending_deliveries().await
    }
}
fn subscribes(hook: &repository_webhook::Model, event: &str) -> bool {
    hook.events
        .split(',')
        .map(str::trim)
        .any(|selected| selected == event)
}
fn user_payload(actor: &user::Model) -> Value {
    json!({"id":actor.id,"login":actor.username,"type":"User"})
}
#[cfg(test)]
mod direct_boundaries {
    use super::*;
    use gitadel_db::{entity::repository_webhook_delivery, test_support};
    use gitadel_secrets::SecretCipher;
    use sea_orm::{ActiveModelTrait, Set};
    use std::{process::Command, sync::Mutex};
    use tokio_util::task::TaskTracker;

    #[derive(Default)]
    struct Effects(Mutex<Vec<String>>);
    #[async_trait]
    impl PostWriteEffects for Effects {
        async fn deployments_enabled(&self, _: Uuid) -> Result<bool, IdentityError> {
            Ok(true)
        }
        async fn actions(
            &self,
            _: &repository::Model,
            _: Uuid,
            change: &PreparedRef,
        ) -> Result<(), String> {
            assert!(change.deleted());
            self.0.lock().unwrap().push("actions".to_owned());
            Err("planner unavailable".to_owned())
        }
        fn deploy(&self, _: &repository::Model, change: &PreparedRef) {
            assert_eq!(change.payload["deleted"], true);
            self.0.lock().unwrap().push("deploy".to_owned());
        }
    }

    #[tokio::test]
    async fn committed_deletion_delivers_without_a_transport_and_survives_actions_failure() {
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "owner").await;
        let repository = test_support::repository(&database, &owner, "project").await;
        let root = std::env::temp_dir().join(format!("gitadel-post-write-{}", Uuid::new_v4()));
        let content = RepositoryContent::new(root.clone());
        let path = content.repository_path(&repository);
        assert!(
            Command::new("git")
                .args(["init", "--bare", "--quiet"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let now = Utc::now();
        let hook = repository_webhook::ActiveModel {
            id: Set(Uuid::new_v4()),
            repository_id: Set(repository.id),
            url: Set("http://127.0.0.1:9/hook".to_owned()),
            secret: Set(None),
            active: Set(true),
            created_at: Set(now),
            updated_at: Set(now),
            last_delivery_at: Set(None),
            last_response_status: Set(None),
            last_response_message: Set(None),
            events: Set("push,delete".to_owned()),
        }
        .insert(&database)
        .await
        .unwrap();
        let tracker = TaskTracker::new();
        let outbox = WebhookOutbox::new(
            database.clone(),
            SecretCipher::from_material(b"post-write-test").unwrap(),
            tracker.clone(),
        );
        let service = PostWriteService::new(database.clone(), content, outbox);
        let effects = Effects::default();
        let prepared = service
            .prepare(
                &repository,
                owner.id,
                BTreeMap::from([("refs/heads/main".to_owned(), "1".repeat(40))]),
                &PushPresentation {
                    public_url: Url::parse("https://gitadel.test").unwrap(),
                    http_clone_url: "https://gitadel.test/owner/project.git".to_owned(),
                    ssh_clone_url: "git@gitadel.test:owner/project.git".to_owned(),
                },
                &effects,
            )
            .await
            .unwrap();
        // Committed effects finish without a response, stream or HTTP status.
        service.deliver(prepared, &effects).await.unwrap();
        tracker.close();
        tracker.wait().await;
        assert_eq!(*effects.0.lock().unwrap(), ["actions", "deploy"]);
        let deliveries = repository_webhook_delivery::Entity::find()
            .filter(repository_webhook_delivery::Column::WebhookId.eq(hook.id))
            .all(&database)
            .await
            .unwrap();
        assert_eq!(deliveries.len(), 2);
        assert!(deliveries.iter().any(|delivery| delivery.event == "delete"));
        let push = deliveries
            .iter()
            .find(|delivery| delivery.event == "push")
            .unwrap();
        let payload: Value = serde_json::from_str(&push.payload).unwrap();
        assert_eq!(payload["before"], "1".repeat(40));
        assert_eq!(payload["after"], "0".repeat(40));
        assert_eq!(payload["repository"]["full_name"], "owner/project");
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub async fn snapshot_refs(path: &Path) -> Result<RefSnapshot, IdentityError> {
    let path = path.to_owned();
    read_git(path, |repository| {
        let mut snapshot = RefSnapshot::new();
        for reference in repository
            .references()
            .list_refs_with_prefix("refs/heads")?
        {
            if let ReferenceTarget::Direct(oid) = reference.target {
                snapshot.insert(reference.name, oid.to_hex());
            }
        }
        for reference in repository.references().list_refs_with_prefix("refs/tags")? {
            if let ReferenceTarget::Direct(oid) = reference.target {
                snapshot.insert(reference.name, oid.to_hex());
            }
        }
        Ok(snapshot)
    })
    .await
}

fn ref_event_payload(
    context: &PushPresentation,
    repository: &repository::Model,
    actor: &user::Model,
    reference: &str,
    sha: &str,
    event: &str,
) -> Option<Value> {
    let (ref_type, name) = if let Some(name) = reference.strip_prefix("refs/heads/") {
        ("branch", name)
    } else {
        ("tag", reference.strip_prefix("refs/tags/")?)
    };
    let mut payload = json!({
        "ref": name,
        "ref_type": ref_type,
        "pusher_type": "user",
        "repository": context.repository_payload(repository),
        "sender": user_payload(actor),
    });
    if event == "create" {
        payload["sha"] = json!(sha);
        payload["master_branch"] = json!(repository.default_branch);
        payload["description"] = json!(repository.description);
    }
    Some(payload)
}

fn pushed_paths(commits: &[Value]) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for commit in commits {
        for field in ["added", "modified", "removed"] {
            if let Some(values) = commit.get(field).and_then(Value::as_array) {
                paths.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        }
    }
    paths.into_iter().collect()
}

fn changed_refs<'a>(
    before: &'a RefSnapshot,
    after: &'a RefSnapshot,
    zero: &'a str,
) -> Vec<(&'a str, &'a str, &'a str)> {
    let mut names = before.keys().chain(after.keys()).collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .filter_map(|name| {
            let old_oid = before.get(name).map_or(zero, String::as_str);
            let new_oid = after.get(name).map_or(zero, String::as_str);
            (old_oid != new_oid).then_some((name.as_str(), old_oid, new_oid))
        })
        .collect()
}

/// Maximum commits enumerated per pushed ref.
///
/// Gitea caps its own push payloads similarly; a branch created from a large
/// import would otherwise walk the entire history.
const PUSH_COMMIT_LIMIT: usize = 50;

/// The four values describing one pushed ref, which always travel together.
struct PushedRef<'a> {
    reference: &'a str,
    before: &'a str,
    after: &'a str,
    zero: &'a str,
}

fn push_payload(
    context: &PushPresentation,
    repository: &repository::Model,
    actor: &user::Model,
    pushed: &PushedRef<'_>,
    commits: Vec<Value>,
) -> Value {
    let PushedRef {
        reference,
        before,
        after,
        zero,
    } = *pushed;
    json!({
        "ref": reference,
        "before": before,
        "after": after,
        "created": before == zero,
        "deleted": after == zero,
        "forced": false,
        "base_ref": null,
        "compare": null,
        "head_commit": commits.last().cloned(),
        "commits": commits,
        "repository": context.repository_payload(repository),
        "pusher": { "name": actor.username },
        "sender": user_payload(actor),
    })
}

/// Enumerate the commits a push introduced, with their changed paths.
///
/// Consumers filter on these paths — Dokploy's watch paths are exactly this —
/// so an empty list silently disables that filtering. Failures degrade to an
/// empty list rather than blocking the push.
async fn commit_entries(path: &Path, before: &str, after: &str, zero: &str) -> Vec<Value> {
    if after == zero {
        return Vec::new();
    }
    let is_new = before == zero;
    let path = path.to_owned();
    let before = before.to_owned();
    let after = after.to_owned();
    read_git(path, move |repository| {
        let mut selection = sley_rev::RevisionSelection::new();
        if is_new {
            selection.add_spec(&after)?;
        } else {
            selection.add_spec(format!("{before}..{after}"))?;
        }
        let resolved = selection.resolve(
            repository.git_dir(),
            repository.object_format(),
            repository.object_database(),
        )?;
        let mut commits = resolved.selected_commit_oids(
            repository.git_dir(),
            repository.object_format(),
            repository.object_database(),
            false,
        )?;
        commits.truncate(PUSH_COMMIT_LIMIT);
        commits.reverse();
        let mut entries = Vec::with_capacity(commits.len());
        for oid in commits {
            let commit = repository.read_commit(&oid)?;
            let options = DiffNameStatusOptions {
                detect_renames: true,
                ..DiffNameStatusOptions::default()
            };
            let changes = if let Some(parent) = commit.parents.first().copied() {
                let parent_tree = repository.read_commit(&parent)?.tree;
                sley_diff_merge::diff_name_status_trees_with_options(
                    repository.object_database(),
                    repository.object_format(),
                    &parent_tree,
                    &commit.tree,
                    options,
                )?
            } else {
                sley_diff_merge::diff_name_status_empty_tree_with_options(
                    repository.object_database(),
                    repository.object_format(),
                    &commit.tree,
                    options,
                )?
            };
            let mut added = Vec::new();
            let mut modified = Vec::new();
            let mut removed = Vec::new();
            for change in changes {
                let path = String::from_utf8_lossy(change.path.as_bytes()).into_owned();
                match change.status {
                    sley_diff_merge::NameStatus::Added => added.push(path),
                    sley_diff_merge::NameStatus::Deleted => removed.push(path),
                    sley_diff_merge::NameStatus::Renamed(_)
                    | sley_diff_merge::NameStatus::Copied(_) => {
                        if let Some(old_path) = change.old_path {
                            removed.push(String::from_utf8_lossy(old_path.as_bytes()).into_owned());
                        }
                        added.push(path);
                    }
                    _ => modified.push(path),
                }
            }
            let message = String::from_utf8_lossy(&commit.message).trim().to_owned();
            let author = commit.author_signature();
            let committer = commit.committer_signature();
            let timestamp = committer
                .as_ref()
                .and_then(|signature| DateTime::<Utc>::from_timestamp(signature.time.seconds, 0))
                .map_or_else(String::new, |timestamp| timestamp.to_rfc3339());
            let author_name = author.as_ref().map_or_else(String::new, |signature| {
                String::from_utf8_lossy(signature.name.as_bytes()).into_owned()
            });
            let author_email = author.as_ref().map_or_else(String::new, |signature| {
                String::from_utf8_lossy(signature.email.as_bytes()).into_owned()
            });
            let committer_name = committer.as_ref().map_or_else(String::new, |signature| {
                String::from_utf8_lossy(signature.name.as_bytes()).into_owned()
            });
            let committer_email = committer.as_ref().map_or_else(String::new, |signature| {
                String::from_utf8_lossy(signature.email.as_bytes()).into_owned()
            });
            entries.push(json!({
                "id": oid.to_hex(),
                "message": message.lines().next().unwrap_or_default(),
                "timestamp": timestamp,
                "author": { "name": author_name, "email": author_email },
                "committer": { "name": committer_name, "email": committer_email },
                "added": added,
                "modified": modified,
                "removed": removed,
            }));
        }
        Ok(entries)
    })
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(%error, "listing pushed commits failed");
        Vec::new()
    })
}
