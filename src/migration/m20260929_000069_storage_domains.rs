use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Storage targets are shared by every kind of stored data, so the per-domain
/// state and migration tables collapse into two tables keyed by domain. Rows
/// are copied so instances keep their selected targets and migration history;
/// phase names converge on one vocabulary.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
ALTER TABLE lfs_storage_targets RENAME TO storage_targets;
DROP INDEX IF EXISTS idx_lfs_storage_target_kind;
CREATE INDEX idx_storage_target_kind ON storage_targets (kind);

CREATE TABLE storage_domain_state (
    domain text NOT NULL PRIMARY KEY,
    active_target_id uuid NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT fk_storage_domain_state_target
        FOREIGN KEY (active_target_id) REFERENCES storage_targets (id) ON DELETE RESTRICT
);
INSERT INTO storage_domain_state (domain, active_target_id, updated_at)
SELECT 'lfs', active_target_id, updated_at FROM lfs_storage_state WHERE id = 1;
INSERT INTO storage_domain_state (domain, active_target_id, updated_at)
SELECT 'registry', active_target_id, updated_at FROM registry_storage_state WHERE id = 1;
INSERT INTO storage_domain_state (domain, active_target_id, updated_at)
SELECT 'lfs', NULL, CURRENT_TIMESTAMP
WHERE NOT EXISTS (SELECT 1 FROM storage_domain_state WHERE domain = 'lfs');
INSERT INTO storage_domain_state (domain, active_target_id, updated_at)
SELECT 'registry', NULL, CURRENT_TIMESTAMP
WHERE NOT EXISTS (SELECT 1 FROM storage_domain_state WHERE domain = 'registry');

CREATE TABLE storage_migrations (
    id uuid NOT NULL PRIMARY KEY,
    domain text NOT NULL,
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
    CONSTRAINT fk_storage_migration_source
        FOREIGN KEY (source_target_id) REFERENCES storage_targets (id) ON DELETE SET NULL,
    CONSTRAINT fk_storage_migration_target
        FOREIGN KEY (target_id) REFERENCES storage_targets (id) ON DELETE SET NULL
);
CREATE INDEX idx_storage_migration_domain ON storage_migrations (domain, started_at);
CREATE UNIQUE INDEX idx_storage_migration_active
ON storage_migrations (domain)
WHERE state IN ('pending', 'copying', 'verifying', 'cutover');

INSERT INTO storage_migrations (
    id, domain, source_target_id, target_id, state, phase, last_key,
    copied_objects, copied_bytes, total_bytes, error, started_at, updated_at, completed_at
)
SELECT id, 'lfs', source_target_id, target_id, state,
       CASE phase
           WHEN 'pending' THEN 'scheduled'
           WHEN 'check' THEN 'checking_destination'
           WHEN 'copy' THEN 'copying'
           WHEN 'verify' THEN 'verifying'
           WHEN 'complete' THEN 'completed'
           ELSE phase
       END,
       last_key, copied_objects, copied_bytes, NULL, error, started_at, updated_at, completed_at
FROM lfs_storage_migrations;

INSERT INTO storage_migrations (
    id, domain, source_target_id, target_id, state, phase, last_key,
    copied_objects, copied_bytes, total_bytes, error, started_at, updated_at, completed_at
)
SELECT id, 'registry', source_target_id, target_id, state,
       CASE phase
           WHEN 'copying_registry' THEN 'copying'
           WHEN 'writing_metadata' THEN 'cutover'
           ELSE phase
       END,
       last_key, copied_objects, copied_bytes, total_bytes, error, started_at, updated_at, completed_at
FROM registry_storage_migrations;

DROP TABLE lfs_storage_migrations;
DROP TABLE lfs_storage_state;
DROP TABLE registry_storage_migrations;
DROP TABLE registry_storage_state;
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
ALTER TABLE storage_targets RENAME TO lfs_storage_targets;
DROP INDEX IF EXISTS idx_storage_target_kind;
CREATE INDEX idx_lfs_storage_target_kind ON lfs_storage_targets (kind);

CREATE TABLE lfs_storage_state (
    id integer NOT NULL PRIMARY KEY CHECK (id = 1),
    active_target_id uuid NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT fk_lfs_storage_state_active_target
        FOREIGN KEY (active_target_id) REFERENCES lfs_storage_targets (id) ON DELETE RESTRICT
);
INSERT INTO lfs_storage_state (id, active_target_id, updated_at)
SELECT 1, active_target_id, updated_at FROM storage_domain_state WHERE domain = 'lfs';

CREATE TABLE registry_storage_state (
    id integer NOT NULL PRIMARY KEY CHECK (id = 1),
    active_target_id uuid NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT fk_registry_storage_state_target
        FOREIGN KEY (active_target_id) REFERENCES lfs_storage_targets (id) ON DELETE RESTRICT
);
INSERT INTO registry_storage_state (id, active_target_id, updated_at)
SELECT 1, active_target_id, updated_at FROM storage_domain_state WHERE domain = 'registry';

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
INSERT INTO lfs_storage_migrations
SELECT id, source_target_id, target_id, state,
       CASE phase
           WHEN 'scheduled' THEN 'pending'
           WHEN 'checking_destination' THEN 'check'
           WHEN 'copying' THEN 'copy'
           WHEN 'verifying' THEN 'verify'
           WHEN 'completed' THEN 'complete'
           ELSE phase
       END,
       last_key, copied_objects, copied_bytes, error, started_at, updated_at, completed_at
FROM storage_migrations WHERE domain = 'lfs';

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
INSERT INTO registry_storage_migrations
SELECT id, source_target_id, target_id, state,
       CASE phase
           WHEN 'copying' THEN 'copying_registry'
           WHEN 'verifying' THEN 'copying_registry'
           WHEN 'cutover' THEN 'writing_metadata'
           ELSE phase
       END,
       last_key, copied_objects, copied_bytes, total_bytes, error, started_at, updated_at, completed_at
FROM storage_migrations WHERE domain = 'registry';

DROP TABLE storage_migrations;
DROP TABLE storage_domain_state;
"#,
            )
            .await?;
        Ok(())
    }
}
