import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  catalogWithEnvironment,
  catalogWithProject,
  connectionAfterBinding,
  connectionConfigured,
  connectionFromCreated,
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
  withSourceBinding,
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
