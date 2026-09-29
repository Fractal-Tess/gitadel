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
  registry_object_count: z.number().nonnegative(),
  registry_bytes: z.number().nonnegative(),
  measured: measuredUsageSchema.nullable(),
});

export type StorageTargetUsage = z.infer<typeof storageTargetUsageSchema>;

export const storageTargetSchema = z.object({
  id: z.uuid(),
  name: z.string(),
  kind: storageTargetKindSchema,
  configuration: z.union([
    filesystemConfigurationSchema,
    s3ConfigurationSchema,
  ]),
  active: z.boolean(),
  registry_active: z.boolean(),
  managed_by_config: z.boolean(),
  capacity: storageTargetCapacitySchema.nullable(),
  usage: storageTargetUsageSchema,
});

export type StorageTarget = z.infer<typeof storageTargetSchema>;

export const storageTargetsSchema = z.array(storageTargetSchema);

export const storageTargetTestSchema = z.object({
  message: z.string(),
});

/** The nil target id, which selects a domain's local storage. */
export const LOCAL_STORAGE_TARGET_ID = "00000000-0000-0000-0000-000000000000";

export const storageUsageDetailSchema = z.object({
  key: z.string(),
  label: z.string(),
  unit: z.enum(["count", "bytes"]),
  per_repository: z.boolean(),
});
export type StorageUsageDetail = z.infer<typeof storageUsageDetailSchema>;

const usageDetailsSchema = z.record(z.string(), z.number().nonnegative());

export const storageDomainMigrationSchema = z.object({
  operation_id: z.uuid(),
  domain: z.string(),
  operation: z.string(),
  source_target_id: z.uuid().nullable(),
  target_id: z.uuid().nullable(),
  state: z.string(),
  phase: z.string(),
  message: z.string(),
  key: z.string(),
  copied_objects: z.number().nonnegative(),
  processed_bytes: z.number().nonnegative(),
  total_bytes: z.number().nonnegative().nullable(),
  error: z.string().nullable(),
  started_at: z.string(),
  updated_at: z.string(),
  completed_at: z.string().nullable(),
});
export type StorageDomainMigration = z.infer<
  typeof storageDomainMigrationSchema
>;

export const storageDomainStatusSchema = z.object({
  name: z.string(),
  label: z.string(),
  active_target_id: z.uuid().nullable(),
  active_target_name: z.string().nullable(),
  local_label: z.string(),
  local_root: z.string(),
  usage: z.object({
    repository_count: z.number().nonnegative(),
    object_count: z.number().nonnegative(),
    total_bytes: z.number().nonnegative(),
    details: usageDetailsSchema,
  }),
  detail_fields: z.array(storageUsageDetailSchema),
  active_migration: storageDomainMigrationSchema.nullable(),
  last_migration: storageDomainMigrationSchema.nullable(),
});
export type StorageDomainStatus = z.infer<typeof storageDomainStatusSchema>;

export const storageDomainsSchema = z.array(storageDomainStatusSchema);

export const storageDomainRepositoryUsageSchema = z.object({
  repository_id: z.uuid(),
  repository_name: z.string(),
  owner_name: z.string(),
  owner_type: z.enum(["user", "organization"]),
  object_count: z.number().nonnegative(),
  total_bytes: z.number().nonnegative(),
  details: usageDetailsSchema,
});
export type StorageDomainRepositoryUsage = z.infer<
  typeof storageDomainRepositoryUsageSchema
>;

export const storageDomainRepositoryUsagePageSchema = z.object({
  repositories: z.array(storageDomainRepositoryUsageSchema),
  total: z.number().nonnegative(),
  limit: z.number().int().positive(),
  offset: z.number().int().nonnegative(),
});
export type StorageDomainRepositoryUsagePage = z.infer<
  typeof storageDomainRepositoryUsagePageSchema
>;

export const storageDomainMigrationStartedSchema = z.object({
  operation_id: z.uuid(),
  domain: z.string(),
  target_id: z.uuid(),
  local: z.boolean(),
  message: z.string(),
});

export type StorageUsageFilters = {
  search: string;
  owner: string;
  ownerType: string;
  minBytes: string;
  maxBytes: string;
  sort: string;
};

/** API path of a storage domain, optionally followed by more segments. */
export function storageDomainPath(domain: string, ...segments: string[]) {
  return ["/api/v1/admin/storage/domains", domain, ...segments]
    .map((segment, index) =>
      index === 0 ? segment : encodeURIComponent(segment),
    )
    .join("/");
}

/** The repository usage page request for `filters`. */
export function storageDomainRepositoriesPath(
  domain: string,
  filters: StorageUsageFilters,
  offset: number,
  limit = 10,
) {
  const parameters = new URLSearchParams({
    limit: String(limit),
    offset: String(offset),
    sort: filters.sort,
  });
  const optional: [string, string][] = [
    ["search", filters.search],
    ["owner", filters.owner],
    ["owner_type", filters.ownerType],
    ["min_bytes", filters.minBytes],
    ["max_bytes", filters.maxBytes],
  ];
  for (const [name, value] of optional) {
    if (value.trim()) parameters.set(name, value.trim());
  }
  return `${storageDomainPath(domain, "repositories")}?${parameters}`;
}
