use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let database = manager.get_connection();
        if manager.has_table("repository_integrations").await? {
            database
                .execute_unprepared(
                    "ALTER TABLE repository_integrations RENAME TO repository_integrations_legacy",
                )
                .await?;
        }
        if !manager.has_table("namespace_integrations_legacy").await? {
            database
                .execute_unprepared(
                    "ALTER TABLE namespace_integrations RENAME TO namespace_integrations_legacy",
                )
                .await?;
        }
        if !manager.has_table("namespace_integrations").await? {
            database
                .execute_unprepared(
                    r#"CREATE TABLE namespace_integrations (
                        id uuid NOT NULL PRIMARY KEY,
                        namespace text NOT NULL,
                        provider text NOT NULL,
                        name text NOT NULL,
                        enabled boolean NOT NULL,
                        url text NOT NULL,
                        api_key text NOT NULL,
                        created_at timestamptz NOT NULL,
                        updated_at timestamptz NOT NULL,
                        CONSTRAINT fk_namespace_integration_namespace
                            FOREIGN KEY (namespace) REFERENCES namespaces (slug) ON DELETE CASCADE
                    )"#,
                )
                .await?;
            database
                .execute_unprepared(
                    "CREATE INDEX idx_namespace_integration_owner \
                     ON namespace_integrations (namespace, provider)",
                )
                .await?;
        }
        database
            .execute_unprepared(
                r#"INSERT INTO namespace_integrations
                    (id, namespace, provider, name, enabled, url, api_key, created_at, updated_at)
                   SELECT randomblob(16), namespace, provider,
                          CASE provider WHEN 'dokploy' THEN 'Dokploy' ELSE provider END,
                          TRUE, url, api_key, created_at, updated_at
                   FROM namespace_integrations_legacy
                   WHERE NOT EXISTS (
                       SELECT 1 FROM namespace_integrations current
                       WHERE current.namespace = namespace_integrations_legacy.namespace
                         AND current.provider = namespace_integrations_legacy.provider
                   )"#,
            )
            .await?;
        if !manager.has_table("repository_integrations").await? {
            database
                .execute_unprepared(
                    r#"CREATE TABLE repository_integrations (
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
                    )"#,
                )
                .await?;
        }
        database
            .execute_unprepared(
                r#"INSERT INTO repository_integrations
                    (repository_id, integration_id, enabled, resource, config, updated_at)
                   SELECT legacy.repository_id, instance.id, legacy.enabled,
                          legacy.resource, legacy.config, legacy.updated_at
                   FROM repository_integrations_legacy legacy
                   JOIN repositories repository ON repository.id = legacy.repository_id
                   JOIN namespace_integrations instance
                     ON instance.namespace = repository.namespace
                    AND instance.provider = legacy.provider
                   WHERE NOT EXISTS (
                       SELECT 1 FROM repository_integrations current
                       WHERE current.repository_id = legacy.repository_id
                         AND current.integration_id = instance.id
                   )"#,
            )
            .await?;
        database
            .execute_unprepared("DROP TABLE repository_integrations_legacy")
            .await?;
        database
            .execute_unprepared("DROP TABLE namespace_integrations_legacy")
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let database = manager.get_connection();
        database
            .execute_unprepared(
                r#"
ALTER TABLE repository_integrations RENAME TO repository_integrations_instances;
ALTER TABLE namespace_integrations RENAME TO namespace_integration_instances;
CREATE TABLE namespace_integrations (
    namespace text NOT NULL,
    provider text NOT NULL,
    url text NOT NULL,
    api_key text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (namespace, provider),
    CONSTRAINT fk_namespace_integration_namespace
        FOREIGN KEY (namespace) REFERENCES namespaces (slug) ON DELETE CASCADE
);
INSERT INTO namespace_integrations
    (namespace, provider, url, api_key, created_at, updated_at)
SELECT namespace, provider, url, api_key, created_at, updated_at
FROM namespace_integration_instances
WHERE id IN (SELECT MIN(id) FROM namespace_integration_instances GROUP BY namespace, provider);

CREATE TABLE repository_integrations (
    repository_id uuid NOT NULL,
    provider text NOT NULL,
    enabled boolean NOT NULL,
    resource text,
    config text,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (repository_id, provider),
    CONSTRAINT fk_repository_integration_repository
        FOREIGN KEY (repository_id) REFERENCES repositories (id) ON DELETE CASCADE
);
INSERT INTO repository_integrations
    (repository_id, provider, enabled, resource, config, updated_at)
SELECT row.repository_id, instance.provider, row.enabled,
       row.resource, row.config, row.updated_at
FROM repository_integrations_instances row
JOIN namespace_integration_instances instance ON instance.id = row.integration_id
WHERE row.integration_id IN (
    SELECT MIN(candidate.integration_id)
    FROM repository_integrations_instances candidate
    JOIN namespace_integration_instances owner ON owner.id = candidate.integration_id
    GROUP BY candidate.repository_id, owner.provider
);

DROP TABLE repository_integrations_instances;
DROP TABLE namespace_integration_instances;
"#,
            )
            .await?;
        Ok(())
    }
}
