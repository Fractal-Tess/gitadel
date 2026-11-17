use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let database = manager.get_connection();
        database
            .execute_unprepared("PRAGMA foreign_keys = OFF")
            .await?;
        database
            .execute_unprepared("PRAGMA legacy_alter_table = ON")
            .await?;
        if let Err(error) = database.execute_unprepared("BEGIN IMMEDIATE").await {
            let _ = database
                .execute_unprepared("PRAGMA legacy_alter_table = OFF")
                .await;
            let _ = database
                .execute_unprepared("PRAGMA foreign_keys = ON")
                .await;
            return Err(error);
        }
        let migration = async {
            database
                .execute_unprepared(
                    "ALTER TABLE action_runners RENAME TO action_runners_repository_owned",
                )
                .await?;
            database
                .execute_unprepared(
                    r#"CREATE TABLE action_runners (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        uuid TEXT NOT NULL UNIQUE,
                        namespace TEXT NOT NULL REFERENCES namespaces(slug) ON DELETE CASCADE,
                        name TEXT NOT NULL,
                        token_hash TEXT NOT NULL UNIQUE,
                        approved_labels TEXT NOT NULL,
                        version TEXT NOT NULL,
                        ephemeral INTEGER NOT NULL DEFAULT 0 CHECK (ephemeral = 0),
                        disabled_at TEXT,
                        deleted_at TEXT,
                        last_seen_at TEXT,
                        created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                        created_at TEXT NOT NULL
                    )"#,
                )
                .await?;
            database
                .execute_unprepared(
                    r#"INSERT INTO action_runners (
                        id, uuid, namespace, name, token_hash, approved_labels, version,
                        ephemeral, disabled_at, deleted_at, last_seen_at, created_by, created_at
                    )
                    SELECT runner.id, runner.uuid, repository.namespace, runner.name,
                           runner.token_hash, runner.approved_labels, runner.version,
                           runner.ephemeral, runner.disabled_at, runner.deleted_at,
                           runner.last_seen_at, runner.created_by, runner.created_at
                    FROM action_runners_repository_owned runner
                    JOIN repositories repository ON repository.id = runner.repository_id"#,
                )
                .await?;
            database
                .execute_unprepared(
                    r#"UPDATE action_runners AS runner
                       SET name = substr(runner.name, 1, 54) || '-migrated-' || runner.uuid
                       WHERE runner.deleted_at IS NULL
                         AND runner.disabled_at IS NULL
                         AND EXISTS (
                             SELECT 1
                             FROM action_runners AS duplicate
                             WHERE duplicate.namespace = runner.namespace
                               AND duplicate.name = runner.name
                               AND duplicate.deleted_at IS NULL
                               AND duplicate.disabled_at IS NULL
                               AND duplicate.id <> runner.id
                         )"#,
                )
                .await?;
            database
                .execute_unprepared("DROP TABLE action_runners_repository_owned")
                .await?;
            database
                .execute_unprepared(
                    r#"CREATE UNIQUE INDEX uq_action_runners_active_namespace_name
                       ON action_runners(namespace, name)
                       WHERE deleted_at IS NULL AND disabled_at IS NULL"#,
                )
                .await?;

            database
                .execute_unprepared(
                    "ALTER TABLE action_runner_registration_tokens \
                     RENAME TO action_runner_registration_tokens_repository_owned",
                )
                .await?;
            database
                .execute_unprepared(
                    r#"CREATE TABLE action_runner_registration_tokens (
                        id TEXT PRIMARY KEY NOT NULL,
                        token_hash TEXT NOT NULL UNIQUE,
                        namespace TEXT NOT NULL REFERENCES namespaces(slug) ON DELETE CASCADE,
                        runner_name TEXT NOT NULL,
                        approved_labels TEXT NOT NULL,
                        expires_at TEXT NOT NULL,
                        used_at TEXT,
                        created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                        created_at TEXT NOT NULL
                    )"#,
                )
                .await?;
            database
                .execute_unprepared(
                    r#"INSERT INTO action_runner_registration_tokens (
                        id, token_hash, namespace, runner_name, approved_labels, expires_at,
                        used_at, created_by, created_at
                    )
                    SELECT grant_row.id, grant_row.token_hash, repository.namespace,
                           grant_row.runner_name, grant_row.approved_labels, grant_row.expires_at,
                           grant_row.used_at, grant_row.created_by, grant_row.created_at
                    FROM action_runner_registration_tokens_repository_owned grant_row
                    JOIN repositories repository ON repository.id = grant_row.repository_id"#,
                )
                .await?;
            database
                .execute_unprepared("DROP TABLE action_runner_registration_tokens_repository_owned")
                .await?;
            database
                .execute_unprepared(
                    r#"CREATE INDEX idx_action_registration_expiry
                       ON action_runner_registration_tokens(expires_at, used_at)"#,
                )
                .await?;
            database
                .execute_unprepared("DROP TABLE action_repository_settings")
                .await?;

            Ok::<(), DbErr>(())
        };

        let transaction_result = match migration.await {
            Ok(()) => match database
                .query_all_raw(Statement::from_string(
                    DbBackend::Sqlite,
                    "SELECT * FROM pragma_foreign_key_check \
                     WHERE \"table\" IN (\
                         'action_runners', \
                         'action_runner_registration_tokens', \
                         'action_jobs', \
                         'action_runner_fetches'\
                     )",
                ))
                .await
            {
                Ok(violations) if violations.is_empty() => {
                    database.execute_unprepared("COMMIT").await
                }
                Ok(violations) => {
                    let _ = database.execute_unprepared("ROLLBACK").await;
                    Err(DbErr::Custom(format!(
                        "owner runner migration left {} Actions foreign-key violations",
                        violations.len()
                    )))
                }
                Err(error) => {
                    let _ = database.execute_unprepared("ROLLBACK").await;
                    Err(error)
                }
            },
            Err(error) => {
                let _ = database.execute_unprepared("ROLLBACK").await;
                Err(error)
            }
        };
        let restore = database
            .execute_unprepared("PRAGMA legacy_alter_table = OFF")
            .await;
        let restore_foreign_keys = database
            .execute_unprepared("PRAGMA foreign_keys = ON")
            .await;
        transaction_result?;
        restore?;
        restore_foreign_keys?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "owner-scoped Actions runners cannot be safely reverted to repository ownership"
                .to_owned(),
        ))
    }
}
