import { z } from "zod";

export const backupProviderKindSchema = z.enum(["filesystem", "s3"]);
export type BackupProviderKind = z.infer<typeof backupProviderKindSchema>;

export const backupProviderCatalogItemSchema = z.object({
  slug: backupProviderKindSchema,
  name: z.string(),
  description: z.string(),
});
export type BackupProviderCatalogItem = z.infer<
  typeof backupProviderCatalogItemSchema
>;

export const backupProviderSchema = z.object({
  id: z.uuid(),
  name: z.string(),
  provider: backupProviderKindSchema,
  managed_by_config: z.boolean(),
  managed_by_storage: z.boolean(),
  path: z.string().nullable(),
  endpoint: z.string().nullable(),
  bucket: z.string().nullable(),
  access_key_hint: z.string().nullable(),
  region: z.string().nullable(),
  prefix: z.string().nullable(),
  schedule: z.string().nullable(),
  next_backup_at: z.string().nullable(),
});
export type BackupProvider = z.infer<typeof backupProviderSchema>;

export const backupProvidersSchema = z.object({
  providers: z.array(backupProviderCatalogItemSchema),
  connections: z.array(backupProviderSchema),
});

export const backupProviderTestSchema = z.object({
  test_token: z.uuid(),
  message: z.string(),
});

export const backupSnapshotSchema = z.object({
  key: z.string(),
  name: z.string().nullable(),
  gitadel_version: z.string().nullable(),
  size: z.number(),
  created_at: z.string(),
});
export type BackupSnapshot = z.infer<typeof backupSnapshotSchema>;

export const backupScheduledSchema = z.object({
  key: z.string(),
  operation_id: z.uuid(),
  message: z.string(),
});

export const backupProgressSchema = z.object({
  operation_id: z.uuid(),
  key: z.string(),
  operation: z.enum(["create", "restore"]),
  phase: z.enum([
    "scheduled",
    "checking_destination",
    "snapshotting_database",
    "copying_repositories",
    "copying_lfs",
    "writing_metadata",
    "compressing",
    "uploading",
    "restoring",
    "completed",
    "failed",
  ]),
  message: z.string(),
  processed_bytes: z.number().nullable(),
  total_bytes: z.number().nullable(),
});
export type BackupProgress = z.infer<typeof backupProgressSchema>;

export const restorePreflightSchema = z.object({
  token: z.uuid(),
  key: z.string(),
  format_version: z.number(),
  gitadel_version: z.string().nullable(),
  backup_name: z.string().nullable(),
  version_warning: z.string().nullable(),
  created_at: z.string(),
  file_count: z.number(),
  uncompressed_size: z.number(),
  includes_settings: z.boolean(),
  includes_host_key: z.boolean(),
  required_free_space: z.number(),
  available_free_space: z.number(),
});
export type RestorePreflight = z.infer<typeof restorePreflightSchema>;
