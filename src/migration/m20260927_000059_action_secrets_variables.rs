use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let statements = [
            r#"CREATE TABLE action_secrets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repository_id TEXT REFERENCES repositories(id) ON DELETE CASCADE,
                namespace TEXT REFERENCES namespaces(slug) ON DELETE CASCADE,
                name TEXT NOT NULL,
                sealed BLOB NOT NULL,
                updated_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                CHECK ((repository_id IS NULL) <> (namespace IS NULL))
            )"#,
            r#"CREATE UNIQUE INDEX uq_action_secrets_repository
               ON action_secrets(repository_id, name) WHERE repository_id IS NOT NULL"#,
            r#"CREATE UNIQUE INDEX uq_action_secrets_namespace
               ON action_secrets(namespace, name) WHERE namespace IS NOT NULL"#,
            r#"CREATE TABLE action_variables (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repository_id TEXT REFERENCES repositories(id) ON DELETE CASCADE,
                namespace TEXT REFERENCES namespaces(slug) ON DELETE CASCADE,
                name TEXT NOT NULL,
                value TEXT NOT NULL,
                updated_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                CHECK ((repository_id IS NULL) <> (namespace IS NULL))
            )"#,
            r#"CREATE UNIQUE INDEX uq_action_variables_repository
               ON action_variables(repository_id, name) WHERE repository_id IS NOT NULL"#,
            r#"CREATE UNIQUE INDEX uq_action_variables_namespace
               ON action_variables(namespace, name) WHERE namespace IS NOT NULL"#,
        ];
        for statement in statements {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in ["action_variables", "action_secrets"] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
