use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        let statements = [
            "DROP TRIGGER IF EXISTS trg_namespace_mirror_identity_delete",
            "DROP TRIGGER IF EXISTS trg_repository_mirror_ssh_identity_update",
            "DROP TRIGGER IF EXISTS trg_repository_mirror_ssh_identity_insert",
            "UPDATE repository_mirrors SET remote_url = 'https://github.com/' || substr(remote_url, length('ssh://git@github.com/') + 1) WHERE remote_url LIKE 'ssh://git@github.com/%'",
            "UPDATE repository_mirrors SET remote_url = 'https://github.com/' || substr(remote_url, length('git@github.com:') + 1) WHERE remote_url LIKE 'git@github.com:%'",
            "UPDATE repository_mirrors SET identity_id = NULL WHERE identity_id IN (SELECT id FROM namespace_mirror_identities WHERE kind <> 'token')",
            "UPDATE repository_mirrors SET ssh_identity_id = NULL",
            "ALTER TABLE repository_mirrors DROP COLUMN ssh_identity_id",
            "UPDATE namespace_mirror_identities SET instance_url = 'https://github.com' WHERE kind = 'token' AND provider = 'github' AND instance_url = 'https://api.github.com'",
            "DELETE FROM namespace_mirror_identities WHERE kind <> 'token' AND id NOT IN (SELECT identity_id FROM repository_imports WHERE identity_id IS NOT NULL)",
            "CREATE TRIGGER trg_namespace_mirror_identity_delete BEFORE DELETE ON namespace_mirror_identities WHEN EXISTS (SELECT 1 FROM repository_mirrors m WHERE m.identity_id = OLD.id) OR EXISTS (SELECT 1 FROM repository_imports i WHERE i.identity_id = OLD.id) BEGIN SELECT RAISE(ABORT, 'repository identity is in use'); END",
        ];
        for statement in statements {
            connection.execute_unprepared(statement).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        let statements = [
            "DROP TRIGGER IF EXISTS trg_namespace_mirror_identity_delete",
            "ALTER TABLE repository_mirrors ADD COLUMN ssh_identity_id BLOB",
            "UPDATE namespace_mirror_identities SET instance_url = 'https://api.github.com' WHERE kind = 'token' AND provider = 'github' AND instance_url = 'https://github.com'",
            "CREATE TRIGGER trg_repository_mirror_ssh_identity_insert BEFORE INSERT ON repository_mirrors WHEN NEW.ssh_identity_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM namespace_mirror_identities i JOIN repositories r ON r.id = NEW.repository_id WHERE i.id = NEW.ssh_identity_id AND i.namespace = r.namespace AND i.kind = 'ssh') BEGIN SELECT RAISE(ABORT, 'invalid SSH mirror identity'); END",
            "CREATE TRIGGER trg_repository_mirror_ssh_identity_update BEFORE UPDATE OF ssh_identity_id, repository_id ON repository_mirrors WHEN NEW.ssh_identity_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM namespace_mirror_identities i JOIN repositories r ON r.id = NEW.repository_id WHERE i.id = NEW.ssh_identity_id AND i.namespace = r.namespace AND i.kind = 'ssh') BEGIN SELECT RAISE(ABORT, 'invalid SSH mirror identity'); END",
            "CREATE TRIGGER trg_namespace_mirror_identity_delete BEFORE DELETE ON namespace_mirror_identities WHEN EXISTS (SELECT 1 FROM repository_mirrors m WHERE m.identity_id = OLD.id OR m.ssh_identity_id = OLD.id) OR EXISTS (SELECT 1 FROM repository_imports i WHERE i.identity_id = OLD.id) BEGIN SELECT RAISE(ABORT, 'repository identity is in use'); END",
        ];
        for statement in statements {
            connection.execute_unprepared(statement).await?;
        }
        Ok(())
    }
}
