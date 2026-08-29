import { z } from "zod";

export const sshKeySchema = z.object({
  id: z.guid(),
  name: z.string(),
  fingerprint: z.string(),
  public_key: z.string(),
  created_at: z.string(),
  last_used_at: z.string().nullable(),
});

export const tokenSchema = z.object({
  id: z.guid(),
  name: z.string(),
  scopes: z.array(z.enum(["read", "write", "ssh_keys"])),
  expires_at: z.string().nullable(),
  created_at: z.string(),
  last_used_at: z.string().nullable(),
});

export const createdTokenSchema = z.object({
  token: z.string(),
  details: tokenSchema,
});

export const oauthApplicationSchema = z.object({
  id: z.guid(),
  name: z.string(),
  client_id: z.string(),
  redirect_uri: z.url(),
  created_at: z.string(),
});

export const createdOauthApplicationSchema = z.object({
  client_secret: z.string(),
  application: oauthApplicationSchema,
});

export const passkeySchema = z.object({
  id: z.guid(),
  name: z.string(),
  created_at: z.string(),
  last_used_at: z.string().nullable(),
});

export type SshKey = z.infer<typeof sshKeySchema>;
export type ApiToken = z.infer<typeof tokenSchema>;
export type PasskeySummary = z.infer<typeof passkeySchema>;
export type OauthApplication = z.infer<typeof oauthApplicationSchema>;

export function avatarUrl(userId: string, updatedAt: string | null) {
  return updatedAt
    ? `/api/v1/users/${userId}/avatar?v=${encodeURIComponent(updatedAt)}`
    : null;
}
