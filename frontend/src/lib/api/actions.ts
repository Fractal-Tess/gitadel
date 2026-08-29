import { z } from "zod";

export const actionStatusSchema = z.enum([
  "queued",
  "running",
  "success",
  "failure",
  "cancelled",
  "skipped",
]);

export const actionRunSummarySchema = z.object({
  id: z.uuid(),
  number: z.number(),
  workflow_name: z.string(),
  workflow_path: z.string(),
  status: actionStatusSchema,
  failure_kind: z.string().nullable(),
  failure_summary: z.string().nullable(),
  reference: z.string(),
  before_sha: z.string(),
  after_sha: z.string(),
  created_at: z.string(),
  started_at: z.string().nullable(),
  completed_at: z.string().nullable(),
});

export const actionRunsSchema = z.object({
  runs: z.array(actionRunSummarySchema),
  page: z.number(),
  per_page: z.number(),
  has_more: z.boolean(),
});

export const actionJobSchema = z.object({
  id: z.number(),
  key: z.string(),
  name: z.string(),
  status: actionStatusSchema,
  result: z.string().nullable(),
  labels: z.array(z.string()),
  runner_id: z.number().nullable(),
  attempt: z.number(),
  failure_kind: z.string().nullable(),
  failure_summary: z.string().nullable(),
  created_at: z.string(),
  started_at: z.string().nullable(),
  completed_at: z.string().nullable(),
});

export const actionRunDetailSchema = z.object({
  run: actionRunSummarySchema,
  jobs: z.array(actionJobSchema),
  can_cancel: z.boolean(),
  diagnostic: z.string().nullable(),
});

export const actionLogsSchema = z.object({
  text: z.string(),
  next_cursor: z.string().nullable(),
  complete: z.boolean(),
});

export const actionArtifactSchema = z.object({
  id: z.number().int().positive(),
  name: z.string(),
  size_bytes: z.number().int().nonnegative(),
  sha256: z.string().nullable(),
  creating_job_id: z.number().int().positive(),
  created_at: z.string(),
  expires_at: z.string(),
  download_url: z.string(),
});
export const actionArtifactsSchema = z.object({
  artifacts: z.array(actionArtifactSchema),
});

export const actionCommitStatusSchema = z.object({
  oid: z.string(),
  status: actionStatusSchema.exclude(["skipped"]),
  total: z.number(),
  success: z.number(),
  failure: z.number(),
  running: z.number(),
  queued: z.number(),
  cancelled: z.number(),
});

export const actionStatusesSchema = z.object({
  statuses: z.array(actionCommitStatusSchema),
});

export const actionRunnerSchema = z.object({
  id: z.number(),
  name: z.string(),
  labels: z.array(z.string()),
  version: z.string(),
  status: z.enum(["online", "offline"]),
  last_seen_at: z.string().nullable(),
  incompatibility: z.string().nullable(),
});

export const actionRunnersSchema = z.object({
  runners: z.array(actionRunnerSchema),
});

export const actionRegistrationSchema = z.object({
  token: z.string(),
  expires_at: z.string(),
  server_url: z.string(),
  required_version: z.string(),
  runner_name: z.string(),
  labels: z.array(z.string()),
});

export type ActionStatus = z.infer<typeof actionStatusSchema>;
export type ActionRunSummary = z.infer<typeof actionRunSummarySchema>;
export type ActionRuns = z.infer<typeof actionRunsSchema>;
export type ActionJob = z.infer<typeof actionJobSchema>;
export type ActionRunDetail = z.infer<typeof actionRunDetailSchema>;
export type ActionLogs = z.infer<typeof actionLogsSchema>;
export type ActionArtifact = z.infer<typeof actionArtifactSchema>;
export type ActionCommitStatus = z.infer<typeof actionCommitStatusSchema>;
export type ActionRunner = z.infer<typeof actionRunnerSchema>;
export type ActionRegistration = z.infer<typeof actionRegistrationSchema>;
