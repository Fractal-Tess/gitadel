import { z } from "zod";

export class ApiFailure extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
  ) {
    super(message);
  }
}

const errorSchema = z.object({
  error: z.object({
    code: z.string(),
    message: z.string(),
  }),
});

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

export const instanceSettingsSchema = z.object({
  site_name: z.string(),
  site_description: z.string().nullable(),
  updated_at: z.string(),
});

export const backupProviderKindSchema = z.enum(["filesystem", "s3"]);
export type BackupProviderKind = z.infer<typeof backupProviderKindSchema>;

export const backupProviderCatalogItemSchema = z.object({
  slug: backupProviderKindSchema,
  name: z.string(),
  description: z.string(),
});
export type BackupProviderCatalogItem = z.infer<
  typeof backupProviderCatalogItemSchema
>;

export const backupProviderSchema = z.object({
  id: z.uuid(),
  name: z.string(),
  provider: backupProviderKindSchema,
  managed_by_config: z.boolean(),
  path: z.string().nullable(),
  endpoint: z.string().nullable(),
  bucket: z.string().nullable(),
  access_key_hint: z.string().nullable(),
  region: z.string().nullable(),
  prefix: z.string().nullable(),
  schedule: z.string().nullable(),
  next_backup_at: z.string().nullable(),
});
export type BackupProvider = z.infer<typeof backupProviderSchema>;

export const backupProvidersSchema = z.object({
  providers: z.array(backupProviderCatalogItemSchema),
  connections: z.array(backupProviderSchema),
});

export const backupProviderTestSchema = z.object({
  test_token: z.uuid(),
  message: z.string(),
});

export const backupSnapshotSchema = z.object({
  key: z.string(),
  name: z.string().nullable(),
  gitadel_version: z.string().nullable(),
  size: z.number(),
  created_at: z.string(),
});
export type BackupSnapshot = z.infer<typeof backupSnapshotSchema>;

export const backupScheduledSchema = z.object({
  key: z.string(),
  operation_id: z.uuid(),
  message: z.string(),
});

export const backupProgressSchema = z.object({
  operation_id: z.uuid(),
  key: z.string(),
  operation: z.enum(["create", "restore"]),
  phase: z.enum([
    "scheduled",
    "checking_destination",
    "snapshotting_database",
    "copying_repositories",
    "copying_lfs",
    "writing_metadata",
    "compressing",
    "uploading",
    "restoring",
    "completed",
    "failed",
  ]),
  message: z.string(),
  processed_bytes: z.number().nullable(),
  total_bytes: z.number().nullable(),
});
export type BackupProgress = z.infer<typeof backupProgressSchema>;

export const restorePreflightSchema = z.object({
  token: z.uuid(),
  key: z.string(),
  format_version: z.number(),
  gitadel_version: z.string().nullable(),
  backup_name: z.string().nullable(),
  version_warning: z.string().nullable(),
  created_at: z.string(),
  file_count: z.number(),
  uncompressed_size: z.number(),
  includes_settings: z.boolean(),
  includes_host_key: z.boolean(),
  required_free_space: z.number(),
  available_free_space: z.number(),
});
export type RestorePreflight = z.infer<typeof restorePreflightSchema>;

export const integrationProviderSchema = z.object({
  slug: z.string(),
  name: z.string(),
  description: z.string(),
  icon: z.string(),
  source_required: z.boolean(),
});
export type IntegrationProvider = z.infer<typeof integrationProviderSchema>;
export const integrationSourceSummarySchema = z.object({
  id: z.string(),
  name: z.string(),
  managed: z.boolean(),
  ready: z.boolean().optional(),
});
export const namespaceIntegrationSchema = z.object({
  id: z.uuid(),
  provider: z.string(),
  provider_name: z.string(),
  name: z.string(),
  enabled: z.boolean(),
  url: z.string(),
  internal_url: z.string(),
  api_key_set: z.boolean(),
  source: integrationSourceSummarySchema.optional(),
});
export type NamespaceIntegration = z.infer<typeof namespaceIntegrationSchema>;
export const integrationCredentialSchema = z.object({
  api_key: z.string(),
});
export const namespaceIntegrationsSchema = z.object({
  namespace: z.string(),
  providers: z.array(integrationProviderSchema),
  integrations: z.array(namespaceIntegrationSchema),
});

export const integrationRemoteSourceSchema = z.object({
  id: z.string(),
  name: z.string(),
  ready: z.boolean(),
});

export const integrationSourceBindingSchema = z.object({
  id: z.string(),
  name: z.string(),
  managed: z.boolean(),
  ready: z.boolean(),
  authorization_url: z.url().optional(),
});

export const integrationSourceConnectionSchema = z.object({
  required: z.boolean(),
  binding: integrationSourceBindingSchema.nullable(),
  sources: z.array(integrationRemoteSourceSchema),
});
export type IntegrationSourceConnection = z.infer<
  typeof integrationSourceConnectionSchema
>;

export const changelogSchema = z.object({
  application_version: z.string(),
  rendered_html: z.string(),
});

export const invitationSchema = z.object({
  token: z.string(),
  expires_at: z.string(),
});

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

export const auditEventSchema = z.object({
  id: z.number(),
  actor_user_id: z.guid().nullable(),
  actor_username: z.string().nullable(),
  action: z.string(),
  target: z.string().nullable(),
  created_at: z.string(),
});

export const webauthnCreationSchema = z.object({
  challenge_id: z.string(),
  options: z.object({ publicKey: z.record(z.string(), z.unknown()) }),
});

export const webauthnRequestSchema = z.object({
  challenge_id: z.string(),
  options: z.object({ publicKey: z.record(z.string(), z.unknown()) }),
});

export const repositorySchema = z.object({
  id: z.guid(),
  namespace: z.string(),
  name: z.string(),
  description: z.string().nullable(),
  visibility: z.enum(["public", "private"]),
  object_format: z.enum(["sha1", "sha256"]),
  mirrored: z.boolean(),
  default_branch: z.string(),
  archived_at: z.string().nullable(),
  created_at: z.string(),
  updated_at: z.string(),
  favorited: z.boolean(),
  ssh_clone_url: z.string(),
  can_manage: z.boolean(),
});

export const remoteImportRepositorySchema = z.object({
  id: z.string(),
  name: z.string(),
  full_name: z.string(),
  description: z.string().nullable(),
  web_url: z.string(),
  clone_url: z.string(),
  visibility: z.string(),
  archived: z.boolean(),
  fork: z.boolean(),
  default_branch: z.string().nullable(),
});
export type RemoteImportRepository = z.infer<
  typeof remoteImportRepositorySchema
>;

export const importDiscoverySchema = z.object({
  provider: z.enum(["github", "gitlab", "gitea", "forgejo"]),
  instance_url: z.string(),
  account: z.string(),
  repositories: z.array(remoteImportRepositorySchema),
});
export type ImportDiscovery = z.infer<typeof importDiscoverySchema>;

export const repositoryImportItemSchema = z.object({
  id: z.uuid(),
  source_id: z.string(),
  source_full_name: z.string(),
  source_web_url: z.string(),
  target_namespace: z.string(),
  target_name: z.string(),
  target_visibility: z.enum(["public", "private"]),
  state: z.enum([
    "queued",
    "cloning",
    "metadata",
    "completed",
    "failed",
    "cancelled",
    "credentials_required",
  ]),
  attempts: z.number(),
  repository_id: z.uuid().nullable(),
  last_error: z.string().nullable(),
});
export type RepositoryImportItem = z.infer<typeof repositoryImportItemSchema>;

export const repositoryImportSchema = z.object({
  id: z.uuid(),
  provider: z.enum(["github", "gitlab", "gitea", "forgejo"]),
  instance_url: z.string(),
  target_namespace: z.string(),
  state: z.enum([
    "queued",
    "running",
    "completed",
    "completed_with_errors",
    "cancelled",
    "credentials_required",
  ]),
  created_at: z.string(),
  updated_at: z.string(),
  items: z.array(repositoryImportItemSchema),
});
export type RepositoryImport = z.infer<typeof repositoryImportSchema>;

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

export const repositoryIntegrationConnectionSchema = z.object({
  id: z.uuid(),
  provider: z.string(),
  provider_name: z.string(),
  name: z.string(),
  configured: z.boolean(),
});

export const repositoryIntegrationSchema = z.object({
  id: z.uuid(),
  connection_id: z.uuid(),
  provider: z.string(),
  provider_name: z.string(),
  name: z.string(),
  connection_name: z.string(),
  configured: z.boolean(),
  repository_setup: z.boolean(),
  enabled: z.boolean(),
  resource: z.unknown().optional(),
  config: z.unknown().optional(),
});

export const repositoryIntegrationsSchema = z.object({
  integrations: z.array(repositoryIntegrationSchema),
  connections: z.array(repositoryIntegrationConnectionSchema),
  providers: z.array(integrationProviderSchema),
});

export const integrationTestResultSchema = z.object({
  account: z.string().nullable(),
  warnings: z.array(z.string()),
});
export type IntegrationTestResult = z.infer<typeof integrationTestResultSchema>;

export const dokployResourceKindSchema = z.enum(["application", "compose"]);

export const remoteResourceSchema = z.object({
  kind: dokployResourceKindSchema,
  id: z.string(),
  name: z.string(),
  status: z.string().nullable(),
  server_name: z.string().nullable(),
});
export type RemoteResource = z.infer<typeof remoteResourceSchema>;

export const dokployEnvironmentSchema = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string().nullable(),
  resources: z.array(remoteResourceSchema),
});
export type DokployEnvironment = z.infer<typeof dokployEnvironmentSchema>;

export const dokployProjectSchema = z.object({
  id: z.string(),
  name: z.string(),
  environments: z.array(dokployEnvironmentSchema),
});
export type DokployProject = z.infer<typeof dokployProjectSchema>;

export const dokployRemoteCatalogSchema = z.object({
  external_url: z.url(),
  projects: z.array(dokployProjectSchema),
  servers: z.array(z.object({ id: z.string(), name: z.string() })),
});
export type DokployRemoteCatalog = z.infer<typeof dokployRemoteCatalogSchema>;

export const dokployCreatedEnvironmentSchema = dokployEnvironmentSchema.extend({
  project_id: z.string(),
  project_name: z.string(),
});

export const dokployResourceLinkSchema = z.object({
  kind: dokployResourceKindSchema,
  id: z.string(),
  name: z.string(),
  branch: z.string(),
  project_id: z.string(),
  project_name: z.string(),
  environment_id: z.string(),
  environment_name: z.string(),
  external_url: z.url(),
});
export type DokployResourceLink = z.infer<typeof dokployResourceLinkSchema>;

export const integrationDeployResultSchema = z.object({
  summary: z.string(),
});

export const topicsSchema = z.object({
  topics: z.array(z.string()),
});

export const webhookSchema = z.object({
  id: z.guid(),
  type: z.literal("Repository"),
  name: z.literal("web"),
  active: z.boolean(),
  events: z.tuple([z.literal("push")]),
  config: z.object({
    url: z.url(),
    content_type: z.literal("json"),
    insecure_ssl: z.literal("0"),
  }),
  url: z.url(),
  ping_url: z.url(),
  created_at: z.string(),
  updated_at: z.string(),
  last_delivery_at: z.string().nullable(),
  last_response: z.object({
    code: z.number().int().nullable(),
    status: z.enum(["ok", "failed", "unused"]),
    message: z.string().nullable(),
  }),
});

export const webhookDeliverySchema = z.object({
  id: z.guid(),
  event: z.string(),
  status_code: z.number().int().nullable(),
  status: z.enum(["ok", "failed"]),
  delivered_at: z.string(),
  duration_ms: z.number().int(),
  payload: z.unknown(),
  response_body: z.string().nullable(),
});

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

export const renderedMarkdownSchema = z.object({
  rendered_html: z.string(),
});

export const releaseAssetSchema = z.object({
  id: z.guid(),
  name: z.string(),
  content_type: z.string(),
  size_bytes: z.number().int().nonnegative(),
  download_count: z.number().int().nonnegative(),
  created_at: z.string(),
  download_url: z.string(),
  external_url: z.string().nullable(),
});

export const releaseSchema = z.object({
  id: z.guid(),
  target_revision: z.string(),
  target_oid: z.string(),
  title: z.string(),
  body: z.string(),
  rendered_body: z.string(),
  prerelease: z.boolean(),
  latest: z.boolean(),
  author: z.string(),
  published_at: z.string(),
  created_at: z.string(),
  updated_at: z.string(),
  assets: z.array(releaseAssetSchema),
  external_url: z.string().nullable(),
  external_author: z
    .object({ username: z.string(), profile_url: z.string().nullable() })
    .nullable(),
});

export const repositoryActivitySchema = z.object({
  start_date: z.iso.date(),
  end_date: z.iso.date(),
  total_commits: z.number().int().nonnegative(),
  days: z.array(
    z.object({
      date: z.iso.date(),
      count: z.number().int().positive(),
    }),
  ),
});

export const repositoryOverviewItemSchema = repositorySchema.extend({
  branch_count: z.number().int().nonnegative(),
  total_lines: z.number().int().nonnegative(),
  languages: z.array(
    z.object({
      language: z.string(),
      lines: z.number().int().nonnegative(),
    }),
  ),
  activity: repositoryActivitySchema,
});

export const repositoryOverviewSchema = z.object({
  repositories: z.array(repositoryOverviewItemSchema),
  page: z.number().int().positive(),
  per_page: z.number().int().positive(),
  has_next: z.boolean(),
});

export const refSchema = z.object({
  name: z.string(),
  oid: z.string(),
  commit_oid: z.string(),
});

export const refsSchema = z.object({
  branches: z.array(refSchema),
  tags: z.array(refSchema),
  size_bytes: z.number().int().nonnegative().nullable(),
});

export const treeEntrySchema = z.object({
  name: z.string(),
  path: z.string(),
  oid: z.string(),
  kind: z.enum(["tree", "blob", "symlink", "submodule"]),
  mode: z.number(),
  size: z.number().nullable(),
});

export const treeSchema = z.object({
  revision: z.string(),
  commit_oid: z.string(),
  commit_count: z.number().int().nonnegative().nullable(),
  path: z.string(),
  entries: z.array(treeEntrySchema),
});

export const blobSchema = z.object({
  revision: z.string(),
  commit_oid: z.string(),
  path: z.string(),
  oid: z.string(),
  size: z.number(),
  binary: z.boolean(),
  too_large: z.boolean(),
  content: z.string().nullable(),
  rendered_html: z.string().nullable(),
});

export const signatureSchema = z.object({
  name: z.string(),
  email: z.string(),
  timestamp: z.number(),
  timezone_offset_minutes: z.number(),
});
export const commitVerificationSchema = z.object({
  verified: z.boolean(),
  reason: z.enum(["verified", "unknown_key", "invalid"]),
  signer: z.string().nullable(),
  fingerprint: z.string().nullable(),
});

export const commitRefSchema = z.object({
  kind: z.enum(["tag", "release"]),
  name: z.string(),
  prerelease: z.boolean(),
  latest: z.boolean(),
  published_at: z.string().nullable(),
  commits_since_previous: z.number().int().nonnegative().nullable(),
});

export const commitSchema = z.object({
  oid: z.string(),
  short_oid: z.string(),
  tree_oid: z.string(),
  parents: z.array(z.string()),
  author: signatureSchema,
  committer: signatureSchema,
  title: z.string(),
  message: z.string(),
  insertions: z.number(),
  deletions: z.number(),
  refs: z.array(commitRefSchema),
  verification: commitVerificationSchema.nullable(),
});

export const historySchema = z.object({
  commits: z.array(commitSchema),
  page: z.number(),
  per_page: z.number(),
  has_next: z.boolean(),
});

export const diffSchema = z.object({
  patch: z.string(),
  truncated: z.boolean(),
});

export const languageStatSchema = z.object({
  language: z.string(),
  files: z.number(),
  code: z.number(),
  comments: z.number(),
  blanks: z.number(),
});

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

export type AuthStatus = z.infer<typeof authStatusSchema>;
export type User = z.infer<typeof userSchema>;
export type InstanceSettings = z.infer<typeof instanceSettingsSchema>;
export type Changelog = z.infer<typeof changelogSchema>;
export type SshKey = z.infer<typeof sshKeySchema>;
export type ApiToken = z.infer<typeof tokenSchema>;
export type PasskeySummary = z.infer<typeof passkeySchema>;
export type OauthApplication = z.infer<typeof oauthApplicationSchema>;
export type Organization = z.infer<typeof organizationSchema>;
export type Member = z.infer<typeof memberSchema>;
export type MemberSuggestion = z.infer<typeof memberSuggestionSchema>;
export type AuditEvent = z.infer<typeof auditEventSchema>;
export type Repository = z.infer<typeof repositorySchema>;
export type MirrorIdentity = z.infer<typeof mirrorIdentitySchema>;
export type RepositoryMirror = z.infer<typeof repositoryMirrorSchema>;
export type RepositoryIntegrationConnection = z.infer<
  typeof repositoryIntegrationConnectionSchema
>;
export type RepositoryIntegration = z.infer<typeof repositoryIntegrationSchema>;
export type RepositoryIntegrations = z.infer<
  typeof repositoryIntegrationsSchema
>;
export type Webhook = z.infer<typeof webhookSchema>;
export type WebhookDelivery = z.infer<typeof webhookDeliverySchema>;
export type Issue = z.infer<typeof issueSchema>;
export type IssueUser = z.infer<typeof issueUserSchema>;
export type IssueComment = z.infer<typeof issueCommentSchema>;
export type IssueLabel = z.infer<typeof issueLabelSchema>;
export type IssueAttachment = z.infer<typeof issueAttachmentSchema>;
export type Release = z.infer<typeof releaseSchema>;
export type ReleaseAsset = z.infer<typeof releaseAssetSchema>;
export type RepositoryOverviewItem = z.infer<
  typeof repositoryOverviewItemSchema
>;
export type RepositoryActivity = z.infer<typeof repositoryActivitySchema>;
export type RepositoryRefs = z.infer<typeof refsSchema>;
export type Tree = z.infer<typeof treeSchema>;
export type TreeEntry = z.infer<typeof treeEntrySchema>;
export type Blob = z.infer<typeof blobSchema>;
export type Commit = z.infer<typeof commitSchema>;
export type CommitRef = z.infer<typeof commitRefSchema>;
export type History = z.infer<typeof historySchema>;
export type Diff = z.infer<typeof diffSchema>;
export type LanguageStat = z.infer<typeof languageStatSchema>;

export async function requestJson<T>(
  path: string,
  schema: z.ZodType<T>,
  init: RequestInit = {},
): Promise<T> {
  const headers = new Headers(init.headers);
  headers.set("accept", "application/json");
  if (init.body !== undefined && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  const response = await fetch(path, {
    ...init,
    headers,
    credentials: "same-origin",
  });
  const payload: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    const parsed = errorSchema.safeParse(payload);
    throw new ApiFailure(
      parsed.success
        ? parsed.data.error.message
        : `Request failed with status ${response.status}.`,
      response.status,
      parsed.success ? parsed.data.error.code : "request_failed",
    );
  }
  const parsed = schema.safeParse(payload);
  if (!parsed.success) {
    throw new ApiFailure(
      "The server returned an invalid response.",
      response.status,
      "invalid_response",
    );
  }
  return parsed.data;
}

export async function requestEmpty(
  path: string,
  init: RequestInit = {},
): Promise<void> {
  const headers = new Headers(init.headers);
  headers.set("accept", "application/json");
  if (init.body !== undefined && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  const response = await fetch(path, {
    ...init,
    headers,
    credentials: "same-origin",
  });
  if (!response.ok) {
    const payload: unknown = await response.json().catch(() => null);
    const parsed = errorSchema.safeParse(payload);
    throw new ApiFailure(
      parsed.success
        ? parsed.data.error.message
        : `Request failed with status ${response.status}.`,
      response.status,
      parsed.success ? parsed.data.error.code : "request_failed",
    );
  }
}

export function avatarUrl(userId: string, updatedAt: string | null) {
  return updatedAt
    ? `/api/v1/users/${userId}/avatar?v=${encodeURIComponent(updatedAt)}`
    : null;
}

export function organizationAvatarUrl(
  slug: string,
  updatedAt: string | null,
): string | null {
  return updatedAt
    ? `/api/v1/organizations/${encodeURIComponent(slug)}/avatar?v=${encodeURIComponent(updatedAt)}`
    : null;
}

export function jsonBody(value: unknown): string {
  return JSON.stringify(value);
}
