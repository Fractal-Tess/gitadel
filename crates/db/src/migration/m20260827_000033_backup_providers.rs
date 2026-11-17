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
CREATE TABLE backup_providers (
    id uuid NOT NULL PRIMARY KEY,
    name text NOT NULL,
    provider text NOT NULL,
    configuration text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX idx_backup_provider_kind ON backup_providers (provider);
INSERT INTO backup_providers
    (id, name, provider, configuration, created_at, updated_at)
SELECT randomblob(16), 'S3 backups', 's3',
       json_object(
           's3', json_object(
               'endpoint', endpoint,
               'bucket', bucket,
               'access_key', access_key,
               'secret_key', secret_key,
               'region', region,
               'prefix', prefix
           )
       ),
       updated_at, updated_at
FROM backup_settings;

CREATE TABLE backup_provider_schedules (
    provider_id uuid NOT NULL PRIMARY KEY,
    schedule text NOT NULL,
    next_backup_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT fk_backup_provider_schedule_provider
        FOREIGN KEY (provider_id) REFERENCES backup_providers (id) ON DELETE CASCADE
);
INSERT INTO backup_provider_schedules
    (provider_id, schedule, next_backup_at, updated_at)
SELECT provider.id, schedule.schedule, schedule.next_backup_at, schedule.updated_at
FROM backup_schedules schedule
JOIN backup_providers provider ON TRUE
ORDER BY provider.created_at, provider.id
LIMIT 1;

DROP TABLE backup_schedules;
DROP TABLE backup_settings;
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
CREATE TABLE backup_settings (
    id integer NOT NULL PRIMARY KEY,
    endpoint text NOT NULL,
    bucket text NOT NULL,
    access_key text NOT NULL,
    secret_key text NOT NULL,
    region text NOT NULL,
    prefix text NOT NULL,
    updated_at timestamptz NOT NULL
);
INSERT INTO backup_settings
    (id, endpoint, bucket, access_key, secret_key, region, prefix, updated_at)
SELECT 1,
       json_extract(configuration, '$.s3.endpoint'),
       json_extract(configuration, '$.s3.bucket'),
       json_extract(configuration, '$.s3.access_key'),
       json_extract(configuration, '$.s3.secret_key'),
       json_extract(configuration, '$.s3.region'),
       json_extract(configuration, '$.s3.prefix'),
       updated_at
FROM backup_providers
WHERE provider = 's3'
ORDER BY created_at, id
LIMIT 1;

CREATE TABLE backup_schedules (
    id integer NOT NULL PRIMARY KEY,
    schedule text NOT NULL,
    next_backup_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
INSERT INTO backup_schedules (id, schedule, next_backup_at, updated_at)
SELECT 1, schedule, next_backup_at, updated_at
FROM backup_provider_schedules
ORDER BY updated_at, provider_id
LIMIT 1;

DROP TABLE backup_provider_schedules;
DROP TABLE backup_providers;
"#,
            )
            .await?;
        Ok(())
    }
}
