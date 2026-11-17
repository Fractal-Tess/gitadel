use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Commit emails must be verified before commits are attributed to them.
///
/// The table is rebuilt because SQLite cannot drop the `UNIQUE (email)`
/// constraint in place: an address now belongs to at most one account only
/// once verified, so an unverified claim by someone else cannot reserve it.
/// Rows that already exist predate the rule and are marked verified; they
/// were added while addresses took effect immediately.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let statements = [
            r#"CREATE TABLE user_commit_emails_next (
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                email TEXT NOT NULL,
                verified_at TEXT,
                created_at TEXT NOT NULL,
                PRIMARY KEY (user_id, email)
            )"#,
            r#"INSERT INTO user_commit_emails_next (user_id, email, verified_at, created_at)
               SELECT user_id, email, created_at, created_at FROM user_commit_emails"#,
            "DROP TABLE user_commit_emails",
            "ALTER TABLE user_commit_emails_next RENAME TO user_commit_emails",
            r#"CREATE UNIQUE INDEX uq_user_commit_emails_verified
               ON user_commit_emails(email) WHERE verified_at IS NOT NULL"#,
            r#"CREATE INDEX idx_user_commit_emails_email
               ON user_commit_emails(email)"#,
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
        let statements = [
            r#"CREATE TABLE user_commit_emails_prev (
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                email TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL,
                PRIMARY KEY (user_id, email)
            )"#,
            r#"INSERT INTO user_commit_emails_prev (user_id, email, created_at)
               SELECT user_id, email, created_at FROM user_commit_emails
               WHERE verified_at IS NOT NULL"#,
            "DROP TABLE user_commit_emails",
            "ALTER TABLE user_commit_emails_prev RENAME TO user_commit_emails",
        ];
        for statement in statements {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }
}
