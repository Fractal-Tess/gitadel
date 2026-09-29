//! Every scenario runs for each storage domain, so LFS and the registry are
//! served by the same API guarantees.

use std::path::PathBuf;

use axum::http::StatusCode;
use chrono::Utc;
use sea_orm::{ActiveModelTrait as _, Set};
use sha2::{Digest as _, Sha256};

use super::*;
use crate::{
    blob_store::{
        BlobDigest, ObjectKey, StorageDomain as _,
        targets::{self, StorageTargetConfiguration},
    },
    config::Settings,
    database,
    entity::lfs_object,
    identity,
};

const DOMAIN_NAMES: [&str; 2] = ["lfs", "registry"];

struct Fixture {
    root: PathBuf,
    state: IdentityState,
    admin_id: Uuid,
    /// `alice/alpha` (100 bytes), `team/beta` (300 bytes), and `alice/gamma`
    /// (200 bytes). `alice/deleted` holds data but is soft-deleted.
    repositories: HashMap<&'static str, repository::Model>,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("gitadel-admin-storage-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let mut settings = Settings::default();
        settings.database.url = format!("sqlite://{}?mode=rwc", root.join("gitadel.db").display());
        settings.storage.repository_root = root.join("repositories");
        settings.storage.lfs_root = root.join("lfs");
        settings.storage.actions_artifact_root = root.join("actions");
        let database = database::connect_and_migrate(&settings.database)
            .await
            .unwrap();
        let admin = identity::bootstrap_admin(&database, "alice", "correct-horse".to_owned())
            .await
            .unwrap();
        let now = Utc::now();
        let team = organization::ActiveModel {
            id: Set(Uuid::new_v4()),
            slug: Set("team".to_owned()),
            display_name: Set("Platform Crew".to_owned()),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        namespace::ActiveModel {
            slug: Set("team".to_owned()),
            kind: Set("organization".to_owned()),
            user_id: Set(None),
            organization_id: Set(Some(team.id)),
            created_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        let mut repositories = HashMap::new();
        for (owner, name, deleted) in [
            ("alice", "alpha", false),
            ("team", "beta", false),
            ("alice", "gamma", false),
            ("alice", "deleted", true),
        ] {
            let model = repository::ActiveModel {
                id: Set(Uuid::new_v4()),
                namespace: Set(owner.to_owned()),
                name: Set(name.to_owned()),
                description: Set(None),
                website_url: Set(None),
                visibility: Set("private".to_owned()),
                object_format: Set("sha1".to_owned()),
                mirrored: Set(false),
                default_branch: Set(Some("main".to_owned())),
                issue_counter: Set(0),
                storage_key: Set(Uuid::new_v4()),
                created_by: Set(admin.id),
                archived_at: Set(None),
                deleted_at: Set(deleted.then_some(now)),
                icon_updated_at: Set(None),
                icon_source: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&database)
            .await
            .unwrap();
            repositories.insert(name, model);
        }
        let (maintenance, _) = tokio::sync::mpsc::channel(1);
        let state =
            IdentityState::new_with_runtime(database, settings.clone(), maintenance).unwrap();
        state
            .initialize_lfs_storage(&settings.storage)
            .await
            .unwrap();
        state
            .initialize_registry_storage(&settings.storage)
            .await
            .unwrap();
        Self {
            root,
            state,
            admin_id: admin.id,
            repositories,
        }
    }

    /// A fixture whose repositories hold `domain` data of the documented sizes.
    async fn with_usage(domain: &str) -> Self {
        let fixture = Self::new().await;
        for (name, sizes) in [
            ("alpha", &[100][..]),
            ("beta", &[120, 180][..]),
            ("gamma", &[200][..]),
            ("deleted", &[50][..]),
        ] {
            for (index, size) in sizes.iter().enumerate() {
                fixture.store(domain, name, index, *size).await;
            }
        }
        fixture
    }

    async fn domain(&self, name: &str) -> Domain {
        Domain::find(&self.state, name).await.unwrap()
    }

    /// Records one object of `size` bytes for repository `name`.
    async fn store(&self, domain: &str, name: &str, index: usize, size: usize) {
        let repository = &self.repositories[name];
        let payload = format!("{name}-{index}")
            .into_bytes()
            .into_iter()
            .cycle()
            .take(size)
            .collect::<Vec<_>>();
        let digest = BlobDigest::from_bytes(Sha256::digest(&payload).into());
        let hex = digest.to_hex();
        match domain {
            // LFS usage is read from the catalog.
            "lfs" => {
                lfs_object::ActiveModel {
                    repository_id: Set(repository.id),
                    oid: Set(hex),
                    size: Set(i64::try_from(size).unwrap()),
                    storage_target_id: Set(None),
                    created_at: Set(Utc::now()),
                }
                .insert(self.state.database())
                .await
                .unwrap();
            }
            // Registry usage is measured from the stored payloads.
            "registry" => {
                let image = hex::encode(Sha256::digest(b""));
                let key = ObjectKey::new(format!(
                    "registry/{}/{image}/blobs/{}/{}/{hex}",
                    repository.storage_key,
                    &hex[..2],
                    &hex[2..4]
                ))
                .unwrap();
                self.domain("registry")
                    .await
                    .storage
                    .store()
                    .put_verified(&key, digest, Box::pin(std::io::Cursor::new(payload)))
                    .await
                    .unwrap();
            }
            other => panic!("no fixture layout for {other}"),
        }
    }

    async fn page(&self, domain: &str, query: RepositoryUsageQuery) -> RepositoryUsagePage {
        repositories(&self.state, &self.domain(domain).await, &query)
            .await
            .unwrap()
    }

    async fn names(&self, domain: &str, query: RepositoryUsageQuery) -> Vec<String> {
        self.page(domain, query)
            .await
            .repositories
            .into_iter()
            .map(|row| row.repository_name)
            .collect()
    }

    async fn close(self) {
        self.state.database().clone().close().await.unwrap();
        tokio::fs::remove_dir_all(self.root).await.unwrap();
    }
}

fn status_of<T>(result: Result<T, ApiError>) -> StatusCode {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(error) => error.status,
    }
}

#[test]
fn unknown_domain_is_not_found() {
    assert_eq!(status_of(entry("artifacts")), StatusCode::NOT_FOUND);
}

#[test]
fn every_catalog_entry_names_its_storage_domain() {
    assert_eq!(
        DOMAINS.iter().map(|entry| entry.name).collect::<Vec<_>>(),
        [
            crate::storage::LfsDomain.name(),
            crate::registry::storage::RegistryDomain.name()
        ]
    );
}

#[test]
fn migration_request_requires_exactly_one_destination() {
    let request =
        |body: serde_json::Value| serde_json::from_value::<MigrationRequest>(body).unwrap();
    let target = Uuid::new_v4();
    assert_eq!(
        request(serde_json::json!({ "target_id": target }))
            .destination()
            .unwrap(),
        target
    );
    assert!(
        request(serde_json::json!({ "local": true }))
            .destination()
            .unwrap()
            .is_nil()
    );
    assert!(
        request(serde_json::json!({ "target_id": Uuid::nil() }))
            .destination()
            .unwrap()
            .is_nil()
    );
    assert_eq!(
        status_of(request(serde_json::json!({})).destination()),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_of(request(serde_json::json!({ "target_id": target, "local": true })).destination()),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn legacy_responses_flatten_domain_details() {
    let mut value = serde_json::json!({ "total_bytes": 1, "details": { "tag_count": 2 } });
    legacy::flatten_details(&mut value);
    assert_eq!(
        value,
        serde_json::json!({ "total_bytes": 1, "tag_count": 2 })
    );
}

#[tokio::test]
async fn status_reports_local_storage_and_totals() {
    for name in DOMAIN_NAMES {
        let fixture = Fixture::with_usage(name).await;
        let status = status(&fixture.state, &fixture.domain(name).await)
            .await
            .unwrap();
        assert_eq!(
            (
                status.name,
                status.active_target_id,
                status.usage.repository_count,
                status.usage.usage.object_count,
                status.usage.usage.total_bytes,
                status.active_migration.is_none(),
            ),
            (name, None, 4, 5, 650, true),
            "{name}"
        );
        assert!(!status.local_root.is_empty(), "{name}");
        fixture.close().await;
    }
}

#[tokio::test]
async fn empty_status_reports_every_detail_counter() {
    let fixture = Fixture::new().await;
    let status = status(&fixture.state, &fixture.domain("registry").await)
        .await
        .unwrap();
    assert_eq!(
        status
            .usage
            .usage
            .details
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        {
            let mut keys = status
                .detail_fields
                .iter()
                .map(|field| field.key)
                .collect::<Vec<_>>();
            keys.sort_unstable();
            keys
        }
    );
    fixture.close().await;
}

#[tokio::test]
async fn registry_status_reports_domain_details() {
    let fixture = Fixture::with_usage("registry").await;
    let status = status(&fixture.state, &fixture.domain("registry").await)
        .await
        .unwrap();
    assert_eq!(status.usage.usage.details.get("blob_count"), Some(&5));
    assert!(
        status
            .detail_fields
            .iter()
            .any(|field| field.key == "tag_count")
    );
    fixture.close().await;
}

#[tokio::test]
async fn repositories_exclude_deleted_and_sort_by_size() {
    for name in DOMAIN_NAMES {
        let fixture = Fixture::with_usage(name).await;
        let page = fixture.page(name, RepositoryUsageQuery::default()).await;
        assert_eq!(
            (
                page.total,
                page.limit,
                page.repositories
                    .iter()
                    .map(|row| (row.repository_name.as_str(), row.usage.total_bytes))
                    .collect::<Vec<_>>()
            ),
            (3, 10, vec![("beta", 300), ("gamma", 200), ("alpha", 100)]),
            "{name}"
        );
        fixture.close().await;
    }
}

#[tokio::test]
async fn repositories_apply_every_filter() {
    for name in DOMAIN_NAMES {
        let fixture = Fixture::with_usage(name).await;
        let cases: [(RepositoryUsageQuery, &[&str]); 7] = [
            (
                RepositoryUsageQuery {
                    search: Some(" AMM ".to_owned()),
                    ..Default::default()
                },
                &["gamma"],
            ),
            (
                // Organizations also match by display name.
                RepositoryUsageQuery {
                    owner: Some("crew".to_owned()),
                    ..Default::default()
                },
                &["beta"],
            ),
            (
                RepositoryUsageQuery {
                    owner_type: Some("user".to_owned()),
                    ..Default::default()
                },
                &["gamma", "alpha"],
            ),
            (
                RepositoryUsageQuery {
                    min_bytes: Some(150),
                    max_bytes: Some(250),
                    ..Default::default()
                },
                &["gamma"],
            ),
            (
                RepositoryUsageQuery {
                    sort: Some("bytes_asc".to_owned()),
                    ..Default::default()
                },
                &["alpha", "gamma", "beta"],
            ),
            (
                RepositoryUsageQuery {
                    sort: Some("name".to_owned()),
                    ..Default::default()
                },
                &["alpha", "beta", "gamma"],
            ),
            (
                RepositoryUsageQuery {
                    sort: Some("name".to_owned()),
                    limit: Some(1),
                    offset: Some(1),
                    ..Default::default()
                },
                &["beta"],
            ),
        ];
        for (query, expected) in cases {
            let description = format!("{name}: {query:?}");
            assert_eq!(fixture.names(name, query).await, expected, "{description}");
        }
        fixture.close().await;
    }
}

#[tokio::test]
async fn repositories_reject_invalid_queries() {
    let fixture = Fixture::new().await;
    for name in DOMAIN_NAMES {
        let domain = fixture.domain(name).await;
        for query in [
            RepositoryUsageQuery {
                limit: Some(0),
                ..Default::default()
            },
            RepositoryUsageQuery {
                limit: Some(101),
                ..Default::default()
            },
            RepositoryUsageQuery {
                min_bytes: Some(2),
                max_bytes: Some(1),
                ..Default::default()
            },
            RepositoryUsageQuery {
                owner_type: Some("team".to_owned()),
                ..Default::default()
            },
            RepositoryUsageQuery {
                sort: Some("owner".to_owned()),
                ..Default::default()
            },
        ] {
            let description = format!("{name}: {query:?}");
            assert_eq!(
                status_of(repositories(&fixture.state, &domain, &query).await),
                StatusCode::BAD_REQUEST,
                "{description}"
            );
        }
    }
    fixture.close().await;
}

#[tokio::test]
async fn migration_starts_reports_progress_and_selects_the_target() {
    for name in DOMAIN_NAMES {
        let fixture = Fixture::with_usage(name).await;
        let domain = fixture.domain(name).await;
        let target = targets::create(
            fixture.state.database(),
            format!("{name} target"),
            StorageTargetConfiguration::Filesystem {
                path: fixture.root.join(format!("target-{name}")),
            },
        )
        .await
        .unwrap();
        let request = MigrationRequest {
            target_id: Some(target.id),
            local: false,
            batch_size: 1,
        };

        let started = start_migration(&fixture.state, &domain, fixture.admin_id, &request)
            .await
            .unwrap();
        domain.storage.wait_for_migration().await;

        let view = find_migration(fixture.state.database(), domain.entry, started.operation_id)
            .await
            .unwrap();
        assert_eq!(
            (
                view.domain,
                view.operation.as_str(),
                view.state.as_str(),
                view.phase.as_ref(),
                view.target_id,
            ),
            (
                name,
                format!("{name}_migrate").as_str(),
                state::COMPLETED,
                "completed",
                Some(target.id)
            ),
            "{name}"
        );
        let status = status(&fixture.state, &domain).await.unwrap();
        assert_eq!(
            (
                status.active_target_id,
                status.active_target_name.as_deref(),
                status
                    .last_migration
                    .map(|migration| migration.operation_id),
            ),
            (
                Some(target.id),
                Some(format!("{name} target").as_str()),
                Some(started.operation_id)
            ),
            "{name}"
        );
        // The migration is invisible under every other domain.
        let other = DOMAINS.iter().find(|entry| entry.name != name).unwrap();
        assert_eq!(
            status_of(find_migration(fixture.state.database(), other, started.operation_id).await),
            StatusCode::NOT_FOUND,
            "{name}"
        );
        fixture.close().await;
    }
}

#[tokio::test]
async fn migration_rejects_unknown_and_active_targets() {
    let fixture = Fixture::new().await;
    for name in DOMAIN_NAMES {
        let domain = fixture.domain(name).await;
        for request in [
            MigrationRequest {
                target_id: Some(Uuid::new_v4()),
                local: false,
                batch_size: 100,
            },
            // The local root is already active.
            MigrationRequest {
                target_id: None,
                local: true,
                batch_size: 100,
            },
            MigrationRequest {
                target_id: None,
                local: true,
                batch_size: 0,
            },
        ] {
            let description = format!("{name}: {request:?}");
            assert_eq!(
                status_of(
                    start_migration(&fixture.state, &domain, fixture.admin_id, &request).await
                ),
                StatusCode::BAD_REQUEST,
                "{description}"
            );
        }
    }
    fixture.close().await;
}
