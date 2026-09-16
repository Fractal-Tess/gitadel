import { z } from "zod";

export const storageTargetKindSchema = z.enum(["filesystem", "s3"]);
export type StorageTargetKind = z.infer<typeof storageTargetKindSchema>;

const filesystemConfigurationSchema = z.object({
  kind: z.literal("filesystem"),
  path: z.string().optional(),
});

const s3ConfigurationSchema = z.object({
  kind: z.literal("s3"),
  s3: z.object({
    endpoint: z.string(),
    bucket: z.string(),
    access_key: z.string(),
    secret_key: z.string(),
    region: z.string(),
    prefix: z.string(),
  }),
});

// Object stores publish no capacity, so only filesystem targets carry this.
export const storageTargetCapacitySchema = z.object({
  total_bytes: z.number().nonnegative(),
  available_bytes: z.number().nonnegative(),
});

export const measuredUsageSchema = z.object({
  object_count: z.number().nonnegative(),
  total_bytes: z.number().nonnegative(),
  measured_at: z.string(),
});
export type MeasuredUsage = z.infer<typeof measuredUsageSchema>;

export const storageTargetUsageSchema = z.object({
  lfs_object_count: z.number().nonnegative(),
  lfs_bytes: z.number().nonnegative(),
  measured: measuredUsageSchema.nullable(),
});

export const storageTargetSchema = z.object({
  id: z.uuid(),
  name: z.string(),
  kind: storageTargetKindSchema,
  configuration: z.union([
    filesystemConfigurationSchema,
    s3ConfigurationSchema,
  ]),
  active: z.boolean(),
  managed_by_config: z.boolean(),
  capacity: storageTargetCapacitySchema.nullable(),
  usage: storageTargetUsageSchema,
});
export type StorageTarget = z.infer<typeof storageTargetSchema>;

export const storageTargetsSchema = z.array(storageTargetSchema);

export const lfsStorageStatusSchema = z.object({
  object_count: z.number().nonnegative(),
  total_bytes: z.number().nonnegative(),
});
export type LfsStorageStatus = z.infer<typeof lfsStorageStatusSchema>;

export const storageTargetTestSchema = z.object({
  message: z.string(),
});

export const storageMigrationScheduledSchema = z.object({
  operation_id: z.uuid(),
  target_id: z.uuid(),
  message: z.string(),
});

export const storageMigrationProgressSchema = z.object({
  operation_id: z.uuid(),
  key: z.string(),
  operation: z.literal("lfs_migrate"),
  phase: z.enum([
    "scheduled",
    "checking_destination",
    "copying_lfs",
    "writing_metadata",
    "completed",
    "failed",
  ]),
  message: z.string(),
  processed_bytes: z.number().nullable(),
  total_bytes: z.number().nullable(),
});
export type StorageMigrationProgress = z.infer<
  typeof storageMigrationProgressSchema
>;

export const lfsRepositoryUsageSchema = z.object({
  repository_id: z.uuid(),
  repository_name: z.string(),
  owner_name: z.string(),
  owner_type: z.enum(["user", "organization"]),
  object_count: z.number().nonnegative(),
  total_bytes: z.number().nonnegative(),
});
export type LfsRepositoryUsage = z.infer<typeof lfsRepositoryUsageSchema>;

export const lfsRepositoryUsageResponseSchema = z.object({
  repositories: z.array(lfsRepositoryUsageSchema),
  total: z.number().nonnegative(),
  limit: z.number().int().positive(),
  offset: z.number().int().nonnegative(),
});
export type LfsRepositoryUsageResponse = z.infer<
  typeof lfsRepositoryUsageResponseSchema
>;
