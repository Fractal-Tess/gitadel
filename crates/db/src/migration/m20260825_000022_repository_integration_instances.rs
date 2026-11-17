use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let database = manager.get_connection();
        database
            .execute_unprepared(
                r#"
ALTER TABLE repository_integrations RENAME TO repository_integrations_connections;
CREATE TABLE repository_integrations (
    id uuid NOT NULL PRIMARY KEY,
    repository_id uuid NOT NULL,
    integration_id uuid NOT NULL,
    name varchar(255) NOT NULL,
    enabled boolean NOT NULL,
    resource text,
    config text,
    updated_at timestamptz NOT NULL,
    CONSTRAINT fk_repository_integration_repository
        FOREIGN KEY (repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
    CONSTRAINT fk_repository_integration_connection
        FOREIGN KEY (integration_id) REFERENCES namespace_integrations (id) ON DELETE CASCADE
);
INSERT INTO repository_integrations
    (id, repository_id, integration_id, name, enabled, resource, config, updated_at)
SELECT unhex(
           lower(hex(randomblob(6))) || '4' ||
           substr(lower(hex(randomblob(1))), 2) || lower(hex(randomblob(1))) ||
           substr('89ab', (random() & 3) + 1, 1) ||
           substr(lower(hex(randomblob(1))), 2) || lower(hex(randomblob(7)))
       ),
       previous.repository_id, previous.integration_id, connection.name,
       previous.enabled, previous.resource, previous.config, previous.updated_at
FROM repository_integrations_connections previous
JOIN namespace_integrations connection ON connection.id = previous.integration_id;
DROP TABLE repository_integrations_connections;
CREATE INDEX idx_repository_integrations_repository_id
    ON repository_integrations (repository_id);
CREATE INDEX idx_repository_integrations_connection_id
    ON repository_integrations (integration_id);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let database = manager.get_connection();
        database
            .execute_unprepared(
                r#"
ALTER TABLE repository_integrations RENAME TO repository_integration_instances;
CREATE TABLE repository_integrations (
    repository_id uuid NOT NULL,
    integration_id uuid NOT NULL,
    enabled boolean NOT NULL,
    resource text,
    config text,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (repository_id, integration_id),
    CONSTRAINT fk_repository_integration_repository
        FOREIGN KEY (repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
    CONSTRAINT fk_repository_integration_instance
        FOREIGN KEY (integration_id) REFERENCES namespace_integrations (id) ON DELETE CASCADE
);
INSERT INTO repository_integrations
    (repository_id, integration_id, enabled, resource, config, updated_at)
SELECT instance.repository_id, instance.integration_id, instance.enabled,
       instance.resource, instance.config, instance.updated_at
FROM repository_integration_instances instance
WHERE instance.id = (
    SELECT candidate.id
    FROM repository_integration_instances candidate
    WHERE candidate.repository_id = instance.repository_id
      AND candidate.integration_id = instance.integration_id
    ORDER BY candidate.updated_at DESC, candidate.id
    LIMIT 1
);
DROP TABLE repository_integration_instances;
"#,
            )
            .await?;
        Ok(())
    }
}
