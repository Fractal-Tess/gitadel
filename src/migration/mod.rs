mod m20260820_000001_create_instance;
mod m20260820_000002_create_identity;
mod m20260820_000003_create_repositories;
mod m20260820_000004_create_repository_favorites;
mod m20260820_000005_add_instance_settings;

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
        ]
    }
}
