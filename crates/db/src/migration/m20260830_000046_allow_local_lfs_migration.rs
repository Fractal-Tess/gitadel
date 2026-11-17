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
CREATE TABLE lfs_storage_migrations_rebuilt (
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
INSERT INTO lfs_storage_migrations_rebuilt
SELECT id, source_target_id, target_id, state, phase, last_key,
       copied_objects, copied_bytes, error, started_at, updated_at, completed_at
FROM lfs_storage_migrations;
DROP TABLE lfs_storage_migrations;
ALTER TABLE lfs_storage_migrations_rebuilt RENAME TO lfs_storage_migrations;
CREATE UNIQUE INDEX idx_lfs_storage_migration_active
ON lfs_storage_migrations ((1))
WHERE state IN ('pending', 'copying', 'verifying', 'cutover');
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
