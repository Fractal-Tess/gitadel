import { z } from "zod";

export const instanceSettingsSchema = z.object({
  site_name: z.string(),
  site_description: z.string().nullable(),
  updated_at: z.string(),
});

export const integritySettingsSchema = z.object({
  enabled: z.boolean(),
  schedule: z.string(),
  last_checked_at: z.string().nullable(),
  last_result: z.string().nullable(),
});

export const changelogSchema = z.object({
  application_version: z.string(),
  rendered_html: z.string(),
});

export const invitationSchema = z.object({
  token: z.string(),
  expires_at: z.string(),
});

export const auditEventSchema = z.object({
  id: z.number(),
  actor_user_id: z.guid().nullable(),
  actor_username: z.string().nullable(),
  action: z.string(),
  target: z.string().nullable(),
  created_at: z.string(),
});

export type InstanceSettings = z.infer<typeof instanceSettingsSchema>;
export type IntegritySettings = z.infer<typeof integritySettingsSchema>;
export type Changelog = z.infer<typeof changelogSchema>;

export type AuditEvent = z.infer<typeof auditEventSchema>;
