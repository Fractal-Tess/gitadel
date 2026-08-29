import { z } from "zod";

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

export type RepositoryIntegrationConnection = z.infer<
  typeof repositoryIntegrationConnectionSchema
>;
export type RepositoryIntegration = z.infer<typeof repositoryIntegrationSchema>;
export type RepositoryIntegrations = z.infer<
  typeof repositoryIntegrationsSchema
>;
