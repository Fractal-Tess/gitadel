import { z } from "zod";

import { jsonBody, requestEmpty, requestJson } from "./transport.js";

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
  /** The forge URL the provider was configured with, when Dokploy reports it. */
  url: z.string().nullable().optional(),
  /** False when the provider belongs to another forge. */
  targets_instance: z.boolean().default(true),
  /** Why this provider cannot serve this Gitadel, if it cannot. */
  problem: z.string().nullable().optional(),
});

export const integrationSourceBindingSchema = z.object({
  id: z.string(),
  name: z.string(),
  managed: z.boolean(),
  ready: z.boolean(),
  authorization_url: z.url().optional(),
  url: z.string().nullable().optional(),
  targets_instance: z.boolean().default(true),
  problem: z.string().nullable().optional(),
});

/** Whether Dokploy reached Gitadel through the bound provider. */
export const sourceCheckSchema = z.object({
  reachable: z.boolean(),
  /** The Gitadel address Dokploy uses after the check. */
  internal_url: z.string(),
  repositories: z.number().int().nullable(),
  /** Addresses that failed, in the order they were tried. */
  attempts: z.array(z.object({ url: z.string(), error: z.string() })),
});
export type SourceCheck = z.infer<typeof sourceCheckSchema>;

export const integrationSourceConnectionSchema = z.object({
  required: z.boolean(),
  binding: integrationSourceBindingSchema.nullable(),
  sources: z.array(integrationRemoteSourceSchema),
  /** Present when the response follows a reachability check. */
  check: sourceCheckSchema.optional(),
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
  /** True when an environment with that name already existed and was reused. */
  existing: z.boolean().optional(),
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

export const buildSuggestionSchema = z.object({
  kind: dokployResourceKindSchema,
  /** Directory Dokploy builds from, such as `/` or `/web`. */
  build_path: z.string(),
  /** Dockerfile name relative to `build_path`. */
  dockerfile: z.string().optional(),
  /** Compose file relative to the repository root. */
  compose_path: z.string().optional(),
  port: z.number().int().optional(),
});
export type BuildSuggestion = z.infer<typeof buildSuggestionSchema>;

export const deploymentSuggestionsSchema = z.object({
  branch: z.string().nullable(),
  project_name: z.string(),
  suggestions: z.array(buildSuggestionSchema),
});
export type DeploymentSuggestions = z.infer<typeof deploymentSuggestionsSchema>;

export type DeploymentSetupRequest = {
  kind: RemoteResource["kind"];
  name: string;
  branch: string;
  server_id?: string;
  repository_path?: string;
  compose_path?: string;
  dockerfile?: string;
  project_id?: string;
  project_name?: string;
  environment: string;
  domain?: { host: string; port: number; https: boolean };
  deploy: boolean;
  deploy_on_push: boolean;
};

export const deploymentStatusSchema = z.object({
  deployment: z
    .object({
      /** `running`, `done`, or `error`; Dokploy may report others. */
      status: z.string().nullable(),
      title: z.string().nullable(),
      created_at: z.string().nullable(),
      error: z.string().nullable(),
    })
    .nullable(),
  logs_url: z.string(),
});
export type DeploymentStatus = z.infer<typeof deploymentStatusSchema>;

export type RepositoryIntegrationConnection = z.infer<
  typeof repositoryIntegrationConnectionSchema
>;
export type RepositoryIntegration = z.infer<typeof repositoryIntegrationSchema>;
export const deploymentSetupResultSchema = z.object({
  integration: repositoryIntegrationSchema,
  steps: z.array(z.string()),
  warning: z.string().nullable(),
});
export type DeploymentSetupResult = z.infer<typeof deploymentSetupResultSchema>;
export type RepositoryIntegrations = z.infer<
  typeof repositoryIntegrationsSchema
>;
export type IntegrationSourceBinding = z.infer<
  typeof integrationSourceBindingSchema
>;
export type DokployCreatedEnvironment = z.infer<
  typeof dokployCreatedEnvironmentSchema
>;

// Paths

export function repositoryIntegrationsPath(
  namespace: string,
  name: string,
): string {
  return `/api/v1/repos/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}/integrations`;
}

export function repositoryIntegrationPath(
  namespace: string,
  name: string,
  id: string,
): string {
  return `${repositoryIntegrationsPath(namespace, name)}/${encodeURIComponent(id)}`;
}

export function namespaceIntegrationsPath(namespace: string): string {
  return `/api/v1/namespaces/${encodeURIComponent(namespace)}/integrations`;
}

export function namespaceIntegrationPath(
  namespace: string,
  id: string,
): string {
  return `${namespaceIntegrationsPath(namespace)}/${encodeURIComponent(id)}`;
}

// Repository integrations

export function loadRepositoryIntegrations(
  namespace: string,
  name: string,
): Promise<RepositoryIntegrations> {
  return requestJson(
    repositoryIntegrationsPath(namespace, name),
    repositoryIntegrationsSchema,
  );
}

export function loadRepositoryIntegration(
  namespace: string,
  name: string,
  id: string,
): Promise<RepositoryIntegration> {
  return requestJson(
    repositoryIntegrationPath(namespace, name, id),
    repositoryIntegrationSchema,
  );
}

export function createRepositoryIntegration(
  namespace: string,
  name: string,
  body: { integration_id: string; name: string },
): Promise<RepositoryIntegration> {
  return requestJson(
    repositoryIntegrationsPath(namespace, name),
    repositoryIntegrationSchema,
    { method: "POST", body: jsonBody(body) },
  );
}

/** `{ enabled }` toggles delivery; `{ enabled: false, resource: null }` unlinks. */
export function updateRepositoryIntegration(
  namespace: string,
  name: string,
  id: string,
  body: { enabled: boolean; resource?: null },
): Promise<RepositoryIntegration> {
  return requestJson(
    repositoryIntegrationPath(namespace, name, id),
    repositoryIntegrationSchema,
    { method: "PUT", body: jsonBody(body) },
  );
}

export function removeRepositoryIntegration(
  namespace: string,
  name: string,
  id: string,
): Promise<void> {
  return requestEmpty(repositoryIntegrationPath(namespace, name, id), {
    method: "DELETE",
  });
}

export function deployRepositoryIntegration(
  namespace: string,
  name: string,
  id: string,
): Promise<z.infer<typeof integrationDeployResultSchema>> {
  return requestJson(
    `${repositoryIntegrationPath(namespace, name, id)}/deploy`,
    integrationDeployResultSchema,
    { method: "POST", body: jsonBody({}) },
  );
}

// Dokploy resources behind a repository integration

export function loadDokployCatalog(
  namespace: string,
  name: string,
  id: string,
): Promise<DokployRemoteCatalog> {
  return requestJson(
    `${repositoryIntegrationPath(namespace, name, id)}/resources`,
    dokployRemoteCatalogSchema,
  );
}

export function createDokployProject(
  namespace: string,
  name: string,
  id: string,
  body: { name: string; description: null },
): Promise<DokployProject> {
  return requestJson(
    `${repositoryIntegrationPath(namespace, name, id)}/projects`,
    dokployProjectSchema,
    { method: "POST", body: jsonBody(body) },
  );
}

export function createDokployEnvironment(
  namespace: string,
  name: string,
  id: string,
  body: { project_id: string; name: string; description: string | null },
): Promise<DokployCreatedEnvironment> {
  return requestJson(
    `${repositoryIntegrationPath(namespace, name, id)}/environments`,
    dokployCreatedEnvironmentSchema,
    { method: "POST", body: jsonBody(body) },
  );
}

export function linkDokployResource(
  namespace: string,
  name: string,
  id: string,
  body: {
    kind: RemoteResource["kind"];
    id: string;
    branch: string;
    repository_path: string;
    compose_path: string;
  },
): Promise<RepositoryIntegration> {
  return requestJson(
    `${repositoryIntegrationPath(namespace, name, id)}/resources/link`,
    repositoryIntegrationSchema,
    { method: "POST", body: jsonBody(body) },
  );
}

export function createDokployResource(
  namespace: string,
  name: string,
  id: string,
  body: {
    kind: RemoteResource["kind"];
    name: string;
    environment_id: string;
    branch: string;
    server_id: string | null;
    repository_path: string;
    compose_path: string;
  },
): Promise<RepositoryIntegration> {
  return requestJson(
    `${repositoryIntegrationPath(namespace, name, id)}/resources`,
    repositoryIntegrationSchema,
    { method: "POST", body: jsonBody(body) },
  );
}

export function loadDeploymentSuggestions(
  namespace: string,
  name: string,
  id: string,
): Promise<DeploymentSuggestions> {
  return requestJson(
    `${repositoryIntegrationPath(namespace, name, id)}/suggestions`,
    deploymentSuggestionsSchema,
  );
}

/** Create, configure, link, and optionally deploy a resource in one step. */
export function setupDeployment(
  namespace: string,
  name: string,
  id: string,
  body: DeploymentSetupRequest,
): Promise<DeploymentSetupResult> {
  return requestJson(
    `${repositoryIntegrationPath(namespace, name, id)}/setup`,
    deploymentSetupResultSchema,
    { method: "POST", body: jsonBody(body) },
  );
}

export function loadDeploymentStatus(
  namespace: string,
  name: string,
  id: string,
): Promise<DeploymentStatus> {
  return requestJson(
    `${repositoryIntegrationPath(namespace, name, id)}/deployment`,
    deploymentStatusSchema,
  );
}

// Namespace connections

export function testNamespaceConnection(
  namespace: string,
  body: { provider: string; url: string; api_key: string },
): Promise<IntegrationTestResult> {
  return requestJson(
    `${namespaceIntegrationsPath(namespace)}/test`,
    integrationTestResultSchema,
    { method: "POST", body: jsonBody(body) },
  );
}

export function createNamespaceConnection(
  namespace: string,
  body: {
    provider: string;
    name: string;
    url: string;
    internal_url: string;
    api_key: string;
  },
): Promise<NamespaceIntegration> {
  return requestJson(
    namespaceIntegrationsPath(namespace),
    namespaceIntegrationSchema,
    { method: "POST", body: jsonBody(body) },
  );
}

export function updateNamespaceConnection(
  namespace: string,
  id: string,
  body: {
    name: string;
    url: string;
    internal_url: string;
    api_key: string | null;
  },
): Promise<NamespaceIntegration> {
  return requestJson(
    namespaceIntegrationPath(namespace, id),
    namespaceIntegrationSchema,
    { method: "PUT", body: jsonBody(body) },
  );
}

/** Change only the address Dokploy uses to reach Gitadel. */
export function updateNamespaceInternalUrl(
  namespace: string,
  id: string,
  internalUrl: string,
): Promise<NamespaceIntegration> {
  return requestJson(
    namespaceIntegrationPath(namespace, id),
    namespaceIntegrationSchema,
    { method: "PUT", body: jsonBody({ internal_url: internalUrl }) },
  );
}

export async function revealNamespaceCredential(
  namespace: string,
  id: string,
): Promise<string> {
  const credential = await requestJson(
    `${namespaceIntegrationPath(namespace, id)}/credential`,
    integrationCredentialSchema,
  );
  return credential.api_key;
}

export function testNamespaceIntegration(
  namespace: string,
  id: string,
): Promise<IntegrationTestResult> {
  return requestJson(
    `${namespaceIntegrationPath(namespace, id)}/test`,
    integrationTestResultSchema,
    { method: "POST" },
  );
}

export function loadIntegrationSource(
  namespace: string,
  id: string,
): Promise<IntegrationSourceConnection> {
  return requestJson(
    `${namespaceIntegrationPath(namespace, id)}/source`,
    integrationSourceConnectionSchema,
  );
}

export function bindIntegrationSource(
  namespace: string,
  id: string,
  sourceId: string,
): Promise<IntegrationSourceConnection> {
  return requestJson(
    `${namespaceIntegrationPath(namespace, id)}/source`,
    integrationSourceConnectionSchema,
    { method: "POST", body: jsonBody({ action: "bind", source_id: sourceId }) },
  );
}

export function createIntegrationSource(
  namespace: string,
  id: string,
  name: string,
): Promise<IntegrationSourceConnection> {
  return requestJson(
    `${namespaceIntegrationPath(namespace, id)}/source`,
    integrationSourceConnectionSchema,
    { method: "POST", body: jsonBody({ action: "create", name }) },
  );
}

/** Check that Dokploy can reach Gitadel through the bound provider. */
export function verifyIntegrationSource(
  namespace: string,
  id: string,
): Promise<SourceCheck> {
  return requestJson(
    `${namespaceIntegrationPath(namespace, id)}/source/verify`,
    sourceCheckSchema,
    { method: "POST" },
  );
}

export function disconnectIntegrationSource(
  namespace: string,
  id: string,
): Promise<void> {
  return requestEmpty(`${namespaceIntegrationPath(namespace, id)}/source`, {
    method: "DELETE",
  });
}

// Pure helpers shared by the repository integration views

/** The Dokploy link stored on an integration, if it is a valid one. */
export function linkedDokployResource(
  integration: Pick<RepositoryIntegration, "provider" | "resource">,
): DokployResourceLink | null {
  if (integration.provider !== "dokploy" || !integration.resource) return null;
  const result = dokployResourceLinkSchema.safeParse(integration.resource);
  return result.success ? result.data : null;
}

/** The parsed Dokploy link of a resource value regardless of provider. */
export function parseDokployResourceLink(
  resource: unknown,
): DokployResourceLink | null {
  if (!resource) return null;
  const result = dokployResourceLinkSchema.safeParse(resource);
  return result.success ? result.data : null;
}

export function connectionConfigured(
  connections: readonly RepositoryIntegrationConnection[],
  integration: Pick<RepositoryIntegration, "connection_id">,
): boolean {
  return (
    connections.find(
      (connection) => connection.id === integration.connection_id,
    )?.configured ?? false
  );
}

export function integrationStatus(
  integration: Pick<RepositoryIntegration, "enabled" | "repository_setup">,
  resource: DokployResourceLink | null,
  connectionReady: boolean,
): string {
  if (!connectionReady) return "Connection setup required";
  if (!integration.enabled) return "Disabled";
  if (resource) return "Push delivery on";
  if (integration.repository_setup) return "Repository setup required";
  return "Enabled";
}

export function describeIntegration(
  integration: Pick<
    RepositoryIntegration,
    "connection_name" | "configured" | "enabled" | "repository_setup"
  >,
  resource: DokployResourceLink | null,
  connectionReady: boolean,
): string {
  if (!connectionReady) {
    return `${integration.connection_name} needs a repository source before deployments can run.`;
  }
  if (!integration.configured) {
    return `${integration.connection_name} is currently disabled.`;
  }
  if (resource) {
    return `Source ${integration.enabled ? "enabled" : "disabled"} · ${resource.project_name} / ${resource.environment_name}`;
  }
  if (!integration.enabled) return "Disabled for this repository.";
  if (!integration.repository_setup) return "Enabled for this repository.";
  return "Setup required before push deployments can be enabled.";
}

export function resourceKindLabel(kind: RemoteResource["kind"]): string {
  return kind === "application" ? "Application" : "Docker Compose";
}

/** `running_now` becomes `Running Now`; no status reads as available. */
export function dokployStatusLabel(status: string | null): string {
  if (!status) return "Available";
  return status
    .replaceAll("_", " ")
    .replace(/\b\w/g, (character) => character.toUpperCase());
}

/** A freshly created namespace connection as the repository list shows it. */
export function connectionFromCreated(
  created: NamespaceIntegration,
  providers: readonly IntegrationProvider[],
): { connection: RepositoryIntegrationConnection; sourceRequired: boolean } {
  const sourceRequired =
    providers.find((provider) => provider.slug === created.provider)
      ?.source_required ?? false;
  return {
    sourceRequired,
    connection: {
      id: created.id,
      provider: created.provider,
      provider_name: created.provider_name,
      name: created.name,
      configured:
        created.enabled && (!sourceRequired || created.source?.ready === true),
    },
  };
}

/** The staged connection with its source binding replaced. */
export function withSourceBinding(
  connection: NamespaceIntegration,
  binding: IntegrationSourceBinding | null | undefined,
): NamespaceIntegration {
  return {
    ...connection,
    source: binding
      ? {
          id: binding.id,
          name: binding.name,
          managed: binding.managed,
          ready: binding.ready,
        }
      : undefined,
  };
}

/** The repository-side view of `staged` once its binding is `binding`. */
export function connectionAfterBinding(
  connection: RepositoryIntegrationConnection,
  staged: NamespaceIntegration,
  binding: IntegrationSourceBinding | null | undefined,
): RepositoryIntegrationConnection {
  return {
    ...connection,
    name: staged.name,
    configured: staged.enabled && binding?.ready === true,
  };
}

/** The environment Dokploy created with `project`, when it matches `name`. */
export function findDefaultEnvironment(
  project: DokployProject | null,
  name: string,
): DokployEnvironment | undefined {
  return project?.environments.find(
    (environment) =>
      environment.name.localeCompare(name.trim(), undefined, {
        sensitivity: "accent",
      }) === 0,
  );
}

/** `catalog` with `project` first, replacing any earlier copy of it. */
export function catalogWithProject(
  catalog: DokployRemoteCatalog,
  project: DokployProject,
): DokployRemoteCatalog {
  return {
    ...catalog,
    projects: [
      project,
      ...catalog.projects.filter((candidate) => candidate.id !== project.id),
    ],
  };
}

/**
 * `catalog` with `created` listed first inside its project. A project that
 * was created in the same step is prepended; an environment the project
 * already lists is not duplicated.
 */
export function catalogWithEnvironment(
  catalog: DokployRemoteCatalog,
  created: DokployCreatedEnvironment,
  createdProject: DokployProject | null,
): DokployRemoteCatalog {
  const projects = createdProject
    ? [
        createdProject,
        ...catalog.projects.filter(
          (project) => project.id !== createdProject?.id,
        ),
      ]
    : catalog.projects;
  return {
    ...catalog,
    projects: projects.map((project) =>
      project.id !== created.project_id ||
      project.environments.some((environment) => environment.id === created.id)
        ? project
        : {
            ...project,
            environments: [
              {
                id: created.id,
                name: created.name,
                description: created.description,
                resources: created.resources,
              },
              ...project.environments,
            ],
          },
    ),
  };
}

// Guided Dokploy connection

/**
 * The step of the connection setup that needs attention: connect a provider,
 * authorize a provider Gitadel did not create, prove Dokploy can reach
 * Gitadel, or deploy a repository once everything is confirmed.
 */
export type SourceSetupStep = "connect" | "authorize" | "reach" | "deploy";

export function sourceSetupStep(
  binding: Pick<IntegrationSourceBinding, "ready" | "targets_instance"> | null,
  check: Pick<SourceCheck, "reachable"> | null,
): SourceSetupStep {
  if (!binding || !binding.targets_instance) return "connect";
  if (!binding.ready) return "authorize";
  if (!check?.reachable) return "reach";
  return "deploy";
}

// One-form deployment setup

/** Select value for "deploy on the Dokploy server itself". */
export const DOKPLOY_SERVER = "__dokploy__";
/** Select value for "create a new project". */
export const NEW_DOKPLOY_PROJECT = "__new_project__";
/** Build choice for "I'll configure the build in Dokploy". */
export const OTHER_BUILD = "other";

export type BuildChoice = {
  id: string;
  suggestion: BuildSuggestion | null;
  label: string;
  detail: string;
};

/** Every detected build, then the manual fallback. */
export function buildChoices(
  suggestions: readonly BuildSuggestion[],
): BuildChoice[] {
  return [
    ...suggestions.map((suggestion, index) => ({
      id: `suggestion-${index}`,
      suggestion,
      label:
        suggestion.kind === "compose"
          ? `Compose file ${suggestion.compose_path ?? "./docker-compose.yml"}`
          : `${suggestion.dockerfile ?? "Dockerfile"} in ${suggestion.build_path}${
              suggestion.port ? ` · port ${suggestion.port}` : ""
            }`,
      detail:
        suggestion.kind === "compose"
          ? "Dokploy starts every service in this Compose file."
          : "Dokploy builds this Dockerfile and runs it as one application.",
    })),
    {
      id: OTHER_BUILD,
      suggestion: null,
      label: "Other — I'll configure the build in Dokploy",
      detail:
        "Gitadel creates an application. Choose its build type in Dokploy before the first deployment.",
    },
  ];
}

export type SetupForm = {
  build: string;
  /** Build path for the manual choice. */
  buildPath: string;
  name: string;
  branch: string;
  /** A project id, or `NEW_DOKPLOY_PROJECT`. */
  project: string;
  projectName: string;
  environment: string;
  /** A server id, or `DOKPLOY_SERVER`. */
  server: string;
  domainHost: string;
  domainPort: number | null;
  domainHttps: boolean;
  deploy: boolean;
  deployOnPush: boolean;
};

/** The port to expose for `choice`, falling back to 80. */
export function suggestedPort(choice: BuildChoice | undefined): number {
  return choice?.suggestion?.port ?? 80;
}

/** Form defaults from what Gitadel detected and what Dokploy already has. */
export function defaultSetupForm({
  repositoryName,
  defaultBranch,
  suggestions,
  catalog,
}: {
  repositoryName: string;
  defaultBranch: string | null | undefined;
  suggestions: DeploymentSuggestions | null;
  catalog: Pick<DokployRemoteCatalog, "projects"> | null;
}): SetupForm {
  const choices = buildChoices(suggestions?.suggestions ?? []);
  const projectName = suggestions?.project_name || repositoryName;
  const existing = catalog?.projects.find(
    (project) =>
      project.name.localeCompare(projectName, undefined, {
        sensitivity: "accent",
      }) === 0,
  );
  return {
    build: choices[0]!.id,
    buildPath: "/",
    name: repositoryName,
    branch: suggestions?.branch || defaultBranch || "main",
    project: existing?.id ?? NEW_DOKPLOY_PROJECT,
    projectName,
    environment: "production",
    server: DOKPLOY_SERVER,
    domainHost: "",
    domainPort: suggestedPort(choices[0]),
    domainHttps: true,
    deploy: true,
    deployOnPush: true,
  };
}

/** Why `form` cannot be submitted yet, or null when it can. */
export function setupFormProblem(
  form: SetupForm,
  choices: readonly BuildChoice[],
): string | null {
  const choice = choices.find((candidate) => candidate.id === form.build);
  if (!choice) return "Choose how Dokploy builds this repository.";
  if (!form.name.trim()) return "Enter a name for the Dokploy resource.";
  if (!form.branch.trim()) return "Enter the branch to deploy.";
  if (form.project === NEW_DOKPLOY_PROJECT && !form.projectName.trim())
    return "Enter a name for the new project.";
  if (!form.environment.trim()) return "Enter an environment name.";
  if (choice.suggestion?.kind !== "compose" && form.domainHost.trim()) {
    const port = form.domainPort;
    if (port === null || !Number.isInteger(port) || port < 1 || port > 65535)
      return "Enter the port your application listens on (1–65535).";
  }
  return null;
}

/** The `POST …/setup` body for `form`. */
export function setupRequestBody(
  form: SetupForm,
  choices: readonly BuildChoice[],
): DeploymentSetupRequest {
  const suggestion =
    choices.find((candidate) => candidate.id === form.build)?.suggestion ??
    null;
  const kind = suggestion?.kind ?? "application";
  const body: DeploymentSetupRequest = {
    kind,
    name: form.name.trim(),
    branch: form.branch.trim(),
    environment: form.environment.trim() || "production",
    deploy: form.deploy,
    deploy_on_push: form.deployOnPush,
  };
  if (form.project === NEW_DOKPLOY_PROJECT) {
    body.project_name = form.projectName.trim();
  } else {
    body.project_id = form.project;
  }
  if (form.server !== DOKPLOY_SERVER) body.server_id = form.server;
  if (kind === "compose") {
    body.compose_path = suggestion?.compose_path ?? "./docker-compose.yml";
    return body;
  }
  body.repository_path = suggestion
    ? suggestion.build_path
    : form.buildPath.trim() || "/";
  if (suggestion?.dockerfile) body.dockerfile = suggestion.dockerfile;
  const host = form.domainHost.trim();
  if (host) {
    body.domain = {
      host,
      port: form.domainPort ?? 80,
      https: form.domainHttps,
    };
  }
  return body;
}

export type DeploymentTone = "idle" | "running" | "done" | "error";

/** How the newest deployment reads, in words and as a tone. */
export function deploymentStatusView(
  deployment: DeploymentStatus["deployment"],
): { label: string; tone: DeploymentTone } {
  if (!deployment) return { label: "No deployments yet", tone: "idle" };
  switch (deployment.status) {
    case null:
    case "running":
      return { label: "Building", tone: "running" };
    case "done":
      return { label: "Deployed", tone: "done" };
    case "error":
      return { label: "Failed", tone: "error" };
    default:
      return { label: dokployStatusLabel(deployment.status), tone: "idle" };
  }
}

/**
 * Whether to ask Dokploy again: while nothing is loaded, while a deployment
 * is building, and, after starting one, until a deployment other than the
 * newest one before it (`awaiting.previous`, by creation time) appears.
 */
export function shouldPollDeployment(
  status: DeploymentStatus | null,
  awaiting: { previous: string | null } | null,
): boolean {
  if (!status) return true;
  if (awaiting && (status.deployment?.created_at ?? null) === awaiting.previous)
    return true;
  if (!status.deployment) return false;
  return deploymentStatusView(status.deployment).tone === "running";
}
