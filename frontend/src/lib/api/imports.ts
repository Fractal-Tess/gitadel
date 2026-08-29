import { z } from "zod";

export const remoteImportRepositorySchema = z.object({
  id: z.string(),
  name: z.string(),
  full_name: z.string(),
  description: z.string().nullable(),
  web_url: z.string(),
  clone_url: z.string(),
  visibility: z.string(),
  archived: z.boolean(),
  fork: z.boolean(),
  default_branch: z.string().nullable(),
});
export type RemoteImportRepository = z.infer<
  typeof remoteImportRepositorySchema
>;

export const importDiscoverySchema = z.object({
  provider: z.enum(["github", "gitlab", "gitea", "forgejo"]),
  instance_url: z.string(),
  account: z.string(),
  repositories: z.array(remoteImportRepositorySchema),
});
export type ImportDiscovery = z.infer<typeof importDiscoverySchema>;

export const repositoryImportItemSchema = z.object({
  id: z.uuid(),
  source_id: z.string(),
  source_full_name: z.string(),
  source_web_url: z.string(),
  target_namespace: z.string(),
  target_name: z.string(),
  target_visibility: z.enum(["public", "private"]),
  state: z.enum([
    "queued",
    "cloning",
    "metadata",
    "completed",
    "failed",
    "cancelled",
    "credentials_required",
  ]),
  attempts: z.number(),
  repository_id: z.uuid().nullable(),
  last_error: z.string().nullable(),
});
export type RepositoryImportItem = z.infer<typeof repositoryImportItemSchema>;

export const repositoryImportSchema = z.object({
  id: z.uuid(),
  provider: z.enum(["github", "gitlab", "gitea", "forgejo"]),
  instance_url: z.string(),
  target_namespace: z.string(),
  state: z.enum([
    "queued",
    "running",
    "completed",
    "completed_with_errors",
    "cancelled",
    "credentials_required",
  ]),
  created_at: z.string(),
  updated_at: z.string(),
  items: z.array(repositoryImportItemSchema),
});
export type RepositoryImport = z.infer<typeof repositoryImportSchema>;
