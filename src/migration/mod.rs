mod m20260820_000001_create_instance;
mod m20260820_000002_create_identity;
mod m20260820_000003_create_repositories;
mod m20260820_000004_create_repository_favorites;
mod m20260820_000005_add_instance_settings;
mod m20260820_000006_create_lfs_locks;
mod m20260822_000007_create_oauth_provider;
mod m20260822_000008_scope_oauth_repository_tokens;
mod m20260822_000009_create_repository_webhooks;
mod m20260822_000010_repository_control;
mod m20260822_000011_create_repository_topics;
mod m20260822_000012_create_instance_assets;
mod m20260822_000013_add_user_avatar;

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260820_000001_create_instance::Migration),
            Box::new(m20260820_000002_create_identity::Migration),
            Box::new(m20260820_000003_create_repositories::Migration),
            Box::new(m20260820_000004_create_repository_favorites::Migration),
            Box::new(m20260820_000005_add_instance_settings::Migration),
            Box::new(m20260820_000006_create_lfs_locks::Migration),
            Box::new(m20260822_000007_create_oauth_provider::Migration),
            Box::new(m20260822_000008_scope_oauth_repository_tokens::Migration),
            Box::new(m20260822_000009_create_repository_webhooks::Migration),
            Box::new(m20260822_000010_repository_control::Migration),
            Box::new(m20260822_000011_create_repository_topics::Migration),
            Box::new(m20260822_000012_create_instance_assets::Migration),
            Box::new(m20260822_000013_add_user_avatar::Migration),
        ]
    }
}
