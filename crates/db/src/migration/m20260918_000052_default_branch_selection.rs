use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const REPOSITORY_COLUMNS: &str = "id, namespace, name, description, website_url, visibility, object_format, mirrored, default_branch, issue_counter, storage_key, created_by, archived_at, deleted_at, icon_updated_at, icon_source, created_at, updated_at";

async fn rebuild_sqlite_repositories(
    manager: &SchemaManager<'_>,
    old_table: &str,
    default_branch_definition: &str,
    drop_marker: bool,
) -> Result<(), DbErr> {
    let database = manager.get_connection();
    database
        .execute_unprepared("PRAGMA foreign_keys = OFF")
        .await?;
    if let Err(error) = database
        .execute_unprepared("PRAGMA legacy_alter_table = ON")
        .await
    {
        let _ = database
            .execute_unprepared("PRAGMA foreign_keys = ON")
            .await;
        return Err(error);
    }
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
        if drop_marker {
            database
                .execute_unprepared("DROP TABLE repository_default_branch_backfills")
                .await?;
        }
        database
            .execute_unprepared(&format!("ALTER TABLE repositories RENAME TO {old_table}"))
            .await?;
        database
            .execute_unprepared(&format!(
                "CREATE TABLE repositories (
                     id BLOB NOT NULL PRIMARY KEY,
                     namespace TEXT NOT NULL,
                     name TEXT NOT NULL,
                     description TEXT,
                     website_url TEXT,
                     visibility TEXT NOT NULL,
                     object_format TEXT NOT NULL,
                     mirrored BOOLEAN NOT NULL DEFAULT FALSE,
                     default_branch {default_branch_definition},
                     issue_counter BIGINT NOT NULL DEFAULT 0,
                     storage_key BLOB NOT NULL UNIQUE,
                     created_by BLOB NOT NULL,
                     archived_at TEXT,
                     deleted_at TEXT,
                     icon_updated_at TEXT,
                     icon_source TEXT,
                     created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL,
                     FOREIGN KEY (namespace) REFERENCES namespaces (slug) ON DELETE CASCADE,
                     FOREIGN KEY (created_by) REFERENCES users (id) ON DELETE RESTRICT
                 )"
            ))
            .await?;
        database
            .execute_unprepared(&format!(
                "INSERT INTO repositories ({REPOSITORY_COLUMNS}) SELECT {REPOSITORY_COLUMNS} FROM {old_table}"
            ))
            .await?;
        database
            .execute_unprepared(&format!("DROP TABLE {old_table}"))
            .await?;
        database
            .execute_unprepared(
                "CREATE UNIQUE INDEX \"uq-repository-namespace-name\" ON repositories (namespace, name)",
            )
            .await?;
        if !drop_marker {
            database
                .execute_unprepared(
                    "CREATE TABLE repository_default_branch_backfills (
                         repository_id BLOB NOT NULL PRIMARY KEY
                             REFERENCES repositories (id) ON DELETE CASCADE
                     )",
                )
                .await?;
            database
                .execute_unprepared(
                    "INSERT INTO repository_default_branch_backfills SELECT id FROM repositories",
                )
                .await?;
        }
        Ok::<(), DbErr>(())
    };

    let transaction_result = match migration.await {
        Ok(()) => match database
            .query_all_raw(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA foreign_key_check",
            ))
            .await
        {
            Ok(violations) if violations.is_empty() => database.execute_unprepared("COMMIT").await,
            Ok(violations) => {
                let _ = database.execute_unprepared("ROLLBACK").await;
                Err(DbErr::Custom(format!(
                    "repository migration left {} foreign-key violations",
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
    let restore_legacy = database
        .execute_unprepared("PRAGMA legacy_alter_table = OFF")
        .await;
    let restore_foreign_keys = database
        .execute_unprepared("PRAGMA foreign_keys = ON")
        .await;
    transaction_result?;
    restore_legacy?;
    restore_foreign_keys?;
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() == DbBackend::Sqlite {
            rebuild_sqlite_repositories(manager, "repositories_default_branch_old", "TEXT", false)
                .await
        } else {
            manager
                .alter_table(
                    Table::alter()
                        .table(Repository::Table)
                        .modify_column(
                            ColumnDef::new(Repository::DefaultBranch)
                                .string_len(255)
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
            manager
                .create_table(
                    Table::create()
                        .table(Backfill::Table)
                        .if_not_exists()
                        .col(
                            ColumnDef::new(Backfill::RepositoryId)
                                .uuid()
                                .not_null()
                                .primary_key(),
                        )
                        .foreign_key(
                            ForeignKey::create()
                                .from(Backfill::Table, Backfill::RepositoryId)
                                .to(Repository::Table, Repository::Id)
                                .on_delete(ForeignKeyAction::Cascade),
                        )
                        .to_owned(),
                )
                .await?;
            manager
                .get_connection()
                .execute_unprepared(
                    "INSERT INTO repository_default_branch_backfills SELECT id FROM repositories",
                )
                .await?;
            Ok(())
        }
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() == DbBackend::Sqlite {
            manager
                .get_connection()
                .execute_unprepared(
                    "UPDATE repositories SET default_branch = 'main' WHERE default_branch IS NULL",
                )
                .await?;
            rebuild_sqlite_repositories(
                manager,
                "repositories_default_branch_old",
                "TEXT NOT NULL",
                true,
            )
            .await
        } else {
            manager
                .drop_table(Table::drop().table(Backfill::Table).to_owned())
                .await?;
            manager
                .alter_table(
                    Table::alter()
                        .table(Repository::Table)
                        .modify_column(
                            ColumnDef::new(Repository::DefaultBranch)
                                .string_len(255)
                                .not_null(),
                        )
                        .to_owned(),
                )
                .await
        }
    }
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
    DefaultBranch,
}

#[derive(DeriveIden)]
enum Backfill {
    #[sea_orm(iden = "repository_default_branch_backfills")]
    Table,
    RepositoryId,
}
