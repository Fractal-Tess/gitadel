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
                UPDATE repositories
                SET icon_source = CASE icon_source
                    WHEN 'manual' THEN 'uploaded'
                    WHEN 'detected' THEN 'automatic'
                    ELSE icon_source
                END
                WHERE icon_source IN ('manual', 'detected')
                "#,
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE IF NOT EXISTS repository_icon_candidates (
                    repository_id BLOB NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                    path TEXT NOT NULL,
                    commit_oid TEXT NOT NULL,
                    oid TEXT NOT NULL,
                    width INTEGER NOT NULL,
                    height INTEGER NOT NULL,
                    mime_type TEXT NOT NULL,
                    reasons_json TEXT NOT NULL,
                    recommended BOOLEAN NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (repository_id, path)
                )
                "#,
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX IF NOT EXISTS idx_repository_icon_candidates_commit
                ON repository_icon_candidates(repository_id, commit_oid)
                "#,
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE IF NOT EXISTS repository_icon_scans (
                    repository_id BLOB NOT NULL PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,
                    commit_oid TEXT,
                    selected_path TEXT,
                    selected_missing BOOLEAN NOT NULL DEFAULT 0,
                    status TEXT NOT NULL,
                    version INTEGER NOT NULL DEFAULT 0,
                    detector_version INTEGER NOT NULL DEFAULT 1,
                    error TEXT,
                    updated_at TEXT NOT NULL
                )
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE repositories SET icon_source = CASE icon_source \
             WHEN 'automatic' THEN 'detected' \
             WHEN 'uploaded' THEN 'manual' \
             WHEN 'selected' THEN 'manual' \
             WHEN 'none' THEN 'manual' ELSE icon_source END",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS repository_icon_scans")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS repository_icon_candidates")
            .await?;
        Ok(())
    }
}
