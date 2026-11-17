use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        let statements = [
            "ALTER TABLE namespace_mirror_identities ADD COLUMN provider TEXT",
            "ALTER TABLE namespace_mirror_identities ADD COLUMN instance_url TEXT",
            "ALTER TABLE namespace_mirror_identities ADD COLUMN public_key TEXT",
            "ALTER TABLE namespace_mirror_identities ADD COLUMN fingerprint TEXT",
            "ALTER TABLE namespace_mirror_identities ADD COLUMN last_used_at TEXT",
            "ALTER TABLE repository_mirrors ADD COLUMN ssh_identity_id TEXT",
            "ALTER TABLE repository_imports ADD COLUMN identity_id TEXT",
            "UPDATE namespace_mirror_identities SET provider = 'github', instance_url = 'https://api.github.com' WHERE kind = 'token'",
            "INSERT INTO namespace_mirror_identities (id, namespace, name, kind, username, secret, created_at, updated_at, public_key, fingerprint, last_used_at) SELECT unhex(replace(printf('00000000-0000-4000-8000-%012x', rowid), '-', '')), namespace, 'Default SSH key', 'ssh', NULL, private_key, created_at, updated_at, public_key, fingerprint, last_used_at FROM namespace_mirror_ssh_keys",
            "UPDATE repository_mirrors AS m SET ssh_identity_id = (SELECT i.id FROM namespace_mirror_identities i JOIN repositories r ON r.namespace = i.namespace WHERE r.id = m.repository_id AND i.kind = 'ssh' ORDER BY i.created_at LIMIT 1) WHERE m.remote_url LIKE 'ssh://%' OR m.remote_url LIKE 'git@github.com:%'",
            "DROP TABLE namespace_mirror_ssh_keys",
            "DROP TRIGGER IF EXISTS trg_namespace_mirror_identity_delete",
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

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        let statements = [
            "DROP TRIGGER IF EXISTS trg_namespace_mirror_identity_delete",
            "DROP TRIGGER IF EXISTS trg_repository_import_identity_update",
            "DROP TRIGGER IF EXISTS trg_repository_import_identity_insert",
            "DROP TRIGGER IF EXISTS trg_repository_mirror_ssh_identity_update",
            "DROP TRIGGER IF EXISTS trg_repository_mirror_ssh_identity_insert",
            "CREATE TABLE namespace_mirror_ssh_keys (namespace TEXT PRIMARY KEY NOT NULL REFERENCES namespaces(slug) ON DELETE CASCADE, private_key TEXT NOT NULL, public_key TEXT NOT NULL, fingerprint TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_used_at TEXT)",
            "INSERT INTO namespace_mirror_ssh_keys (namespace, private_key, public_key, fingerprint, created_at, updated_at, last_used_at) SELECT namespace, secret, public_key, fingerprint, created_at, updated_at, last_used_at FROM namespace_mirror_identities WHERE kind = 'ssh' GROUP BY namespace",
            "DELETE FROM namespace_mirror_identities WHERE kind = 'ssh'",
            "ALTER TABLE repository_imports DROP COLUMN identity_id",
            "ALTER TABLE repository_mirrors DROP COLUMN ssh_identity_id",
            "ALTER TABLE namespace_mirror_identities DROP COLUMN last_used_at",
            "ALTER TABLE namespace_mirror_identities DROP COLUMN fingerprint",
            "ALTER TABLE namespace_mirror_identities DROP COLUMN public_key",
            "ALTER TABLE namespace_mirror_identities DROP COLUMN instance_url",
            "ALTER TABLE namespace_mirror_identities DROP COLUMN provider",
            "CREATE TRIGGER trg_namespace_mirror_identity_delete BEFORE DELETE ON namespace_mirror_identities WHEN EXISTS (SELECT 1 FROM repository_mirrors AS m WHERE m.identity_id = OLD.id) BEGIN SELECT RAISE(ABORT, 'mirror identity is in use'); END",
        ];
        for statement in statements {
            connection.execute_unprepared(statement).await?;
        }
        Ok(())
    }
}
