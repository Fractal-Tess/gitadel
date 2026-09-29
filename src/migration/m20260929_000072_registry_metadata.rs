use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Container registry metadata moves from files into the database, so blobs
/// and manifest bytes are the only payloads and the whole registry can live on
/// any storage target.
///
/// - `registry_images`: one row per image name (the suffix after
///   `<namespace>/<repository>`).
/// - `registry_objects`: payloads a repository holds in the active registry
///   store, keyed `registry/<storage-key>/{blobs,manifests}/aa/bb/<digest>`.
///   Images of one repository share them.
/// - `registry_image_blobs`: the blobs each image exposes; a layer mount inside
///   one repository only adds a row here.
/// - `registry_manifests`: the manifests each image holds, with what the
///   referrers API and browsing need.
/// - `registry_manifest_references`: every digest a manifest references, which
///   blocks deleting a referenced blob or manifest.
/// - `registry_tags`: tag names pointing at manifests.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
CREATE TABLE registry_images (
    id uuid NOT NULL PRIMARY KEY,
    repository_id uuid NOT NULL,
    name text NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT fk_registry_image_repository
        FOREIGN KEY (repository_id) REFERENCES repositories (id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX idx_registry_image_name ON registry_images (repository_id, name);

CREATE TABLE registry_objects (
    repository_id uuid NOT NULL,
    kind text NOT NULL CHECK (kind IN ('blob', 'manifest')),
    digest text NOT NULL,
    size bigint NOT NULL CHECK (size >= 0),
    created_at timestamptz NOT NULL,
    PRIMARY KEY (repository_id, kind, digest),
    CONSTRAINT fk_registry_object_repository
        FOREIGN KEY (repository_id) REFERENCES repositories (id) ON DELETE CASCADE
);

CREATE TABLE registry_image_blobs (
    image_id uuid NOT NULL,
    digest text NOT NULL,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (image_id, digest),
    CONSTRAINT fk_registry_image_blob_image
        FOREIGN KEY (image_id) REFERENCES registry_images (id) ON DELETE CASCADE
);

CREATE TABLE registry_manifests (
    image_id uuid NOT NULL,
    digest text NOT NULL,
    media_type text NOT NULL,
    size bigint NOT NULL CHECK (size >= 0),
    subject_digest text NULL,
    artifact_type text NULL,
    annotations text NULL,
    created_at timestamptz NOT NULL,
    pushed_at timestamptz NULL,
    PRIMARY KEY (image_id, digest),
    CONSTRAINT fk_registry_manifest_image
        FOREIGN KEY (image_id) REFERENCES registry_images (id) ON DELETE CASCADE
);
CREATE INDEX idx_registry_manifest_subject ON registry_manifests (image_id, subject_digest);

CREATE TABLE registry_manifest_references (
    image_id uuid NOT NULL,
    manifest_digest text NOT NULL,
    referenced_digest text NOT NULL,
    PRIMARY KEY (image_id, manifest_digest, referenced_digest),
    CONSTRAINT fk_registry_manifest_reference_manifest
        FOREIGN KEY (image_id, manifest_digest)
        REFERENCES registry_manifests (image_id, digest) ON DELETE CASCADE
);
CREATE INDEX idx_registry_manifest_reference_target
    ON registry_manifest_references (image_id, referenced_digest);

CREATE TABLE registry_tags (
    image_id uuid NOT NULL,
    name text NOT NULL,
    digest text NOT NULL,
    updated_at timestamptz NULL,
    PRIMARY KEY (image_id, name),
    CONSTRAINT fk_registry_tag_manifest
        FOREIGN KEY (image_id, digest)
        REFERENCES registry_manifests (image_id, digest) ON DELETE CASCADE
);
CREATE INDEX idx_registry_tag_digest ON registry_tags (image_id, digest);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
DROP TABLE registry_tags;
DROP TABLE registry_manifest_references;
DROP TABLE registry_manifests;
DROP TABLE registry_image_blobs;
DROP TABLE registry_objects;
DROP TABLE registry_images;
"#,
            )
            .await?;
        Ok(())
    }
}
