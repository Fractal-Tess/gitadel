use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rebuild(manager, true).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rebuild(manager, false).await
    }
}

async fn rebuild(manager: &SchemaManager<'_>, allow_system: bool) -> Result<(), DbErr> {
    let database = manager.get_connection();
    database
        .execute_unprepared("PRAGMA foreign_keys = OFF")
        .await?;
    database
        .execute_unprepared("PRAGMA legacy_alter_table = ON")
        .await?;
    if let Err(error) = database.execute_unprepared("BEGIN IMMEDIATE").await {
        restore_pragmas(database).await;
        return Err(error);
    }

    let migration = async {
        if !allow_system {
            database
                .execute_unprepared(
                    "DELETE FROM action_runner_registration_tokens WHERE namespace IS NULL",
                )
                .await?;
            database
                .execute_unprepared("DELETE FROM action_runners WHERE namespace IS NULL")
                .await?;
        }

        database
            .execute_unprepared("ALTER TABLE action_runners RENAME TO action_runners_scoped")
            .await?;
        database
            .execute_unprepared(&runner_table_sql(allow_system))
            .await?;
        database
            .execute_unprepared(
                r#"INSERT INTO action_runners (
                    id, uuid, namespace, name, token_hash, approved_labels, version,
                    ephemeral, disabled_at, deleted_at, last_seen_at, created_by, created_at
                )
                SELECT id, uuid, namespace, name, token_hash, approved_labels, version,
                       ephemeral, disabled_at, deleted_at, last_seen_at, created_by, created_at
                FROM action_runners_scoped"#,
            )
            .await?;
        database
            .execute_unprepared("DROP TABLE action_runners_scoped")
            .await?;
        database
            .execute_unprepared(
                r#"CREATE UNIQUE INDEX uq_action_runners_active_namespace_name
                   ON action_runners(namespace, name)
                   WHERE namespace IS NOT NULL AND deleted_at IS NULL AND disabled_at IS NULL"#,
            )
            .await?;
        if allow_system {
            database
                .execute_unprepared(
                    r#"CREATE UNIQUE INDEX uq_action_runners_active_system_name
                       ON action_runners(name)
                       WHERE namespace IS NULL AND deleted_at IS NULL AND disabled_at IS NULL"#,
                )
                .await?;
        }

        database
            .execute_unprepared(
                "ALTER TABLE action_runner_registration_tokens \
                 RENAME TO action_runner_registration_tokens_scoped",
            )
            .await?;
        database
            .execute_unprepared(&registration_table_sql(allow_system))
            .await?;
        database
            .execute_unprepared(
                r#"INSERT INTO action_runner_registration_tokens (
                    id, token_hash, namespace, runner_name, approved_labels, expires_at,
                    used_at, created_by, created_at
                )
                SELECT id, token_hash, namespace, runner_name, approved_labels, expires_at,
                       used_at, created_by, created_at
                FROM action_runner_registration_tokens_scoped"#,
            )
            .await?;
        database
            .execute_unprepared("DROP TABLE action_runner_registration_tokens_scoped")
            .await?;
        database
            .execute_unprepared(
                "CREATE INDEX idx_action_registration_expiry \
                 ON action_runner_registration_tokens(expires_at, used_at)",
            )
            .await?;

        Ok::<(), DbErr>(())
    }
    .await;

    let result = match migration {
        Ok(()) => match database
            .query_all_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT * FROM pragma_foreign_key_check WHERE \"table\" IN (\
                 'action_runners', 'action_runner_registration_tokens', \
                 'action_jobs', 'action_runner_fetches')",
            ))
            .await
        {
            Ok(violations) if violations.is_empty() => {
                database.execute_unprepared("COMMIT").await.map(|_| ())
            }
            Ok(_) => {
                let _ = database.execute_unprepared("ROLLBACK").await;
                Err(DbErr::Migration(
                    "runner scope migration violated foreign keys".to_owned(),
                ))
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
    restore_pragmas(database).await;
    result
}

fn runner_table_sql(allow_system: bool) -> String {
    let namespace = if allow_system {
        "namespace TEXT REFERENCES namespaces(slug) ON DELETE CASCADE"
    } else {
        "namespace TEXT NOT NULL REFERENCES namespaces(slug) ON DELETE CASCADE"
    };
    format!(
        r#"CREATE TABLE action_runners (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            uuid TEXT NOT NULL UNIQUE,
            {namespace},
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
        )"#
    )
}

fn registration_table_sql(allow_system: bool) -> String {
    let namespace = if allow_system {
        "namespace TEXT REFERENCES namespaces(slug) ON DELETE CASCADE"
    } else {
        "namespace TEXT NOT NULL REFERENCES namespaces(slug) ON DELETE CASCADE"
    };
    format!(
        r#"CREATE TABLE action_runner_registration_tokens (
            id TEXT PRIMARY KEY NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            {namespace},
            runner_name TEXT NOT NULL,
            approved_labels TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            used_at TEXT,
            created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL
        )"#
    )
}

async fn restore_pragmas(database: &impl ConnectionTrait) {
    let _ = database
        .execute_unprepared("PRAGMA legacy_alter_table = OFF")
        .await;
    let _ = database
        .execute_unprepared("PRAGMA foreign_keys = ON")
        .await;
}
