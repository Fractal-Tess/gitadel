use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ProtectionRule::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ProtectionRule::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ProtectionRule::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProtectionRule::Kind)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProtectionRule::Pattern)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProtectionRule::BlockForcePush)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(ProtectionRule::BlockDeletion)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(ProtectionRule::BlockUpdate)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(ProtectionRule::RestrictPushes)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(ProtectionRule::AdminsBypass)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(ProtectionRule::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProtectionRule::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-protection-rule-repository")
                            .from(ProtectionRule::Table, ProtectionRule::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-protection-rule-pattern")
                    .table(ProtectionRule::Table)
                    .col(ProtectionRule::RepositoryId)
                    .col(ProtectionRule::Kind)
                    .col(ProtectionRule::Pattern)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(ProtectionRuleUser::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ProtectionRuleUser::RuleId).uuid().not_null())
                    .col(ColumnDef::new(ProtectionRuleUser::UserId).uuid().not_null())
                    .primary_key(
                        Index::create()
                            .col(ProtectionRuleUser::RuleId)
                            .col(ProtectionRuleUser::UserId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-protection-rule-user-rule")
                            .from(ProtectionRuleUser::Table, ProtectionRuleUser::RuleId)
                            .to(ProtectionRule::Table, ProtectionRule::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-protection-rule-user-user")
                            .from(ProtectionRuleUser::Table, ProtectionRuleUser::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ProtectionRuleUser::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ProtectionRule::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ProtectionRule {
    #[sea_orm(iden = "repository_protection_rules")]
    Table,
    Id,
    RepositoryId,
    Kind,
    Pattern,
    BlockForcePush,
    BlockDeletion,
    BlockUpdate,
    RestrictPushes,
    AdminsBypass,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ProtectionRuleUser {
    #[sea_orm(iden = "repository_protection_rule_users")]
    Table,
    RuleId,
    UserId,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}
