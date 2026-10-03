use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The cache table admitted only the kinds it was created with, so the
/// per-day commit lists were filed under `overview` and per-commit line
/// stats under `commit_count`, told apart by a key prefix. Rebuild the table
/// with the kinds those features own and drop the rows written under the
/// borrowed kinds; they are recomputed on demand.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
DELETE FROM repository_cache_entries
WHERE (kind = 'overview' AND cache_key LIKE '%:day:%')
   OR (kind = 'commit_count' AND cache_key LIKE 'diff:%');
CREATE TABLE repository_cache_entries_rebuilt (
    repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN (
        'overview', 'language_stats', 'commit_count', 'size',
        'activity', 'day_commits', 'commit_stats'
    )),
    cache_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (repository_id, kind, cache_key)
);
INSERT INTO repository_cache_entries_rebuilt
SELECT repository_id, kind, cache_key, payload_json, expires_at, created_at
FROM repository_cache_entries;
DROP TABLE repository_cache_entries;
ALTER TABLE repository_cache_entries_rebuilt RENAME TO repository_cache_entries;
CREATE INDEX idx_repository_cache_expiry ON repository_cache_entries(expires_at);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
