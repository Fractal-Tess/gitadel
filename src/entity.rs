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
        pub default_repository_visibility: String,
        pub updated_at: DateTimeUtc,
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
        pub disabled_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
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
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
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
    pub enum Relation {}

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
        pub visibility: String,
        pub object_format: String,
        pub default_branch: String,
        #[sea_orm(unique)]
        pub storage_key: Uuid,
        pub created_by: Uuid,
        pub archived_at: Option<DateTimeUtc>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
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
