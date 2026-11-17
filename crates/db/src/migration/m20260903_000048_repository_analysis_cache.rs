use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"CREATE TABLE repository_cache_entries (
                    repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                    kind TEXT NOT NULL CHECK (kind IN ('overview', 'language_stats', 'commit_count', 'size')),
                    cache_key TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (repository_id, kind, cache_key)
                )"#,
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX idx_repository_cache_expiry ON repository_cache_entries(expires_at)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE repository_cache_entries")
            .await?;
        Ok(())
    }
}
