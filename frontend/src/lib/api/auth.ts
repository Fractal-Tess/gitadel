import { z } from "zod";

export const themePreferenceSchema = z.enum(["system", "light", "dark"]);
export type ThemePreference = z.infer<typeof themePreferenceSchema>;

const userSchema = z.object({
  id: z.guid(),
  username: z.string(),
  is_admin: z.boolean(),
  default_repository_visibility: z.enum(["public", "private"]),
  theme_preference: themePreferenceSchema,
  avatar_updated_at: z.string().nullable(),
});

export const authStatusSchema = z.object({
  setup_required: z.boolean(),
  authenticated: z.boolean(),
  user: userSchema.nullable(),
});

export const authResponseSchema = z.object({ user: userSchema });

export const webauthnCreationSchema = z.object({
  challenge_id: z.string(),
  options: z.object({ publicKey: z.record(z.string(), z.unknown()) }),
});

export const webauthnRequestSchema = z.object({
  challenge_id: z.string(),
  options: z.object({ publicKey: z.record(z.string(), z.unknown()) }),
});

export type AuthStatus = z.infer<typeof authStatusSchema>;
export type User = z.infer<typeof userSchema>;
