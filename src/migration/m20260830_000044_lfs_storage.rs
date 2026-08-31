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
CREATE TABLE lfs_storage_targets (
    id uuid NOT NULL PRIMARY KEY,
    name text NOT NULL UNIQUE,
    kind text NOT NULL,
    configuration text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX idx_lfs_storage_target_kind ON lfs_storage_targets (kind);

CREATE TABLE lfs_storage_state (
    id integer NOT NULL PRIMARY KEY CHECK (id = 1),
    active_target_id uuid NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT fk_lfs_storage_state_active_target
        FOREIGN KEY (active_target_id) REFERENCES lfs_storage_targets (id) ON DELETE RESTRICT
);
INSERT INTO lfs_storage_state (id, active_target_id, updated_at)
VALUES (1, NULL, CURRENT_TIMESTAMP);

CREATE TABLE lfs_objects (
    repository_id uuid NOT NULL,
    oid text NOT NULL,
    size bigint NOT NULL CHECK (size >= 0),
    storage_target_id uuid NULL,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (repository_id, oid),
    CONSTRAINT fk_lfs_object_repository
        FOREIGN KEY (repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
    CONSTRAINT fk_lfs_object_storage_target
        FOREIGN KEY (storage_target_id) REFERENCES lfs_storage_targets (id) ON DELETE SET NULL
);
CREATE INDEX idx_lfs_object_target ON lfs_objects (storage_target_id);

CREATE TABLE lfs_storage_migrations (
    id uuid NOT NULL PRIMARY KEY,
    source_target_id uuid NULL,
    target_id uuid NULL,
    state text NOT NULL,
    phase text NOT NULL,
    last_key text NULL,
    copied_objects bigint NOT NULL DEFAULT 0,
    copied_bytes bigint NOT NULL DEFAULT 0,
    error text NULL,
    started_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    completed_at timestamptz NULL,
    CONSTRAINT fk_lfs_migration_source
        FOREIGN KEY (source_target_id) REFERENCES lfs_storage_targets (id) ON DELETE SET NULL,
    CONSTRAINT fk_lfs_migration_target
        FOREIGN KEY (target_id) REFERENCES lfs_storage_targets (id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX idx_lfs_storage_migration_active
ON lfs_storage_migrations ((1))
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
DROP TABLE lfs_storage_migrations;
DROP TABLE lfs_objects;
DROP TABLE lfs_storage_state;
DROP TABLE lfs_storage_targets;
"#,
            )
            .await?;
        Ok(())
    }
}
