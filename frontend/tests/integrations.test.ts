import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  DOKPLOY_SERVER,
  NEW_DOKPLOY_PROJECT,
  OTHER_BUILD,
  buildChoices,
  catalogWithEnvironment,
  catalogWithProject,
  connectionAfterBinding,
  connectionConfigured,
  connectionFromCreated,
  defaultSetupForm,
  deploymentStatusView,
  describeIntegration,
  dokployStatusLabel,
  findDefaultEnvironment,
  integrationStatus,
  linkedDokployResource,
  namespaceIntegrationPath,
  namespaceIntegrationsPath,
  parseDokployResourceLink,
  repositoryIntegrationPath,
  repositoryIntegrationsPath,
  resourceKindLabel,
  setupFormProblem,
  setupRequestBody,
  shouldPollDeployment,
  sourceSetupStep,
  suggestedPort,
  withSourceBinding,
  type DeploymentSuggestions,
  type DokployRemoteCatalog,
  type DokployResourceLink,
  type NamespaceIntegration,
  type RepositoryIntegration,
  type RepositoryIntegrationConnection,
} from "../src/lib/api/integrations.js";

const link: DokployResourceLink = {
  kind: "application",
  id: "app-1",
  name: "web",
  branch: "main",
  project_id: "p1",
  project_name: "Shop",
  environment_id: "e1",
  environment_name: "production",
  external_url: "https://dokploy.example/app-1",
};

function integration(
  overrides: Partial<RepositoryIntegration> = {},
): RepositoryIntegration {
  return {
    id: "8f1e6d0a-5d4e-4c7b-9a3b-2f1c0e9d8b7a",
    connection_id: "1b2c3d4e-5f60-4718-8293-a4b5c6d7e8f9",
    provider: "dokploy",
    provider_name: "Dokploy",
    name: "Production",
    connection_name: "Dokploy EU",
    configured: true,
    repository_setup: true,
    enabled: true,
    ...overrides,
  };
}

function connection(
  overrides: Partial<RepositoryIntegrationConnection> = {},
): RepositoryIntegrationConnection {
  return {
    id: "1b2c3d4e-5f60-4718-8293-a4b5c6d7e8f9",
    provider: "dokploy",
    provider_name: "Dokploy",
    name: "Dokploy EU",
    configured: true,
    ...overrides,
  };
}

function namespaceIntegration(
  overrides: Partial<NamespaceIntegration> = {},
): NamespaceIntegration {
  return {
    id: "1b2c3d4e-5f60-4718-8293-a4b5c6d7e8f9",
    provider: "dokploy",
    provider_name: "Dokploy",
    name: "Dokploy EU",
    enabled: true,
    url: "https://dokploy.example",
    internal_url: "http://gitadel.internal:3030",
    api_key_set: true,
    ...overrides,
  };
}

describe("integration paths", () => {
  test("encode every segment", () => {
    assert.equal(
      repositoryIntegrationsPath("a b", "c/d"),
      "/api/v1/repos/a%20b/c%2Fd/integrations",
    );
    assert.equal(
      repositoryIntegrationPath("alice", "project", "x?y"),
      "/api/v1/repos/alice/project/integrations/x%3Fy",
    );
    assert.equal(
      namespaceIntegrationsPath("ör g"),
      "/api/v1/namespaces/%C3%B6r%20g/integrations",
    );
    assert.equal(
      namespaceIntegrationPath("alice", "id#1"),
      "/api/v1/namespaces/alice/integrations/id%231",
    );
  });
});

describe("linkedDokployResource", () => {
  test("accepts only a valid link on a Dokploy integration", () => {
    assert.deepEqual(
      linkedDokployResource(integration({ resource: link })),
      link,
    );
    assert.equal(
      linkedDokployResource(integration({ provider: "other", resource: link })),
      null,
    );
    assert.equal(
      linkedDokployResource(integration({ resource: undefined })),
      null,
    );
    assert.equal(
      linkedDokployResource(integration({ resource: { kind: "application" } })),
      null,
    );
    assert.deepEqual(parseDokployResourceLink(link), link);
    assert.equal(parseDokployResourceLink(null), null);
    assert.equal(parseDokployResourceLink({ external_url: "nope" }), null);
  });
});

describe("integration cards", () => {
  test("look up whether the connection is configured", () => {
    const connections = [
      connection(),
      connection({ id: "other", configured: false }),
    ];
    assert.equal(connectionConfigured(connections, integration()), true);
    assert.equal(
      connectionConfigured(
        connections,
        integration({ connection_id: "other" }),
      ),
      false,
    );
    assert.equal(
      connectionConfigured(
        connections,
        integration({ connection_id: "missing" }),
      ),
      false,
    );
  });

  test("status follows connection, enablement, link, and setup in that order", () => {
    assert.equal(
      integrationStatus(integration(), link, false),
      "Connection setup required",
    );
    assert.equal(
      integrationStatus(integration({ enabled: false }), link, true),
      "Disabled",
    );
    assert.equal(
      integrationStatus(integration(), link, true),
      "Push delivery on",
    );
    assert.equal(
      integrationStatus(integration(), null, true),
      "Repository setup required",
    );
    assert.equal(
      integrationStatus(integration({ repository_setup: false }), null, true),
      "Enabled",
    );
  });

  test("descriptions match the status", () => {
    assert.equal(
      describeIntegration(integration(), link, false),
      "Dokploy EU needs a repository source before deployments can run.",
    );
    assert.equal(
      describeIntegration(integration({ configured: false }), link, true),
      "Dokploy EU is currently disabled.",
    );
    assert.equal(
      describeIntegration(integration(), link, true),
      "Source enabled · Shop / production",
    );
    assert.equal(
      describeIntegration(integration({ enabled: false }), link, true),
      "Source disabled · Shop / production",
    );
    assert.equal(
      describeIntegration(integration({ enabled: false }), null, true),
      "Disabled for this repository.",
    );
    assert.equal(
      describeIntegration(integration({ repository_setup: false }), null, true),
      "Enabled for this repository.",
    );
    assert.equal(
      describeIntegration(integration(), null, true),
      "Setup required before push deployments can be enabled.",
    );
  });

  test("labels resource kinds and Dokploy statuses", () => {
    assert.equal(resourceKindLabel("application"), "Application");
    assert.equal(resourceKindLabel("compose"), "Docker Compose");
    assert.equal(dokployStatusLabel(null), "Available");
    assert.equal(dokployStatusLabel(""), "Available");
    assert.equal(dokployStatusLabel("running"), "Running");
    assert.equal(dokployStatusLabel("not_running_yet"), "Not Running Yet");
  });
});

describe("staged connections", () => {
  test("a created connection is configured once enabled and, if needed, sourced", () => {
    const providers = [
      {
        slug: "dokploy",
        name: "Dokploy",
        description: "",
        icon: "",
        source_required: true,
      },
      {
        slug: "plain",
        name: "Plain",
        description: "",
        icon: "",
        source_required: false,
      },
    ];
    const sourced = connectionFromCreated(
      namespaceIntegration({
        source: { id: "s", name: "Source", managed: true, ready: true },
      }),
      providers,
    );
    assert.equal(sourced.sourceRequired, true);
    assert.equal(sourced.connection.configured, true);
    assert.equal(sourced.connection.name, "Dokploy EU");
    const unsourced = connectionFromCreated(namespaceIntegration(), providers);
    assert.equal(unsourced.connection.configured, false);
    const disabled = connectionFromCreated(
      namespaceIntegration({ provider: "plain", enabled: false }),
      providers,
    );
    assert.equal(disabled.sourceRequired, false);
    assert.equal(disabled.connection.configured, false);
    const plain = connectionFromCreated(
      namespaceIntegration({ provider: "plain" }),
      providers,
    );
    assert.equal(plain.connection.configured, true);
    // Unknown providers are treated as not needing a source.
    assert.equal(
      connectionFromCreated(
        namespaceIntegration({ provider: "unknown" }),
        providers,
      ).sourceRequired,
      false,
    );
  });

  test("bindings are mirrored onto the staged connection and the list entry", () => {
    const staged = namespaceIntegration();
    const binding = {
      id: "s",
      name: "Source",
      managed: true,
      ready: false,
      targets_instance: true,
      authorization_url: "https://dokploy.example/authorize",
    };
    const bound = withSourceBinding(staged, binding);
    assert.deepEqual(bound.source, {
      id: "s",
      name: "Source",
      managed: true,
      ready: false,
    });
    assert.equal(withSourceBinding(staged, null).source, undefined);
    assert.equal(withSourceBinding(bound, undefined).source, undefined);

    const entry = connection({ name: "Old name", configured: false });
    const renamed = { ...bound, name: "New name" };
    assert.deepEqual(connectionAfterBinding(entry, renamed, binding), {
      ...entry,
      name: "New name",
      configured: false,
    });
    assert.equal(
      connectionAfterBinding(entry, renamed, { ...binding, ready: true })
        .configured,
      true,
    );
    assert.equal(
      connectionAfterBinding(
        entry,
        { ...renamed, enabled: false },
        {
          ...binding,
          ready: true,
        },
      ).configured,
      false,
    );
    assert.equal(
      connectionAfterBinding(entry, renamed, null).configured,
      false,
    );
  });
});

describe("Dokploy catalog updates", () => {
  const catalog: DokployRemoteCatalog = {
    external_url: "https://dokploy.example",
    servers: [],
    projects: [
      {
        id: "p1",
        name: "Shop",
        environments: [
          { id: "e1", name: "production", description: null, resources: [] },
        ],
      },
      { id: "p2", name: "Blog", environments: [] },
    ],
  };

  test("find the environment a new project came with, ignoring case", () => {
    const project = catalog.projects[0]!;
    assert.equal(findDefaultEnvironment(project, " Production ")?.id, "e1");
    assert.equal(findDefaultEnvironment(project, "staging"), undefined);
    assert.equal(findDefaultEnvironment(null, "production"), undefined);
  });

  test("a project moves to the front and replaces its earlier copy", () => {
    const updated = catalogWithProject(catalog, {
      id: "p2",
      name: "Blog renamed",
      environments: [],
    });
    assert.deepEqual(
      updated.projects.map((project) => [project.id, project.name]),
      [
        ["p2", "Blog renamed"],
        ["p1", "Shop"],
      ],
    );
    assert.equal(
      catalogWithProject(catalog, catalog.projects[0]!).projects.length,
      2,
    );
  });

  test("a created environment is listed first in its project, once", () => {
    const created = {
      id: "e2",
      name: "staging",
      description: "Previews",
      resources: [],
      project_id: "p1",
      project_name: "Shop",
    };
    const updated = catalogWithEnvironment(catalog, created, null);
    assert.deepEqual(
      updated.projects[0]!.environments.map((environment) => environment.id),
      ["e2", "e1"],
    );
    assert.equal(updated.projects[0]!.environments[0]!.description, "Previews");
    assert.deepEqual(updated.projects[1], catalog.projects[1]);
    // Already present: nothing changes.
    const again = catalogWithEnvironment(updated, created, null);
    assert.deepEqual(again, updated);

    // Created together with its project: the project is prepended and the
    // environment that came with it is not duplicated.
    const project = {
      id: "p3",
      name: "New",
      environments: [
        { id: "e3", name: "production", description: null, resources: [] },
      ],
    };
    const withProject = catalogWithEnvironment(
      catalog,
      { ...project.environments[0]!, project_id: "p3", project_name: "New" },
      project,
    );
    assert.deepEqual(
      withProject.projects.map((candidate) => candidate.id),
      ["p3", "p1", "p2"],
    );
    assert.equal(withProject.projects[0]!.environments.length, 1);
    const fresh = catalogWithEnvironment(
      catalog,
      {
        id: "e4",
        name: "qa",
        description: null,
        resources: [],
        project_id: "p3",
        project_name: "New",
      },
      project,
    );
    assert.deepEqual(
      fresh.projects[0]!.environments.map((environment) => environment.id),
      ["e4", "e3"],
    );
  });
});

describe("connection setup steps", () => {
  const ready = { ready: true, targets_instance: true };
  test("advance only on what Dokploy confirmed", () => {
    assert.equal(sourceSetupStep(null, null), "connect");
    assert.equal(
      sourceSetupStep({ ready: true, targets_instance: false }, null),
      "connect",
    );
    assert.equal(
      sourceSetupStep({ ready: false, targets_instance: true }, null),
      "authorize",
    );
    assert.equal(sourceSetupStep(ready, null), "reach");
    assert.equal(sourceSetupStep(ready, { reachable: false }), "reach");
    assert.equal(sourceSetupStep(ready, { reachable: true }), "deploy");
  });
  test("a reachable check does not skip an unauthorized provider", () => {
    assert.equal(
      sourceSetupStep(
        { ready: false, targets_instance: true },
        { reachable: true },
      ),
      "authorize",
    );
  });
});

describe("deployment setup form", () => {
  const detected: DeploymentSuggestions = {
    branch: "trunk",
    project_name: "home-therapy",
    suggestions: [
      {
        kind: "application",
        build_path: "/web",
        dockerfile: "Dockerfile",
        port: 8080,
      },
      { kind: "compose", build_path: "/", compose_path: "./compose.yml" },
    ],
  };
  const choices = buildChoices(detected.suggestions);

  test("labels every suggestion and always offers the manual build last", () => {
    assert.deepEqual(
      choices.map((choice) => choice.label),
      [
        "Dockerfile in /web · port 8080",
        "Compose file ./compose.yml",
        "Other — I'll configure the build in Dokploy",
      ],
    );
    assert.equal(choices.at(-1)?.id, OTHER_BUILD);
    assert.deepEqual(
      buildChoices([]).map((choice) => choice.id),
      [OTHER_BUILD],
    );
    assert.equal(suggestedPort(choices[0]), 8080);
    assert.equal(suggestedPort(choices[1]), 80);
  });

  test("defaults come from suggestions, then the repository", () => {
    const form = defaultSetupForm({
      repositoryName: "home-therapy",
      defaultBranch: "main",
      suggestions: detected,
      catalog: { projects: [] },
    });
    assert.equal(form.build, "suggestion-0");
    assert.equal(form.branch, "trunk");
    assert.equal(form.name, "home-therapy");
    assert.equal(form.project, NEW_DOKPLOY_PROJECT);
    assert.equal(form.projectName, "home-therapy");
    assert.equal(form.environment, "production");
    assert.equal(form.server, DOKPLOY_SERVER);
    assert.equal(form.domainPort, 8080);
    assert.equal(form.domainHttps, true);
    assert.equal(form.deploy, true);
    assert.equal(form.deployOnPush, true);

    const fallback = defaultSetupForm({
      repositoryName: "shop",
      defaultBranch: null,
      suggestions: null,
      catalog: null,
    });
    assert.equal(fallback.build, OTHER_BUILD);
    assert.equal(fallback.branch, "main");
    assert.equal(fallback.domainPort, 80);
  });

  test("an existing project with the same name is chosen", () => {
    const form = defaultSetupForm({
      repositoryName: "home-therapy",
      defaultBranch: "main",
      suggestions: detected,
      catalog: {
        projects: [{ id: "p9", name: "Home-Therapy", environments: [] }],
      },
    });
    assert.equal(form.project, "p9");
  });

  test("builds a Dockerfile application body with a domain", () => {
    const form = {
      ...defaultSetupForm({
        repositoryName: "home-therapy",
        defaultBranch: "main",
        suggestions: detected,
        catalog: null,
      }),
      name: "  web ",
      domainHost: " app.example.com ",
      deployOnPush: false,
    };
    assert.deepEqual(setupRequestBody(form, choices), {
      kind: "application",
      name: "web",
      branch: "trunk",
      environment: "production",
      deploy: true,
      deploy_on_push: false,
      project_name: "home-therapy",
      repository_path: "/web",
      dockerfile: "Dockerfile",
      domain: { host: "app.example.com", port: 8080, https: true },
    });
  });

  test("builds a compose body for an existing project and server", () => {
    const form = {
      ...defaultSetupForm({
        repositoryName: "shop",
        defaultBranch: "main",
        suggestions: detected,
        catalog: null,
      }),
      build: "suggestion-1",
      project: "p1",
      server: "s1",
      environment: "staging",
      domainHost: "ignored.example.com",
    };
    assert.deepEqual(setupRequestBody(form, choices), {
      kind: "compose",
      name: "shop",
      branch: "trunk",
      environment: "staging",
      deploy: true,
      deploy_on_push: true,
      project_id: "p1",
      server_id: "s1",
      compose_path: "./compose.yml",
    });
  });

  test("the manual build sends a build path and no Dockerfile", () => {
    const form = {
      ...defaultSetupForm({
        repositoryName: "shop",
        defaultBranch: "main",
        suggestions: null,
        catalog: null,
      }),
      buildPath: " /api ",
    };
    const body = setupRequestBody(form, buildChoices([]));
    assert.equal(body.kind, "application");
    assert.equal(body.repository_path, "/api");
    assert.equal(body.dockerfile, undefined);
    assert.equal(body.domain, undefined);
  });

  test("explains what is missing before submitting", () => {
    const form = defaultSetupForm({
      repositoryName: "shop",
      defaultBranch: "main",
      suggestions: detected,
      catalog: null,
    });
    assert.equal(setupFormProblem(form, choices), null);
    assert.match(
      setupFormProblem({ ...form, name: " " }, choices) ?? "",
      /name/,
    );
    assert.match(
      setupFormProblem({ ...form, projectName: "" }, choices) ?? "",
      /project/,
    );
    assert.match(
      setupFormProblem(
        { ...form, domainHost: "a.example.com", domainPort: 70000 },
        choices,
      ) ?? "",
      /port/,
    );
    assert.equal(
      setupFormProblem(
        { ...form, build: "suggestion-1", domainHost: "a", domainPort: null },
        choices,
      ),
      null,
    );
    assert.match(
      setupFormProblem({ ...form, build: "missing" }, choices) ?? "",
      /build/,
    );
  });
});

describe("deployment status", () => {
  const deployment = (
    status: string | null,
    created_at = "2026-10-17T10:00:00Z",
  ) => ({
    deployment: { status, title: "Deploy", created_at, error: null },
    logs_url: "https://dokploy.example/app?tab=deployments",
  });

  test("reads Dokploy statuses in words", () => {
    assert.deepEqual(deploymentStatusView(null), {
      label: "No deployments yet",
      tone: "idle",
    });
    assert.equal(
      deploymentStatusView(deployment("running").deployment).label,
      "Building",
    );
    assert.equal(
      deploymentStatusView(deployment(null).deployment).label,
      "Building",
    );
    assert.equal(
      deploymentStatusView(deployment("done").deployment).label,
      "Deployed",
    );
    assert.equal(
      deploymentStatusView(deployment("error").deployment).label,
      "Failed",
    );
    assert.equal(
      deploymentStatusView(deployment("cancelled").deployment).label,
      "Cancelled",
    );
  });

  test("polls while building or until a started deployment appears", () => {
    const none = { deployment: null, logs_url: "" };
    assert.equal(shouldPollDeployment(null, null), true);
    assert.equal(shouldPollDeployment(none, null), false);
    assert.equal(shouldPollDeployment(none, { previous: null }), true);
    assert.equal(shouldPollDeployment(deployment("running"), null), true);
    assert.equal(shouldPollDeployment(deployment("done"), null), false);
    assert.equal(shouldPollDeployment(deployment("error"), null), false);
    // The previous deployment is still the newest: keep waiting.
    assert.equal(
      shouldPollDeployment(deployment("done"), {
        previous: "2026-10-17T10:00:00Z",
      }),
      true,
    );
    assert.equal(
      shouldPollDeployment(deployment("done", "2026-10-17T10:05:00Z"), {
        previous: "2026-10-17T10:00:00Z",
      }),
      false,
    );
  });
});
