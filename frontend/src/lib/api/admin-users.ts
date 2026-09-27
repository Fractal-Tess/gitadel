import { z } from "zod";

export const adminUserSchema = z.object({
  id: z.guid(),
  username: z.string(),
  is_admin: z.boolean(),
  disabled_at: z.string().nullable(),
  avatar_updated_at: z.string().nullable(),
  two_factor_enabled: z.boolean(),
  created_at: z.string(),
});

export const adminUserPageSchema = z.object({
  users: z.array(adminUserSchema),
  total: z.number().int().nonnegative(),
});

export const pendingInvitationSchema = z.object({
  id: z.string(),
  created_by: z.string().nullable(),
  created_at: z.string(),
  expires_at: z.string(),
});

export type AdminUser = z.infer<typeof adminUserSchema>;
export type AdminUserPage = z.infer<typeof adminUserPageSchema>;
export type PendingInvitation = z.infer<typeof pendingInvitationSchema>;

export function adminUserApi(username: string, action = ""): string {
  return `/api/v1/admin/users/${encodeURIComponent(username)}${action}`;
}
