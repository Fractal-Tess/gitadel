import { z } from "zod";

export const organizationSchema = z.object({
  id: z.guid(),
  slug: z.string(),
  display_name: z.string(),
  avatar_updated_at: z.string().nullable(),
  role: z.enum(["owner", "member"]),
});

export const memberSchema = z.object({
  username: z.string(),
  role: z.enum(["owner", "member"]),
  created_at: z.string(),
});

export const memberSuggestionSchema = z.object({
  id: z.guid(),
  username: z.string(),
  avatar_updated_at: z.string().nullable(),
});

export type Organization = z.infer<typeof organizationSchema>;
export type Member = z.infer<typeof memberSchema>;
export type MemberSuggestion = z.infer<typeof memberSuggestionSchema>;

export function organizationAvatarUrl(
  slug: string,
  updatedAt: string | null,
): string | null {
  return updatedAt
    ? `/api/v1/organizations/${encodeURIComponent(slug)}/avatar?v=${encodeURIComponent(updatedAt)}`
    : null;
}
