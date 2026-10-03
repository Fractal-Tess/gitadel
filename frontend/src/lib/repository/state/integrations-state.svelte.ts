import { toast } from "svelte-sonner";

import {
  bindIntegrationSource,
  catalogWithEnvironment,
  catalogWithProject,
  createDokployEnvironment,
  createDokployProject,
  createDokployResource,
  createIntegrationSource,
  deployRepositoryIntegration,
  disconnectIntegrationSource,
  findDefaultEnvironment,
  linkDokployResource,
  loadDokployCatalog,
  loadIntegrationSource,
  loadRepositoryIntegration,
  parseDokployResourceLink,
  revealNamespaceCredential,
  testNamespaceIntegration,
  updateNamespaceConnection,
  updateRepositoryIntegration,
  withSourceBinding,
  type DokployProject,
  type DokployRemoteCatalog,
  type IntegrationSourceBinding,
  type IntegrationSourceConnection,
  type IntegrationTestResult,
  type NamespaceIntegration,
  type RemoteResource,
  type RepositoryIntegration,
} from "$lib/api/integrations.js";
import { errorMessage } from "$lib/api/transport.js";
import type { ConnectionEditorValue } from "$lib/components/integrations/integration-connection-editor.svelte";
import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

export type StagedConnectionCallbacks = {
  /** The staged connection's source binding was loaded or changed. */
  onbinding(
    connection: NamespaceIntegration,
    binding: IntegrationSourceBinding | null,
  ): void;
  /** The staged connection's details were saved. */
  onsaved(connection: NamespaceIntegration): void;
};

/**
 * A namespace connection created from inside a repository, while its
 * Dokploy source is being connected before the repository integration is
 * added.
 */
export class StagedConnectionState {
  connection = $state<NamespaceIntegration | null>(null);
  source = $state<IntegrationSourceConnection | null>(null);
  loading = $state(false);
  working = $state(false);
  error = $state<string | null>(null);

  readonly #namespace: () => string | null;
  readonly #callbacks: StagedConnectionCallbacks;

  constructor(
    namespace: () => string | null,
    callbacks: StagedConnectionCallbacks,
  ) {
    this.#namespace = namespace;
    this.#callbacks = callbacks;
  }

  reset(): void {
    this.connection = null;
    this.source = null;
    this.error = null;
  }

  stage(connection: NamespaceIntegration): void {
    this.connection = connection;
    this.source = null;
    this.error = null;
  }

  /** The staged connection and its namespace, or the reason there is none. */
  #target(): { namespace: string; connection: NamespaceIntegration } {
    if (!this.connection) throw new Error("No connection is selected.");
    const namespace = this.#namespace();
    if (!namespace) throw new Error("The repository namespace is unavailable.");
    return { namespace, connection: this.connection };
  }

  #update(response: IntegrationSourceConnection | null): void {
    this.source = response;
    if (!this.connection) return;
    const binding = response?.binding ?? null;
    this.connection = withSourceBinding(this.connection, binding);
    this.#callbacks.onbinding(this.connection, binding);
  }

  async load(): Promise<void> {
    if (!this.connection) return;
    this.loading = true;
    this.error = null;
    try {
      const { namespace, connection } = this.#target();
      this.#update(await loadIntegrationSource(namespace, connection.id));
    } catch (caught) {
      this.error = errorMessage(
        caught,
        "Could not load the repository source.",
      );
    } finally {
      this.loading = false;
    }
  }

  async bind(sourceId: string): Promise<void> {
    this.working = true;
    this.error = null;
    try {
      const { namespace, connection } = this.#target();
      this.#update(
        await bindIntegrationSource(namespace, connection.id, sourceId),
      );
    } catch (caught) {
      toast.error(
        errorMessage(caught, "Could not connect the Dokploy source."),
      );
    } finally {
      this.working = false;
    }
  }

  async create(sourceName: string): Promise<void> {
    this.working = true;
    this.error = null;
    try {
      const { namespace, connection } = this.#target();
      this.#update(
        await createIntegrationSource(namespace, connection.id, sourceName),
      );
    } catch (caught) {
      toast.error(errorMessage(caught, "Could not create the Dokploy source."));
    } finally {
      this.working = false;
    }
  }

  async disconnect(): Promise<void> {
    this.working = true;
    this.error = null;
    try {
      const { namespace, connection } = this.#target();
      await disconnectIntegrationSource(namespace, connection.id);
      this.#update(null);
      await this.load();
    } catch (caught) {
      toast.error(
        errorMessage(caught, "Could not disconnect the Dokploy source."),
      );
    } finally {
      this.working = false;
    }
  }

  async save(value: ConnectionEditorValue): Promise<void> {
    const { namespace, connection } = this.#target();
    const updated = await updateNamespaceConnection(namespace, connection.id, {
      name: value.name,
      url: value.url,
      internal_url: value.internalUrl,
      api_key: value.apiKey || null,
    });
    this.connection = updated;
    this.#callbacks.onsaved(updated);
  }

  async reveal(): Promise<string> {
    const { namespace, connection } = this.#target();
    return revealNamespaceCredential(namespace, connection.id);
  }

  async test(): Promise<IntegrationTestResult> {
    const { namespace, connection } = this.#target();
    return testNamespaceIntegration(namespace, connection.id);
  }
}

export type ConfigureStep = "kind" | "create" | "link";
export type SetupStep = "environments" | "resources" | ConfigureStep;
export const DOKPLOY_SERVER = "__dokploy__";
export const NEW_DOKPLOY_PROJECT = "__new_project__";

/** The Dokploy setup wizard of one repository integration. */
export class DokployConfigureState {
  readonly page: RepositoryPageState;

  detail = $state.raw<RepositoryIntegration | null>(null);
  catalog = $state.raw<DokployRemoteCatalog | null>(null);
  loading = $state(true);
  catalogLoading = $state(false);
  working = $state(false);
  loadError = $state<string | null>(null);
  operationError = $state<string | null>(null);
  showSetup = $state(false);
  setupStep = $state<SetupStep>("environments");
  retainedConfigureStep = $state<ConfigureStep | null>(null);
  selectedProjectId = $state("");
  selectedEnvironmentId = $state("");
  selectedResource = $state.raw<RemoteResource | null>(null);
  selectedKind = $state<RemoteResource["kind"]>("application");

  environmentDialogOpen = $state(false);
  environmentProjectId = $state("");
  environmentProjectName = $state("");
  environmentName = $state("");
  environmentDescription = $state("");

  resourceName = $state("");
  branch = $state("");
  repositoryPath = $state("/");
  composePath = $state("./docker-compose.yml");
  serverId = $state(DOKPLOY_SERVER);

  unlinkDialogOpen = $state(false);

  // Deferred so `page`, assigned in the constructor, is read lazily.
  readonly id = $derived.by(() => this.page.integrationProvider ?? "");
  readonly link = $derived(parseDokployResourceLink(this.detail?.resource));
  readonly selectedProject = $derived(
    this.catalog?.projects.find(
      (project) => project.id === this.selectedProjectId,
    ) ?? null,
  );
  readonly selectedEnvironment = $derived(
    this.selectedProject?.environments.find(
      (environment) => environment.id === this.selectedEnvironmentId,
    ) ?? null,
  );

  constructor(page: RepositoryPageState) {
    this.page = page;
  }

  async load(): Promise<void> {
    this.loading = true;
    this.loadError = null;
    try {
      this.detail = await loadRepositoryIntegration(
        this.page.namespace,
        this.page.name,
        this.id,
      );
      this.showSetup = !parseDokployResourceLink(this.detail.resource);
      await this.loadCatalog();
    } catch (error) {
      this.loadError = errorMessage(error, "Could not reach Gitadel.");
    } finally {
      this.loading = false;
    }
  }

  async loadCatalog(): Promise<void> {
    this.catalogLoading = true;
    try {
      this.catalog = await loadDokployCatalog(
        this.page.namespace,
        this.page.name,
        this.id,
      );
    } catch (error) {
      if (!this.link)
        this.loadError = errorMessage(error, "Could not reach Gitadel.");
      else
        this.operationError = errorMessage(error, "Could not reach Gitadel.");
    } finally {
      this.catalogLoading = false;
    }
  }

  resetMessages(): void {
    this.operationError = null;
  }

  chooseEnvironment(project: DokployProject, environmentId: string): void {
    this.resetMessages();
    const environmentChanged =
      this.selectedProjectId !== project.id ||
      this.selectedEnvironmentId !== environmentId;
    this.selectedProjectId = project.id;
    this.selectedEnvironmentId = environmentId;
    if (environmentChanged) {
      this.selectedResource = null;
      this.retainedConfigureStep = null;
    }
    this.setupStep = "resources";
  }

  beginResourceKind(): void {
    this.resetMessages();
    this.selectedResource = null;
    this.retainedConfigureStep = "kind";
    this.setupStep = "kind";
  }

  beginEnvironment(): void {
    this.resetMessages();
    this.environmentProjectId =
      this.catalog?.projects[0]?.id ?? NEW_DOKPLOY_PROJECT;
    this.environmentProjectName = "";
    this.environmentName = "";
    this.environmentDescription = "";
    this.environmentDialogOpen = true;
  }

  async createEnvironment(): Promise<void> {
    if (this.working) return;
    this.working = true;
    this.operationError = null;
    let createdProject: DokployProject | null = null;
    try {
      let projectId = this.environmentProjectId;
      if (projectId === NEW_DOKPLOY_PROJECT) {
        createdProject = await createDokployProject(
          this.page.namespace,
          this.page.name,
          this.id,
          {
            name: this.environmentProjectName,
            description: null,
          },
        );
        projectId = createdProject.id;
      }

      const defaultEnvironment = findDefaultEnvironment(
        createdProject,
        this.environmentName,
      );
      const created = defaultEnvironment
        ? {
            ...defaultEnvironment,
            project_id: projectId,
            project_name: createdProject?.name ?? projectId,
          }
        : await createDokployEnvironment(
            this.page.namespace,
            this.page.name,
            this.id,
            {
              project_id: projectId,
              name: this.environmentName,
              description: this.environmentDescription || null,
            },
          );
      if (this.catalog) {
        this.catalog = catalogWithEnvironment(
          this.catalog,
          created,
          createdProject,
        );
      }
      this.environmentDialogOpen = false;
      this.selectedProjectId = created.project_id;
      this.selectedEnvironmentId = created.id;
      this.selectedResource = null;
      this.retainedConfigureStep = null;
      this.setupStep = "resources";
      toast.success(`Created ${created.name}.`);
    } catch (error) {
      if (createdProject && this.catalog) {
        this.catalog = catalogWithProject(this.catalog, createdProject);
        this.environmentProjectId = createdProject.id;
      }
      toast.error(errorMessage(error, "Could not reach Gitadel."));
    } finally {
      this.working = false;
    }
  }

  beginExisting(resource: RemoteResource): void {
    this.resetMessages();
    const resourceChanged = this.selectedResource?.id !== resource.id;
    this.selectedResource = resource;
    if (resourceChanged) {
      this.branch = this.page.repository?.default_branch ?? "main";
      this.repositoryPath = "/";
      this.composePath = "./docker-compose.yml";
    }
    this.retainedConfigureStep = "link";
    this.setupStep = "link";
  }

  beginCreate(kind: RemoteResource["kind"]): void {
    this.resetMessages();
    const resourceChanged =
      this.selectedResource !== null ||
      this.retainedConfigureStep !== "create" ||
      this.selectedKind !== kind;
    this.selectedResource = null;
    this.selectedKind = kind;
    if (resourceChanged) {
      this.resourceName = this.page.repository?.name ?? "";
      this.branch = this.page.repository?.default_branch ?? "main";
      this.repositoryPath = "/";
      this.composePath = "./docker-compose.yml";
      this.serverId = DOKPLOY_SERVER;
    }
    this.retainedConfigureStep = "create";
    this.setupStep = "create";
  }

  async linkExisting(): Promise<void> {
    if (!this.selectedResource || this.working) return;
    this.working = true;
    this.operationError = null;
    try {
      this.detail = await linkDokployResource(
        this.page.namespace,
        this.page.name,
        this.id,
        {
          kind: this.selectedResource.kind,
          id: this.selectedResource.id,
          branch: this.branch,
          repository_path: this.repositoryPath,
          compose_path: this.composePath,
        },
      );
      this.showSetup = false;
      toast.success(`${this.selectedResource.name} is connected.`);
    } catch (error) {
      toast.error(errorMessage(error, "Could not reach Gitadel."));
    } finally {
      this.working = false;
    }
  }

  async createResource(): Promise<void> {
    if (!this.selectedEnvironment || this.working) return;
    this.working = true;
    this.operationError = null;
    try {
      this.detail = await createDokployResource(
        this.page.namespace,
        this.page.name,
        this.id,
        {
          kind: this.selectedKind,
          name: this.resourceName,
          environment_id: this.selectedEnvironment.id,
          branch: this.branch,
          server_id: this.serverId === DOKPLOY_SERVER ? null : this.serverId,
          repository_path: this.repositoryPath,
          compose_path: this.composePath,
        },
      );
      this.showSetup = false;
      toast.success(`${this.resourceName} was created and connected.`);
    } catch (error) {
      toast.error(errorMessage(error, "Could not reach Gitadel."));
    } finally {
      this.working = false;
    }
  }

  async setEnabled(enabled: boolean): Promise<void> {
    if (!this.detail || this.working) return;
    const previous = this.detail;
    this.detail = { ...this.detail, enabled };
    this.working = true;
    this.operationError = null;
    try {
      this.detail = await updateRepositoryIntegration(
        this.page.namespace,
        this.page.name,
        this.id,
        { enabled },
      );
      toast.success(
        enabled
          ? "Push deployments are enabled."
          : "Push deployments are disabled.",
      );
    } catch (error) {
      this.detail = previous;
      toast.error(errorMessage(error, "Could not reach Gitadel."));
    } finally {
      this.working = false;
    }
  }

  async deployNow(): Promise<void> {
    if (this.working) return;
    this.working = true;
    this.operationError = null;
    try {
      const result = await deployRepositoryIntegration(
        this.page.namespace,
        this.page.name,
        this.id,
      );
      toast.success(result.summary);
    } catch (error) {
      toast.error(errorMessage(error, "Could not reach Gitadel."));
    } finally {
      this.working = false;
    }
  }

  changeResource(): void {
    this.resetMessages();
    this.showSetup = true;
    this.setupStep = "environments";
    this.selectedProjectId = "";
    this.selectedEnvironmentId = "";
    this.selectedResource = null;
    this.retainedConfigureStep = null;
  }

  async unlinkResource(): Promise<void> {
    if (this.working) return;
    this.working = true;
    this.operationError = null;
    try {
      this.detail = await updateRepositoryIntegration(
        this.page.namespace,
        this.page.name,
        this.id,
        { enabled: false, resource: null },
      );
      this.unlinkDialogOpen = false;
      this.showSetup = true;
      this.setupStep = "environments";
      toast.success(
        "The Dokploy resource was unlinked. It was not deleted from Dokploy.",
      );
    } catch (error) {
      toast.error(errorMessage(error, "Could not reach Gitadel."));
    } finally {
      this.working = false;
    }
  }

  showEnvironmentStep = (event: MouseEvent): void => {
    event.preventDefault();
    this.resetMessages();
    this.setupStep = "environments";
  };

  showResourceStep = (event: MouseEvent): void => {
    event.preventDefault();
    if (!this.selectedEnvironment) return;
    this.resetMessages();
    this.setupStep = "resources";
  };

  showConfigureStep = (event: MouseEvent): void => {
    event.preventDefault();
    if (!this.retainedConfigureStep) return;
    this.resetMessages();
    this.setupStep = this.retainedConfigureStep;
  };
}
