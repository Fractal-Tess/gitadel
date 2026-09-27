import { z } from "zod";

export const protectionRuleSchema = z.object({
  id: z.guid(),
  kind: z.enum(["branch", "tag"]),
  pattern: z.string(),
  block_force_push: z.boolean(),
  block_deletion: z.boolean(),
  block_update: z.boolean(),
  restrict_pushes: z.boolean(),
  admins_bypass: z.boolean(),
  allowed_users: z.array(z.string()),
  created_at: z.string(),
  updated_at: z.string(),
});

export const deployKeySchema = z.object({
  id: z.guid(),
  title: z.string(),
  fingerprint: z.string(),
  public_key: z.string(),
  read_only: z.boolean(),
  created_at: z.string(),
  last_used_at: z.string().nullable(),
});

export type ProtectionRule = z.infer<typeof protectionRuleSchema>;
export type ProtectionRuleInput = Partial<
  Omit<ProtectionRule, "id" | "created_at" | "updated_at">
>;
export type DeployKey = z.infer<typeof deployKeySchema>;
