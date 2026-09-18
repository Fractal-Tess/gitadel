mod m20260820_000001_create_instance;
mod m20260820_000002_create_identity;
mod m20260820_000003_create_repositories;
mod m20260820_000004_create_repository_favorites;
mod m20260820_000005_add_instance_settings;
mod m20260820_000006_create_lfs_locks;
mod m20260822_000007_create_oauth_provider;
mod m20260822_000008_scope_oauth_repository_tokens;
mod m20260822_000009_create_repository_webhooks;
mod m20260822_000010_repository_control;
mod m20260822_000011_create_repository_topics;
mod m20260822_000012_create_instance_assets;
mod m20260822_000013_add_user_avatar;
mod m20260822_000014_create_repository_releases;
mod m20260822_000015_create_repository_issues;
mod m20260822_000016_create_issue_attachments;
mod m20260822_000017_create_repository_webhook_deliveries;
mod m20260823_000018_add_integrations;
mod m20260824_000019_repository_integrations;
mod m20260824_000020_integration_instances;
mod m20260825_000021_account_repository_visibility;
mod m20260825_000022_repository_integration_instances;
mod m20260825_000023_repository_integration_uuid_storage;
mod m20260826_000024_repository_mirrors;
mod m20260826_000025_mirror_identities;
mod m20260826_000026_harden_mirror_identities;
mod m20260826_000027_dokploy_source_bindings;
mod m20260827_000028_dokploy_internal_url;
mod m20260827_000029_mirror_ssh_key_usage;
mod m20260827_000030_backup_settings;
mod m20260827_000031_user_theme_preference;
mod m20260827_000032_backup_schedules;
mod m20260827_000033_backup_providers;
mod m20260827_000034_repository_imports;
mod m20260827_000035_create_actions;
mod m20260827_000036_repository_import_item_namespaces;
mod m20260827_000037_repository_import_metadata;
mod m20260828_000038_owner_action_runners;
mod m20260828_000039_action_artifacts;
mod m20260829_000040_unified_repository_identities;
mod m20260829_000041_repository_identity_uuid_storage;
mod m20260829_000042_organization_profiles;
mod m20260829_000043_token_mirror_identities;
mod m20260830_000044_lfs_storage;
mod m20260830_000045_backup_provider_exclusions;
mod m20260830_000046_allow_local_lfs_migration;
mod m20260831_000047_authentication_methods;
mod m20260903_000048_repository_analysis_cache;
mod m20260903_000049_system_action_runners;
mod m20260904_000050_integrity_settings;
mod m20260915_000051_repository_icons;
mod m20260918_000052_default_branch_selection;
mod m20260918_000053_repository_icon_discovery;

mod m20260918_000054_registry_storage;

#[cfg(test)]
mod tests;

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260820_000001_create_instance::Migration),
            Box::new(m20260820_000002_create_identity::Migration),
            Box::new(m20260820_000003_create_repositories::Migration),
            Box::new(m20260820_000004_create_repository_favorites::Migration),
            Box::new(m20260820_000005_add_instance_settings::Migration),
            Box::new(m20260820_000006_create_lfs_locks::Migration),
            Box::new(m20260822_000007_create_oauth_provider::Migration),
            Box::new(m20260822_000008_scope_oauth_repository_tokens::Migration),
            Box::new(m20260822_000009_create_repository_webhooks::Migration),
            Box::new(m20260822_000010_repository_control::Migration),
            Box::new(m20260822_000011_create_repository_topics::Migration),
            Box::new(m20260822_000012_create_instance_assets::Migration),
            Box::new(m20260822_000013_add_user_avatar::Migration),
            Box::new(m20260822_000014_create_repository_releases::Migration),
            Box::new(m20260822_000015_create_repository_issues::Migration),
            Box::new(m20260822_000016_create_issue_attachments::Migration),
            Box::new(m20260822_000017_create_repository_webhook_deliveries::Migration),
            Box::new(m20260823_000018_add_integrations::Migration),
            Box::new(m20260824_000019_repository_integrations::Migration),
            Box::new(m20260824_000020_integration_instances::Migration),
            Box::new(m20260825_000021_account_repository_visibility::Migration),
            Box::new(m20260825_000022_repository_integration_instances::Migration),
            Box::new(m20260825_000023_repository_integration_uuid_storage::Migration),
            Box::new(m20260826_000024_repository_mirrors::Migration),
            Box::new(m20260826_000025_mirror_identities::Migration),
            Box::new(m20260826_000026_harden_mirror_identities::Migration),
            Box::new(m20260826_000027_dokploy_source_bindings::Migration),
            Box::new(m20260827_000028_dokploy_internal_url::Migration),
            Box::new(m20260827_000029_mirror_ssh_key_usage::Migration),
            Box::new(m20260827_000030_backup_settings::Migration),
            Box::new(m20260827_000031_user_theme_preference::Migration),
            Box::new(m20260827_000032_backup_schedules::Migration),
            Box::new(m20260827_000033_backup_providers::Migration),
            Box::new(m20260827_000034_repository_imports::Migration),
            Box::new(m20260827_000035_create_actions::Migration),
            Box::new(m20260827_000036_repository_import_item_namespaces::Migration),
            Box::new(m20260827_000037_repository_import_metadata::Migration),
            Box::new(m20260828_000038_owner_action_runners::Migration),
            Box::new(m20260828_000039_action_artifacts::Migration),
            Box::new(m20260829_000040_unified_repository_identities::Migration),
            Box::new(m20260829_000041_repository_identity_uuid_storage::Migration),
            Box::new(m20260829_000042_organization_profiles::Migration),
            Box::new(m20260829_000043_token_mirror_identities::Migration),
            Box::new(m20260830_000044_lfs_storage::Migration),
            Box::new(m20260830_000045_backup_provider_exclusions::Migration),
            Box::new(m20260830_000046_allow_local_lfs_migration::Migration),
            Box::new(m20260831_000047_authentication_methods::Migration),
            Box::new(m20260903_000048_repository_analysis_cache::Migration),
            Box::new(m20260903_000049_system_action_runners::Migration),
            Box::new(m20260904_000050_integrity_settings::Migration),
            Box::new(m20260915_000051_repository_icons::Migration),
            Box::new(m20260918_000052_default_branch_selection::Migration),
            Box::new(m20260918_000053_repository_icon_discovery::Migration),
            Box::new(m20260918_000054_registry_storage::Migration),
        ]
    }
}
