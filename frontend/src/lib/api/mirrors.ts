import { z } from "zod";

export const identityUsageSchema = z.object({
  repository: z.string(),
  last_attempted_at: z.string().nullable(),
  last_synced_at: z.string().nullable(),
  last_error: z.string().nullable(),
});

export const mirrorIdentitySchema = z.object({
  id: z.uuid(),
  name: z.string(),
  kind: z.enum(["basic", "token", "ssh"]),
  username: z.string().nullable(),
  provider: z.enum(["github", "gitlab", "gitea", "forgejo"]).nullable(),
  instance_url: z.string().nullable(),
  public_key: z.string().nullable(),
  fingerprint: z.string().nullable(),
  last_used_at: z.string().nullable(),
  usage_history: z.array(identityUsageSchema),
  created_at: z.string(),
  updated_at: z.string(),
});

export const mirrorIdentitiesSchema = z.array(mirrorIdentitySchema);

export const repositoryMirrorSchema = z.object({
  remote_url: z.string(),
  identity_id: z.uuid().nullable(),
  credential_configured: z.boolean(),
  schedule: z.string().nullable(),
  last_attempted_at: z.string().nullable(),
  last_synced_at: z.string().nullable(),
  last_error: z.string().nullable(),
  metadata_last_synced_at: z.string().nullable(),
  metadata_error: z.string().nullable(),
  next_sync_at: z.string().nullable(),
  syncing: z.boolean(),
});

export type MirrorIdentity = z.infer<typeof mirrorIdentitySchema>;
export type RepositoryMirror = z.infer<typeof repositoryMirrorSchema>;
