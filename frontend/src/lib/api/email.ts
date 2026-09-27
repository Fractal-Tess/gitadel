import { z } from "zod";

export const emailStatusSchema = z.object({
  email_enabled: z.boolean(),
  email: z.string().nullable(),
  verified: z.boolean(),
});

export const smtpStatusSchema = z.object({
  configured: z.boolean(),
  host: z.string().optional(),
  port: z.number().int().optional(),
  tls: z.enum(["starttls", "tls", "none"]).optional(),
  from: z.string().optional(),
  username: z.string().nullable().optional(),
});

export const testEmailSchema = z.object({ delivered_to: z.string() });

export const passwordResetAcceptedSchema = z.object({ message: z.string() });

export const notificationPreferencesSchema = z.object({
  email_enabled: z.boolean(),
  issues: z.boolean(),
  issue_comments: z.boolean(),
  action_failures: z.boolean(),
});

export type NotificationPreferences = z.infer<
  typeof notificationPreferencesSchema
>;
export type EmailStatus = z.infer<typeof emailStatusSchema>;
export type SmtpStatus = z.infer<typeof smtpStatusSchema>;
