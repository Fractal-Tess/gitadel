import { z } from "zod";

export const issueUserSchema = z.object({
  id: z.guid(),
  username: z.string(),
  avatar_updated_at: z.string().nullable(),
});

export const externalIssueAuthorSchema = z.object({
  username: z.string(),
  profile_url: z.url(),
});

export const issueLabelSchema = z.object({
  id: z.guid(),
  name: z.string(),
  color: z.string(),
  description: z.string(),
});

export const issueSchema = z.object({
  id: z.guid(),
  number: z.number().int().positive(),
  title: z.string(),
  body: z.string(),
  rendered_body: z.string(),
  state: z.enum(["open", "closed"]),
  author: issueUserSchema,
  assignee: issueUserSchema.nullable(),
  labels: z.array(issueLabelSchema),
  comment_count: z.number().int().nonnegative(),
  created_at: z.string(),
  updated_at: z.string(),
  closed_at: z.string().nullable(),
  external_url: z.url().nullable(),
  external_author: externalIssueAuthorSchema.nullable(),
  can_edit: z.boolean(),
  can_manage: z.boolean(),
});

export const issueCommentSchema = z.object({
  id: z.guid(),
  body: z.string(),
  rendered_body: z.string(),
  author: issueUserSchema,
  created_at: z.string(),
  updated_at: z.string(),
  external_url: z.url().nullable(),
  external_author: externalIssueAuthorSchema.nullable(),
  can_edit: z.boolean(),
});

export const issueAttachmentSchema = z.object({
  id: z.guid(),
  name: z.string(),
  content_type: z.string(),
  size_bytes: z.number().int().nonnegative(),
  created_at: z.string(),
  url: z.string(),
});

export type Issue = z.infer<typeof issueSchema>;
export type IssueUser = z.infer<typeof issueUserSchema>;
export type IssueComment = z.infer<typeof issueCommentSchema>;
export type IssueLabel = z.infer<typeof issueLabelSchema>;
export type IssueAttachment = z.infer<typeof issueAttachmentSchema>;
