use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        let statements = [
            "DROP TRIGGER IF EXISTS trg_namespace_mirror_identity_delete",
            "DROP TRIGGER IF EXISTS trg_repository_import_identity_update",
            "DROP TRIGGER IF EXISTS trg_repository_import_identity_insert",
            "DROP TRIGGER IF EXISTS trg_repository_mirror_ssh_identity_update",
            "DROP TRIGGER IF EXISTS trg_repository_mirror_ssh_identity_insert",
            "UPDATE repository_mirrors SET ssh_identity_id = unhex(replace(ssh_identity_id, '-', '')) WHERE typeof(ssh_identity_id) = 'text' AND length(ssh_identity_id) = 36",
            "UPDATE namespace_mirror_identities SET instance_url = 'https://api.github.com' WHERE kind = 'token' AND provider = 'github' AND instance_url = 'https://github.com'",
            "UPDATE repository_imports SET identity_id = unhex(replace(identity_id, '-', '')) WHERE typeof(identity_id) = 'text' AND length(identity_id) = 36",
            "UPDATE namespace_mirror_identities SET id = unhex(replace(id, '-', '')) WHERE typeof(id) = 'text' AND length(id) = 36",
            "CREATE TRIGGER trg_repository_mirror_ssh_identity_insert BEFORE INSERT ON repository_mirrors WHEN NEW.ssh_identity_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM namespace_mirror_identities i JOIN repositories r ON r.id = NEW.repository_id WHERE i.id = NEW.ssh_identity_id AND i.namespace = r.namespace AND i.kind = 'ssh') BEGIN SELECT RAISE(ABORT, 'invalid SSH mirror identity'); END",
            "CREATE TRIGGER trg_repository_mirror_ssh_identity_update BEFORE UPDATE OF ssh_identity_id, repository_id ON repository_mirrors WHEN NEW.ssh_identity_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM namespace_mirror_identities i JOIN repositories r ON r.id = NEW.repository_id WHERE i.id = NEW.ssh_identity_id AND i.namespace = r.namespace AND i.kind = 'ssh') BEGIN SELECT RAISE(ABORT, 'invalid SSH mirror identity'); END",
            "CREATE TRIGGER trg_repository_import_identity_insert BEFORE INSERT ON repository_imports WHEN NEW.identity_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM namespace_mirror_identities i JOIN namespaces n ON n.slug = i.namespace LEFT JOIN organization_members om ON om.organization_id = n.organization_id AND om.user_id = NEW.created_by WHERE i.id = NEW.identity_id AND (n.user_id = NEW.created_by OR om.role = 'owner')) BEGIN SELECT RAISE(ABORT, 'invalid import identity'); END",
            "CREATE TRIGGER trg_repository_import_identity_update BEFORE UPDATE OF identity_id, created_by ON repository_imports WHEN NEW.identity_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM namespace_mirror_identities i JOIN namespaces n ON n.slug = i.namespace LEFT JOIN organization_members om ON om.organization_id = n.organization_id AND om.user_id = NEW.created_by WHERE i.id = NEW.identity_id AND (n.user_id = NEW.created_by OR om.role = 'owner')) BEGIN SELECT RAISE(ABORT, 'invalid import identity'); END",
            "CREATE TRIGGER trg_namespace_mirror_identity_delete BEFORE DELETE ON namespace_mirror_identities WHEN EXISTS (SELECT 1 FROM repository_mirrors m WHERE m.identity_id = OLD.id OR m.ssh_identity_id = OLD.id) OR EXISTS (SELECT 1 FROM repository_imports i WHERE i.identity_id = OLD.id) BEGIN SELECT RAISE(ABORT, 'repository identity is in use'); END",
        ];
        for statement in statements {
            connection.execute_unprepared(statement).await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
