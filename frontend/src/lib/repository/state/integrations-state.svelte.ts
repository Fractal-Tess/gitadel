import { toast } from "svelte-sonner";

import {
  DOKPLOY_SERVER,
  NEW_DOKPLOY_PROJECT,
  bindIntegrationSource,
  buildChoices,
  defaultSetupForm,
  loadDeploymentStatus,
  loadDeploymentSuggestions,
  setupDeployment,
  setupFormProblem,
  setupRequestBody,
  shouldPollDeployment,
  suggestedPort,
  updateNamespaceInternalUrl,
  verifyIntegrationSource,
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
  type DeploymentStatus,
  type DeploymentSuggestions,
  type DokployProject,
  type DokployRemoteCatalog,
  type SetupForm,
  type SourceCheck,
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
  /** The latest reachability check; null until one has run. */
  check = $state.raw<SourceCheck | null>(null);
  checking = $state(false);
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
    this.check = null;
    this.error = null;
  }

  stage(connection: NamespaceIntegration): void {
    this.connection = connection;
    this.source = null;
    this.check = null;
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
    if (!response?.binding) this.check = null;
    else if (response.check) this.check = response.check;
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
      this.error = errorMessage(caught, "Could not use that Dokploy provider.");
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
      this.error = errorMessage(caught, "Could not connect Gitadel to Dokploy.");
    } finally {
      this.working = false;
    }
  }

  async verify(): Promise<void> {
    this.checking = true;
    this.error = null;
    try {
      const { namespace, connection } = this.#target();
      this.check = await verifyIntegrationSource(namespace, connection.id);
      if (this.connection && this.check.internal_url) {
        this.connection = {
          ...this.connection,
          internal_url: this.check.internal_url,
        };
      }
    } catch (caught) {
      this.error = errorMessage(
        caught,
        "Could not check whether Dokploy reaches Gitadel.",
      );
    } finally {
      this.checking = false;
    }
  }

  /** Point Dokploy at `internalUrl`, then check again. */
  async setInternalUrl(internalUrl: string): Promise<void> {
    this.checking = true;
    this.error = null;
    try {
      const { namespace, connection } = this.#target();
      const updated = await updateNamespaceInternalUrl(
        namespace,
        connection.id,
        internalUrl,
      );
      this.connection = {
        ...updated,
        source: updated.source ?? this.connection?.source,
      };
    } catch (caught) {
      this.checking = false;
      this.error = errorMessage(caught, "Could not save the address.");
      return;
    }
    await this.verify();
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
    this.connection = {
      ...updated,
      source: updated.source ?? connection.source,
    };
    // The address may have changed; prove it again.
    this.check = null;
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
export { DOKPLOY_SERVER, NEW_DOKPLOY_PROJECT };

const DEPLOYMENT_POLL_INTERVAL_MS = 3_000;
const DEPLOYMENT_POLL_LIMIT_MS = 10 * 60 * 1_000;

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
  environmentError = $state<string | null>(null);
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

  /** `setup` is the one-form flow; `link` is the step-by-step wizard. */
  setupMode = $state<"setup" | "link">("setup");
  suggestions = $state.raw<DeploymentSuggestions | null>(null);
  suggestionsError = $state<string | null>(null);
  form = $state<SetupForm>(
    defaultSetupForm({
      repositoryName: "",
      defaultBranch: null,
      suggestions: null,
      catalog: null,
    }),
  );
  setupLoading = $state(false);
  setupWorking = $state(false);
  setupError = $state<string | null>(null);
  setupResult = $state.raw<{ steps: string[]; warning: string | null } | null>(
    null,
  );

  deployment = $state.raw<DeploymentStatus | null>(null);
  deploymentError = $state<string | null>(null);
  deploymentLoading = $state(false);
  /** True after starting a deployment until Dokploy lists it. */
  awaitingDeployment = $state(false);
  /** True when polling gave up after the time limit. */
  deploymentPollExpired = $state(false);
  /** Bumped to restart deployment polling. */
  deploymentWatch = $state(0);
  #awaitingPrevious: string | null = null;

  // Deferred so `page`, assigned in the constructor, is read lazily.
  readonly id = $derived.by(() => this.page.integrationProvider ?? "");
  readonly link = $derived(parseDokployResourceLink(this.detail?.resource));
  readonly choices = $derived(
    buildChoices(this.suggestions?.suggestions ?? []),
  );
  readonly selectedChoice = $derived(
    this.choices.find((choice) => choice.id === this.form.build),
  );
  readonly formProblem = $derived(setupFormProblem(this.form, this.choices));
  readonly formProject = $derived(
    this.catalog?.projects.find((project) => project.id === this.form.project) ??
      null,
  );
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
      if (this.showSetup) {
        await Promise.all([this.loadCatalog(), this.loadSuggestions()]);
        this.resetForm();
      } else {
        await this.loadCatalog();
      }
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

  async loadSuggestions(): Promise<void> {
    this.suggestionsError = null;
    try {
      this.suggestions = await loadDeploymentSuggestions(
        this.page.namespace,
        this.page.name,
        this.id,
      );
    } catch (error) {
      this.suggestions = null;
      this.suggestionsError = errorMessage(
        error,
        "Could not read the repository to suggest a build.",
      );
    }
  }

  /** Fill the setup form from the suggestions and the Dokploy catalog. */
  resetForm(): void {
    this.form = defaultSetupForm({
      repositoryName: this.page.repository?.name ?? this.page.name,
      defaultBranch: this.page.repository?.default_branch,
      suggestions: this.suggestions,
      catalog: this.catalog,
    });
  }

  chooseBuild(id: string): void {
    this.form.build = id;
    this.form.domainPort = suggestedPort(
      this.choices.find((choice) => choice.id === id),
    );
  }

  /** Show the one-form setup, loading what it needs. */
  async openSetup(): Promise<void> {
    this.resetMessages();
    this.setupError = null;
    this.setupResult = null;
    this.setupMode = "setup";
    this.showSetup = true;
    const pending: Promise<void>[] = [];
    if (!this.suggestions) pending.push(this.loadSuggestions());
    if (!this.catalog) pending.push(this.loadCatalog());
    if (pending.length > 0) {
      this.setupLoading = true;
      try {
        await Promise.all(pending);
      } finally {
        this.setupLoading = false;
      }
    }
    this.resetForm();
  }

  /** Switch to linking an existing resource with the step-by-step wizard. */
  openLinkWizard(): void {
    this.resetMessages();
    this.setupError = null;
    this.setupMode = "link";
    this.setupStep = "environments";
  }

  async submitSetup(): Promise<void> {
    if (this.setupWorking || this.formProblem) return;
    this.setupWorking = true;
    this.setupError = null;
    this.resetMessages();
    const deploy = this.form.deploy;
    try {
      const result = await setupDeployment(
        this.page.namespace,
        this.page.name,
        this.id,
        setupRequestBody(this.form, this.choices),
      );
      this.detail = result.integration;
      this.setupResult = { steps: result.steps, warning: result.warning };
      this.showSetup = false;
      this.deployment = null;
      // The first deployment only starts when every earlier step worked.
      this.#awaitDeployment(deploy && result.warning === null, null);
      this.deploymentWatch += 1;
      void this.loadCatalog();
    } catch (error) {
      this.setupError = errorMessage(error, "Dokploy setup failed.");
    } finally {
      this.setupWorking = false;
    }
  }

  #awaitDeployment(awaiting: boolean, previous: string | null): void {
    this.awaitingDeployment = awaiting;
    this.#awaitingPrevious = previous;
  }

  async refreshDeployment(): Promise<void> {
    this.deploymentLoading = true;
    try {
      const status = await loadDeploymentStatus(
        this.page.namespace,
        this.page.name,
        this.id,
      );
      this.deployment = status;
      this.deploymentError = null;
      if (
        this.awaitingDeployment &&
        (status.deployment?.created_at ?? null) !== this.#awaitingPrevious
      ) {
        this.#awaitDeployment(false, null);
      }
    } catch (error) {
      this.deploymentError = errorMessage(
        error,
        "Could not read the deployment status from Dokploy.",
      );
    } finally {
      this.deploymentLoading = false;
    }
  }

  /**
   * Poll the newest deployment every few seconds while it builds, for at
   * most ten minutes. Returns the function that stops polling.
   */
  watchDeployment(): () => void {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const started = Date.now();
    const tick = async () => {
      if (stopped) return;
      this.deploymentPollExpired = false;
      await this.refreshDeployment();
      if (stopped || this.deploymentError) return;
      const awaiting = this.awaitingDeployment
        ? { previous: this.#awaitingPrevious }
        : null;
      if (!shouldPollDeployment(this.deployment, awaiting)) return;
      if (Date.now() - started >= DEPLOYMENT_POLL_LIMIT_MS) {
        this.deploymentPollExpired = true;
        return;
      }
      timer = setTimeout(() => void tick(), DEPLOYMENT_POLL_INTERVAL_MS);
    };
    // Deferred so the caller's effect does not track what `tick` reads.
    timer = setTimeout(() => void tick(), 0);
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
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
    this.environmentProjectName = this.page.repository?.name ?? "";
    // Every Dokploy project has a production environment; choosing it selects
    // the existing one instead of creating a duplicate.
    this.environmentName = "production";
    this.environmentDescription = "";
    this.environmentError = null;
    this.environmentDialogOpen = true;
  }

  async createEnvironment(): Promise<void> {
    if (this.working) return;
    this.working = true;
    this.operationError = null;
    this.environmentError = null;
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
      toast.success(
        defaultEnvironment || ("existing" in created && created.existing)
          ? `Selected ${created.name} in ${created.project_name}. Next, choose a resource.`
          : `Created ${created.name} in ${created.project_name}. Next, choose a resource.`,
      );
    } catch (error) {
      if (createdProject && this.catalog) {
        this.catalog = catalogWithProject(this.catalog, createdProject);
        this.environmentProjectId = createdProject.id;
      }
      this.environmentError = errorMessage(
        error,
        "Could not create the environment.",
      );
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
      this.setupResult = null;
      toast.success(`${this.selectedResource.name} is connected.`, {
        description:
          "Deploy on push is off. Check the build settings in Dokploy, then turn it on here.",
      });
    } catch (error) {
      this.operationError = errorMessage(
        error,
        "Could not connect the resource.",
      );
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
      this.setupResult = null;
      toast.success(`${this.resourceName} was created in Dokploy.`, {
        description:
          "Next, open it in Dokploy to choose a build type, domain, and variables. Then deploy it or turn on Deploy on push here.",
      });
    } catch (error) {
      this.operationError = errorMessage(
        error,
        "Could not create the resource.",
      );
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
      this.#awaitDeployment(
        true,
        this.deployment?.deployment?.created_at ?? null,
      );
      this.deploymentWatch += 1;
    } catch (error) {
      toast.error(errorMessage(error, "Could not reach Gitadel."));
    } finally {
      this.working = false;
    }
  }

  changeResource(): void {
    this.resetMessages();
    this.showSetup = true;
    this.setupMode = "link";
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
      this.deployment = null;
      this.#awaitDeployment(false, null);
      this.setupStep = "environments";
      void this.openSetup();
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
