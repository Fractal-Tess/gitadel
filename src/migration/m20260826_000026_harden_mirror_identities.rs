use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                "UPDATE namespace_mirror_identities \
                 SET kind = 'token' \
                 WHERE kind = 'basic' AND username IS NULL",
            )
            .await?;
        connection
            .execute_unprepared(
                "UPDATE repository_mirrors AS m \
                 SET identity_id = NULL \
                 WHERE identity_id IS NOT NULL \
                   AND NOT EXISTS ( \
                     SELECT 1 \
                     FROM namespace_mirror_identities AS i \
                     JOIN repositories AS r ON r.id = m.repository_id \
                     WHERE i.id = m.identity_id AND i.namespace = r.namespace \
                   )",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE TRIGGER IF NOT EXISTS trg_repository_mirror_identity_insert \
                 BEFORE INSERT ON repository_mirrors \
                 WHEN NEW.identity_id IS NOT NULL AND NOT EXISTS ( \
                   SELECT 1 \
                   FROM namespace_mirror_identities AS i \
                   JOIN repositories AS r ON r.id = NEW.repository_id \
                   WHERE i.id = NEW.identity_id AND i.namespace = r.namespace \
                 ) \
                 BEGIN \
                   SELECT RAISE(ABORT, 'invalid mirror identity'); \
                 END",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE TRIGGER IF NOT EXISTS trg_repository_mirror_identity_update \
                 BEFORE UPDATE OF identity_id, repository_id ON repository_mirrors \
                 WHEN NEW.identity_id IS NOT NULL AND NOT EXISTS ( \
                   SELECT 1 \
                   FROM namespace_mirror_identities AS i \
                   JOIN repositories AS r ON r.id = NEW.repository_id \
                   WHERE i.id = NEW.identity_id AND i.namespace = r.namespace \
                 ) \
                 BEGIN \
                   SELECT RAISE(ABORT, 'invalid mirror identity'); \
                 END",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE TRIGGER IF NOT EXISTS trg_namespace_mirror_identity_delete \
                 BEFORE DELETE ON namespace_mirror_identities \
                 WHEN EXISTS ( \
                   SELECT 1 FROM repository_mirrors AS m WHERE m.identity_id = OLD.id \
                 ) \
                 BEGIN \
                   SELECT RAISE(ABORT, 'mirror identity is in use'); \
                 END",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared("DROP TRIGGER IF EXISTS trg_namespace_mirror_identity_delete")
            .await?;
        connection
            .execute_unprepared("DROP TRIGGER IF EXISTS trg_repository_mirror_identity_update")
            .await?;
        connection
            .execute_unprepared("DROP TRIGGER IF EXISTS trg_repository_mirror_identity_insert")
            .await?;
        Ok(())
    }
}
