use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
CREATE TABLE registry_storage_state (
    id integer NOT NULL PRIMARY KEY CHECK (id = 1),
    active_target_id uuid NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT fk_registry_storage_state_target
        FOREIGN KEY (active_target_id) REFERENCES lfs_storage_targets (id) ON DELETE RESTRICT
);
INSERT INTO registry_storage_state (id, active_target_id, updated_at)
VALUES (1, NULL, CURRENT_TIMESTAMP);

CREATE TABLE registry_storage_migrations (
    id uuid NOT NULL PRIMARY KEY,
    source_target_id uuid NULL,
    target_id uuid NULL,
    state text NOT NULL,
    phase text NOT NULL,
    last_key text NULL,
    copied_objects bigint NOT NULL DEFAULT 0,
    copied_bytes bigint NOT NULL DEFAULT 0,
    total_bytes bigint NULL,
    error text NULL,
    started_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    completed_at timestamptz NULL,
    CONSTRAINT fk_registry_migration_source
        FOREIGN KEY (source_target_id) REFERENCES lfs_storage_targets (id) ON DELETE SET NULL,
    CONSTRAINT fk_registry_migration_target
        FOREIGN KEY (target_id) REFERENCES lfs_storage_targets (id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX idx_registry_storage_migration_active
ON registry_storage_migrations ((1))
WHERE state IN ('pending', 'copying', 'verifying', 'cutover');
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
DROP TABLE registry_storage_migrations;
DROP TABLE registry_storage_state;
"#,
            )
            .await?;
        Ok(())
    }
}
