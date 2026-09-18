use sea_orm::entity::prelude::*;

pub mod instance {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "instance")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        pub created_at: DateTimeUtc,
        pub site_name: String,
        pub site_description: Option<String>,
        pub password_login_enabled: bool,
        pub passkey_login_enabled: bool,
        pub integrity_checks_enabled: bool,
        pub integrity_check_schedule: String,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod backup_provider {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "backup_providers")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub name: String,
        pub provider: String,
        pub configuration: String,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod backup_provider_exclusion {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "backup_provider_exclusions")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub provider_id: Uuid,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod backup_provider_schedule {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "backup_provider_schedules")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub provider_id: Uuid,
        pub schedule: String,
        pub next_backup_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod lfs_storage_target {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "lfs_storage_targets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub name: String,
        pub kind: String,
        pub configuration: String,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod lfs_storage_state {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "lfs_storage_state")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        pub active_target_id: Option<Uuid>,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod lfs_object {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "lfs_objects")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub repository_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub oid: String,
        pub size: i64,
        pub storage_target_id: Option<Uuid>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod lfs_storage_migration {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "lfs_storage_migrations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub source_target_id: Option<Uuid>,
        pub target_id: Option<Uuid>,
        pub state: String,
        pub phase: String,
        pub last_key: Option<String>,
        pub copied_objects: i64,
        pub copied_bytes: i64,
        pub error: Option<String>,
        pub started_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
        pub completed_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod registry_storage_state {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "registry_storage_state")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        pub active_target_id: Option<Uuid>,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod registry_storage_migration {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "registry_storage_migrations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub source_target_id: Option<Uuid>,
        pub target_id: Option<Uuid>,
        pub state: String,
        pub phase: String,
        pub last_key: Option<String>,
        pub copied_objects: i64,
        pub copied_bytes: i64,
        pub total_bytes: Option<i64>,
        pub error: Option<String>,
        pub started_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
        pub completed_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod namespace_integration {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "namespace_integrations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        /// Namespace slug that owns this connection instance.
        pub namespace: String,
        /// Provider implementation used by this connection.
        pub provider: String,
        /// User-defined label that distinguishes instances of one provider.
        pub name: String,
        pub enabled: bool,
        pub url: String,
        pub api_key: String,
        /// URL Dokploy containers use to reach Gitadel.
        pub dokploy_internal_url: Option<String>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod dokploy_source_binding {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "dokploy_source_bindings")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub integration_id: Uuid,
        pub gitea_id: String,
        pub git_provider_id: String,
        pub oauth_application_id: Option<Uuid>,
        pub managed: bool,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_integration {
    use super::*;

    /// An independently configured integration attached to a repository.
    /// Multiple rows may use the same namespace connection.
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_integrations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub repository_id: Uuid,
        pub integration_id: Uuid,
        pub name: String,
        pub enabled: bool,
        /// Which remote resource this repository deploys as, if linked.
        /// Opaque JSON: only the provider's dispatcher reads it.
        pub resource: Option<String>,
        /// Provider-specific deployment configuration.
        pub config: Option<String>,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod instance_asset {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "instance_assets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub name: String,
        pub content_type: String,
        pub content: Vec<u8>,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod oidc_provider {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "oidc_providers")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub name: String,
        pub issuer_url: String,
        pub client_id: String,
        pub client_secret: String,
        pub enabled: bool,
        pub auto_provision: bool,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod oidc_identity {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "oidc_identities")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub provider_id: Uuid,
        pub subject: String,
        pub user_id: Uuid,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod user {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "users")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        #[sea_orm(unique)]
        pub username: String,
        pub password_hash: String,
        pub is_admin: bool,
        pub default_repository_visibility: String,
        pub theme_preference: String,
        pub disabled_at: Option<DateTimeUtc>,
        pub avatar_updated_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod user_avatar {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "user_avatars")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub user_id: Uuid,
        pub content: Vec<u8>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod session {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "sessions")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub token_hash: String,
        pub user_id: Uuid,
        pub expires_at: DateTimeUtc,
        pub created_at: DateTimeUtc,
        pub last_seen_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod invitation {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "invitations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub token_hash: String,
        pub created_by: Uuid,
        pub expires_at: DateTimeUtc,
        pub used_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod passkey {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "passkeys")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub user_id: Uuid,
        pub name: String,
        #[sea_orm(unique)]
        pub credential_id: String,
        pub credential: String,
        pub created_at: DateTimeUtc,
        pub last_used_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod ssh_key {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "ssh_keys")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub user_id: Uuid,
        pub name: String,
        #[sea_orm(unique)]
        pub fingerprint: String,
        pub public_key: String,
        pub created_at: DateTimeUtc,
        pub last_used_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod oauth_application {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "oauth_applications")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub user_id: Uuid,
        pub name: String,
        #[sea_orm(unique)]
        pub client_id: String,
        pub client_secret_hash: String,
        pub redirect_uri: String,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod oauth_authorization_code {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "oauth_authorization_codes")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub code_hash: String,
        pub application_id: Uuid,
        pub user_id: Uuid,
        pub redirect_uri: String,
        pub scope: String,
        pub expires_at: DateTimeUtc,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod oauth_access_token {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "oauth_access_tokens")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub token_hash: String,
        pub application_id: Uuid,
        pub user_id: Uuid,
        pub scopes: i32,
        pub scope: String,
        pub created_at: DateTimeUtc,
        pub last_used_at: Option<DateTimeUtc>,
        pub revoked_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod api_token {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "api_tokens")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub user_id: Uuid,
        pub name: String,
        #[sea_orm(unique)]
        pub token_hash: String,
        pub scopes: i32,
        pub expires_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub last_used_at: Option<DateTimeUtc>,
        pub revoked_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod organization {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "organizations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        #[sea_orm(unique)]
        pub slug: String,
        pub display_name: String,
        pub avatar_updated_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod organization_avatar {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "organization_avatars")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub organization_id: Uuid,
        pub content: Vec<u8>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod organization_member {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "organization_members")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub organization_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub user_id: Uuid,
        pub role: String,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod audit_event {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "audit_events")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = true)]
        pub id: i64,
        pub actor_user_id: Option<Uuid>,
        pub action: String,
        pub target: Option<String>,
        pub remote_address: Option<String>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(
            belongs_to = "super::user::Entity",
            from = "Column::ActorUserId",
            to = "super::user::Column::Id"
        )]
        User,
    }

    impl Related<super::user::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::User.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod namespace {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "namespaces")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub slug: String,
        pub kind: String,
        pub user_id: Option<Uuid>,
        pub organization_id: Option<Uuid>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod namespace_mirror_identity {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "namespace_mirror_identities")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub namespace: String,
        pub name: String,
        pub kind: String,
        pub username: Option<String>,
        pub secret: String,
        pub provider: Option<String>,
        pub instance_url: Option<String>,
        pub public_key: Option<String>,
        pub fingerprint: Option<String>,
        pub last_used_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repositories")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub namespace: String,
        pub name: String,
        pub description: Option<String>,
        pub website_url: Option<String>,
        pub visibility: String,
        pub object_format: String,
        pub mirrored: bool,
        pub default_branch: Option<String>,
        pub issue_counter: i64,
        #[sea_orm(unique)]
        pub storage_key: Uuid,
        pub created_by: Uuid,
        pub archived_at: Option<DateTimeUtc>,
        pub deleted_at: Option<DateTimeUtc>,
        pub icon_updated_at: Option<DateTimeUtc>,
        pub icon_source: Option<String>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_icon {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_icons")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub repository_id: Uuid,
        pub content: Vec<u8>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_icon_candidate {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_icon_candidates")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub repository_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub path: String,
        pub commit_oid: String,
        pub oid: String,
        pub width: i32,
        pub height: i32,
        pub mime_type: String,
        pub reasons_json: String,
        pub recommended: bool,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_icon_scan {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_icon_scans")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub repository_id: Uuid,
        pub commit_oid: Option<String>,
        pub selected_path: Option<String>,
        pub selected_missing: bool,
        pub detector_version: i64,
        pub status: String,
        pub version: i64,
        pub error: Option<String>,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_cache_entry {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_cache_entries")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub repository_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub kind: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub cache_key: String,
        pub payload_json: String,
        pub expires_at: DateTimeUtc,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_mirror {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_mirrors")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub repository_id: Uuid,
        pub remote_url: String,
        pub identity_id: Option<Uuid>,
        pub github_owner: Option<String>,
        pub github_repository: Option<String>,
        pub schedule: Option<String>,
        pub last_attempted_at: Option<DateTimeUtc>,
        pub last_synced_at: Option<DateTimeUtc>,
        pub last_error: Option<String>,
        pub metadata_last_synced_at: Option<DateTimeUtc>,
        pub metadata_error: Option<String>,
        pub next_sync_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_import {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_imports")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub created_by: Uuid,
        pub target_namespace: String,
        pub provider: String,
        pub instance_url: String,
        pub state: String,
        pub identity_id: Option<Uuid>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_import_item {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_import_items")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub import_id: Uuid,
        pub source_id: String,
        pub source_full_name: String,
        pub source_web_url: String,
        pub source_clone_url: String,
        pub target_namespace: String,
        pub target_name: String,
        pub target_visibility: String,
        pub state: String,
        pub attempts: i32,
        pub repository_id: Option<Uuid>,
        pub last_error: Option<String>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_alias {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_aliases")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub namespace: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub name: String,
        pub repository_id: Uuid,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_collaborator {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_collaborators")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub repository_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub user_id: Uuid,
        pub role: String,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_webhook {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_webhooks")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub repository_id: Uuid,
        pub url: String,
        pub secret: Option<String>,
        pub active: bool,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
        pub last_delivery_at: Option<DateTimeUtc>,
        pub last_response_status: Option<i32>,
        pub last_response_message: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_webhook_delivery {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_webhook_deliveries")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub webhook_id: Uuid,
        pub event: String,
        pub payload: String,
        pub response_status: Option<i32>,
        pub response_body: Option<String>,
        pub duration_ms: i32,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod topic {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "topics")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        #[sea_orm(unique)]
        pub name: String,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_topic {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_topics")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub repository_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub topic_id: Uuid,
        pub created_at: DateTimeUtc,
        pub external_source: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_issue {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_issues")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub repository_id: Uuid,
        pub number: i64,
        pub author_user_id: Uuid,
        pub title: String,
        pub body: String,
        pub state: String,
        pub assignee_user_id: Option<Uuid>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
        pub closed_at: Option<DateTimeUtc>,
        pub external_source: Option<String>,
        pub external_id: Option<String>,
        pub external_url: Option<String>,
        pub external_author: Option<String>,
        pub external_author_url: Option<String>,
        pub external_updated_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod issue_attachment {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "issue_attachments")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub repository_id: Uuid,
        pub issue_id: Option<Uuid>,
        pub uploader_user_id: Uuid,
        pub name: String,
        pub content_type: String,
        pub size_bytes: i64,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod issue_comment {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "issue_comments")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub issue_id: Uuid,
        pub author_user_id: Uuid,
        pub body: String,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
        pub external_source: Option<String>,
        pub external_id: Option<String>,
        pub external_url: Option<String>,
        pub external_author: Option<String>,
        pub external_author_url: Option<String>,
        pub external_updated_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod issue_label {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "issue_labels")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub repository_id: Uuid,
        pub name: String,
        pub color: String,
        pub description: String,
        pub external_source: Option<String>,
        pub external_instance_url: Option<String>,
        pub external_id: Option<String>,
        pub external_url: Option<String>,
        pub external_updated_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod issue_label_assignment {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "issue_label_assignments")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub issue_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub label_id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_release {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_releases")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub repository_id: Uuid,
        pub author_user_id: Uuid,
        pub target_revision: String,
        pub target_oid: String,
        pub title: String,
        pub body: String,
        pub prerelease: bool,
        pub published_at: DateTimeUtc,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
        pub external_source: Option<String>,
        pub external_instance_url: Option<String>,
        pub external_id: Option<String>,
        pub external_url: Option<String>,
        pub external_author: Option<String>,
        pub external_author_url: Option<String>,
        pub external_updated_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod release_asset {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "release_assets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub release_id: Uuid,
        pub name: String,
        pub content_type: String,
        pub size_bytes: i64,
        pub download_count: i64,
        pub created_at: DateTimeUtc,
        pub external_source: Option<String>,
        pub external_instance_url: Option<String>,
        pub external_id: Option<String>,
        pub external_url: Option<String>,
        pub external_updated_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod repository_favorite {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "repository_favorites")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub user_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub repository_id: Uuid,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod lfs_lock {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "lfs_locks")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub repository_id: Uuid,
        pub user_id: Uuid,
        pub path: String,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod action_run {
    use super::*;
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_runs")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub repository_id: Uuid,
        pub number: i64,
        pub workflow_path: String,
        pub workflow_name: String,
        pub event: String,
        pub ref_name: String,
        pub before_sha: String,
        pub after_sha: String,
        pub actor_id: Option<Uuid>,
        pub status: String,
        pub failure_kind: Option<String>,
        pub failure_summary: Option<String>,
        pub diagnostic: Option<String>,
        pub event_json: String,
        pub cancel_requested_at: Option<DateTimeUtc>,
        pub cancelled_by: Option<Uuid>,
        pub created_at: DateTimeUtc,
        pub started_at: Option<DateTimeUtc>,
        pub completed_at: Option<DateTimeUtc>,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub mod action_runner {
    use super::*;
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_runners")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub uuid: String,
        pub namespace: Option<String>,
        pub name: String,
        pub token_hash: String,
        pub approved_labels: String,
        pub version: String,
        pub ephemeral: bool,
        pub disabled_at: Option<DateTimeUtc>,
        pub deleted_at: Option<DateTimeUtc>,
        pub last_seen_at: Option<DateTimeUtc>,
        pub created_by: Option<Uuid>,
        pub created_at: DateTimeUtc,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub mod action_job {
    use super::*;
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_jobs")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub run_id: Uuid,
        pub job_key: String,
        pub name: String,
        pub required_labels: String,
        pub workflow_payload: Vec<u8>,
        pub status: String,
        pub result: Option<String>,
        pub runner_id: Option<i64>,
        pub request_key: Option<String>,
        pub lease_generation: i64,
        pub lease_deadline: Option<DateTimeUtc>,
        pub last_report_at: Option<DateTimeUtc>,
        pub attempt: i64,
        pub step_state: String,
        pub outputs: String,
        pub expected_log_index: i64,
        pub log_bytes: i64,
        pub log_truncated: bool,
        pub failure_kind: Option<String>,
        pub failure_summary: Option<String>,
        pub created_at: DateTimeUtc,
        pub started_at: Option<DateTimeUtc>,
        pub completed_at: Option<DateTimeUtc>,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub mod action_job_need {
    use super::*;
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_job_needs")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub job_id: i64,
        #[sea_orm(primary_key, auto_increment = false)]
        pub needed_job_id: i64,
        pub needed_job_key: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub mod action_job_log {
    use super::*;
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_job_logs")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub job_id: i64,
        #[sea_orm(primary_key, auto_increment = false)]
        pub row_index: i64,
        pub timestamp: DateTimeUtc,
        pub content: String,
        pub byte_count: i64,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub mod action_runner_registration_token {
    use super::*;
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_runner_registration_tokens")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub token_hash: String,
        pub namespace: Option<String>,
        pub runner_name: String,
        pub approved_labels: String,
        pub expires_at: DateTimeUtc,
        pub used_at: Option<DateTimeUtc>,
        pub created_by: Option<Uuid>,
        pub created_at: DateTimeUtc,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub mod action_runner_fetch {
    use super::*;
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_runner_fetches")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub runner_id: i64,
        #[sea_orm(primary_key, auto_increment = false)]
        pub request_key: String,
        pub job_id: i64,
        pub lease_generation: i64,
        pub token_generation: String,
        pub created_at: DateTimeUtc,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub mod action_job_token {
    use super::*;
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_job_tokens")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub job_id: i64,
        pub repository_id: Uuid,
        pub lease_generation: i64,
        pub token_hash: String,
        pub expires_at: DateTimeUtc,
        pub revoked_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub mod action_artifact {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_artifacts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub repository_id: Uuid,
        pub run_id: Uuid,
        pub creating_job_id: i64,
        pub name: String,
        pub storage_key: String,
        pub status: String,
        pub size_bytes: i64,
        pub sha256: Option<String>,
        pub expires_at: DateTimeUtc,
        pub finalized_at: Option<DateTimeUtc>,
        pub deleted_at: Option<DateTimeUtc>,
        pub deleted_by_job_id: Option<i64>,
        pub metadata_json: String,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod action_artifact_grant {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_artifact_grants")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub artifact_id: i64,
        pub issued_to_job_id: i64,
        pub scope: String,
        pub token_hash: String,
        pub expires_at: DateTimeUtc,
        pub revoked_at: Option<DateTimeUtc>,
        pub last_used_at: Option<DateTimeUtc>,
        pub use_count: i64,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
