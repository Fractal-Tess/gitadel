<script lang="ts">
  import { toast } from "svelte-sonner";
  import AppWindow from "@lucide/svelte/icons/app-window";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import Box from "@lucide/svelte/icons/box";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import Layers3 from "@lucide/svelte/icons/layers-3";
  import Rocket from "@lucide/svelte/icons/rocket";
  import Unlink from "@lucide/svelte/icons/unlink";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import SiDocker from "@icons-pack/svelte-simple-icons/icons/SiDocker";

  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as Breadcrumb from "$lib/components/ui/breadcrumb/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Separator } from "$lib/components/ui/separator/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import {
    ApiFailure,
    jsonBody,
    requestJson,
  } from "$lib/api/transport.js";
  import {
    dokployCreatedEnvironmentSchema,
    dokployRemoteCatalogSchema,
    dokployProjectSchema,
    dokployResourceLinkSchema,
    integrationDeployResultSchema,
    repositoryIntegrationSchema,
    type DokployProject,
    type DokployRemoteCatalog,
    type RemoteResource,
    type RepositoryIntegration,
  } from "$lib/api/integrations.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  type ConfigureStep = "kind" | "create" | "link";
  type SetupStep = "environments" | "resources" | ConfigureStep;
  const DOKPLOY_SERVER = "__dokploy__";
  const NEW_DOKPLOY_PROJECT = "__new_project__";

  let detail = $state.raw<RepositoryIntegration | null>(null);
  let catalog = $state.raw<DokployRemoteCatalog | null>(null);
  let loading = $state(true);
  let catalogLoading = $state(false);
  let working = $state(false);
  let loadError = $state<string | null>(null);
  let operationError = $state<string | null>(null);
  let showSetup = $state(false);
  let setupStep = $state<SetupStep>("environments");
  let retainedConfigureStep = $state<ConfigureStep | null>(null);
  let selectedProjectId = $state("");
  let selectedEnvironmentId = $state("");
  let selectedResource = $state.raw<RemoteResource | null>(null);
  let selectedKind = $state<RemoteResource["kind"]>("application");

  let environmentDialogOpen = $state(false);
  let environmentProjectId = $state("");
  let environmentProjectName = $state("");
  let environmentName = $state("");
  let environmentDescription = $state("");

  let resourceName = $state("");
  let branch = $state("");
  let repositoryPath = $state("/");
  let composePath = $state("./docker-compose.yml");
  let serverId = $state(DOKPLOY_SERVER);

  let unlinkDialogOpen = $state(false);

  const id = $derived(repository.integrationProvider ?? "");
  const basePath = $derived(
    `/api/v1/repos/${encodeURIComponent(repository.namespace)}/${encodeURIComponent(repository.name)}/integrations/${encodeURIComponent(id)}`,
  );
  const link = $derived.by(() => {
    if (!detail?.resource) return null;
    const result = dokployResourceLinkSchema.safeParse(detail.resource);
    return result.success ? result.data : null;
  });
  const selectedProject = $derived(
    catalog?.projects.find((project) => project.id === selectedProjectId) ??
      null,
  );
  const selectedEnvironment = $derived(
    selectedProject?.environments.find(
      (environment) => environment.id === selectedEnvironmentId,
    ) ?? null,
  );

  $effect(() => {
    void load();
  });

  function errorMessage(error: unknown): string {
    return error instanceof ApiFailure
      ? error.message
      : "Could not reach Gitadel.";
  }

  async function load() {
    loading = true;
    loadError = null;
    try {
      detail = await requestJson(basePath, repositoryIntegrationSchema);
      const parsed = detail.resource
        ? dokployResourceLinkSchema.safeParse(detail.resource)
        : null;
      showSetup = !parsed?.success;
      await loadCatalog();
    } catch (error) {
      loadError = errorMessage(error);
    } finally {
      loading = false;
    }
  }

  async function loadCatalog() {
    catalogLoading = true;
    try {
      catalog = await requestJson(
        `${basePath}/resources`,
        dokployRemoteCatalogSchema,
      );
    } catch (error) {
      if (!link) loadError = errorMessage(error);
      else operationError = errorMessage(error);
    } finally {
      catalogLoading = false;
    }
  }

  function openExternal(url: string) {
    window.open(url, "_blank", "noopener,noreferrer");
  }

  function resetMessages() {
    operationError = null;
  }

  function chooseEnvironment(project: DokployProject, environmentId: string) {
    resetMessages();
    const environmentChanged =
      selectedProjectId !== project.id ||
      selectedEnvironmentId !== environmentId;
    selectedProjectId = project.id;
    selectedEnvironmentId = environmentId;
    if (environmentChanged) {
      selectedResource = null;
      retainedConfigureStep = null;
    }
    setupStep = "resources";
  }

  function beginResourceKind() {
    resetMessages();
    selectedResource = null;
    retainedConfigureStep = "kind";
    setupStep = "kind";
  }

  function beginEnvironment() {
    resetMessages();
    environmentProjectId = catalog?.projects[0]?.id ?? NEW_DOKPLOY_PROJECT;
    environmentProjectName = "";
    environmentName = "";
    environmentDescription = "";
    environmentDialogOpen = true;
  }

  async function createEnvironment() {
    if (working) return;
    working = true;
    operationError = null;
    let createdProject: DokployProject | null = null;
    try {
      let projectId = environmentProjectId;
      if (projectId === NEW_DOKPLOY_PROJECT) {
        createdProject = await requestJson(
          `${basePath}/projects`,
          dokployProjectSchema,
          {
            method: "POST",
            body: jsonBody({
              name: environmentProjectName,
              description: null,
            }),
          },
        );
        projectId = createdProject.id;
      }

      const defaultEnvironment = createdProject?.environments.find(
        (environment) =>
          environment.name.localeCompare(environmentName.trim(), undefined, {
            sensitivity: "accent",
          }) === 0,
      );
      const created = defaultEnvironment
        ? {
            ...defaultEnvironment,
            project_id: projectId,
            project_name: createdProject?.name ?? projectId,
          }
        : await requestJson(
            `${basePath}/environments`,
            dokployCreatedEnvironmentSchema,
            {
              method: "POST",
              body: jsonBody({
                project_id: projectId,
                name: environmentName,
                description: environmentDescription || null,
              }),
            },
          );
      if (catalog) {
        const projects = createdProject
          ? [
              createdProject,
              ...catalog.projects.filter(
                (project) => project.id !== createdProject?.id,
              ),
            ]
          : catalog.projects;
        catalog = {
          ...catalog,
          projects: projects.map((project) =>
            project.id !== created.project_id ||
            project.environments.some(
              (environment) => environment.id === created.id,
            )
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
      environmentDialogOpen = false;
      selectedProjectId = created.project_id;
      selectedEnvironmentId = created.id;
      selectedResource = null;
      retainedConfigureStep = null;
      setupStep = "resources";
      toast.success(`Created ${created.name}.`);
    } catch (error) {
      if (createdProject && catalog) {
        catalog = {
          ...catalog,
          projects: [
            createdProject,
            ...catalog.projects.filter(
              (project) => project.id !== createdProject?.id,
            ),
          ],
        };
        environmentProjectId = createdProject.id;
      }
      toast.error(errorMessage(error));
    } finally {
      working = false;
    }
  }

  function beginExisting(resource: RemoteResource) {
    resetMessages();
    const resourceChanged = selectedResource?.id !== resource.id;
    selectedResource = resource;
    if (resourceChanged) {
      branch = repository.repository?.default_branch ?? "main";
      repositoryPath = "/";
      composePath = "./docker-compose.yml";
    }
    retainedConfigureStep = "link";
    setupStep = "link";
  }

  function beginCreate(kind: RemoteResource["kind"]) {
    resetMessages();
    const resourceChanged =
      selectedResource !== null ||
      retainedConfigureStep !== "create" ||
      selectedKind !== kind;
    selectedResource = null;
    selectedKind = kind;
    if (resourceChanged) {
      resourceName = repository.repository?.name ?? "";
      branch = repository.repository?.default_branch ?? "main";
      repositoryPath = "/";
      composePath = "./docker-compose.yml";
      serverId = DOKPLOY_SERVER;
    }
    retainedConfigureStep = "create";
    setupStep = "create";
  }

  async function linkExisting() {
    if (!selectedResource || working) return;
    working = true;
    operationError = null;
    try {
      detail = await requestJson(
        `${basePath}/resources/link`,
        repositoryIntegrationSchema,
        {
          method: "POST",
          body: jsonBody({
            kind: selectedResource.kind,
            id: selectedResource.id,
            branch,
            repository_path: repositoryPath,
            compose_path: composePath,
          }),
        },
      );
      showSetup = false;
      toast.success(`${selectedResource.name} is connected.`);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      working = false;
    }
  }

  async function createResource() {
    if (!selectedEnvironment || working) return;
    working = true;
    operationError = null;
    try {
      detail = await requestJson(
        `${basePath}/resources`,
        repositoryIntegrationSchema,
        {
          method: "POST",
          body: jsonBody({
            kind: selectedKind,
            name: resourceName,
            environment_id: selectedEnvironment.id,
            branch,
            server_id: serverId === DOKPLOY_SERVER ? null : serverId,
            repository_path: repositoryPath,
            compose_path: composePath,
          }),
        },
      );
      showSetup = false;
      toast.success(`${resourceName} was created and connected.`);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      working = false;
    }
  }

  async function setEnabled(enabled: boolean) {
    if (!detail || working) return;
    const previous = detail;
    detail = { ...detail, enabled };
    working = true;
    operationError = null;
    try {
      detail = await requestJson(basePath, repositoryIntegrationSchema, {
        method: "PUT",
        body: jsonBody({ enabled }),
      });
      toast.success(
        enabled ? "Push deployments are enabled." : "Push deployments are disabled.",
      );
    } catch (error) {
      detail = previous;
      toast.error(errorMessage(error));
    } finally {
      working = false;
    }
  }

  async function deployNow() {
    if (working) return;
    working = true;
    operationError = null;
    try {
      const result = await requestJson(
        `${basePath}/deploy`,
        integrationDeployResultSchema,
        { method: "POST", body: jsonBody({}) },
      );
      toast.success(result.summary);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      working = false;
    }
  }

  function changeResource() {
    resetMessages();
    showSetup = true;
    setupStep = "environments";
    selectedProjectId = "";
    selectedEnvironmentId = "";
    selectedResource = null;
    retainedConfigureStep = null;
  }

  async function unlinkResource() {
    if (working) return;
    working = true;
    operationError = null;
    try {
      detail = await requestJson(basePath, repositoryIntegrationSchema, {
        method: "PUT",
        body: jsonBody({ enabled: false, resource: null }),
      });
      unlinkDialogOpen = false;
      showSetup = true;
      setupStep = "environments";
      toast.success(
        "The Dokploy resource was unlinked. It was not deleted from Dokploy.",
      );
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      working = false;
    }
  }

  function showEnvironmentStep(event: MouseEvent) {
    event.preventDefault();
    resetMessages();
    setupStep = "environments";
  }

  function showResourceStep(event: MouseEvent) {
    event.preventDefault();
    if (!selectedEnvironment) return;
    resetMessages();
    setupStep = "resources";
  }

  function showConfigureStep(event: MouseEvent) {
    event.preventDefault();
    if (!retainedConfigureStep) return;
    resetMessages();
    setupStep = retainedConfigureStep;
  }

  function statusLabel(status: string | null): string {
    if (!status) return "Available";
    return status
      .replaceAll("_", " ")
      .replace(/\b\w/g, (character) => character.toUpperCase());
  }
</script>

<div class="space-y-6">
  <header class="flex items-center gap-4">
    <Button
      variant="ghost"
      size="icon"
      aria-label="Back to integrations"
      onclick={() =>
        repository.navigate("settings", { settingsTab: "integrations" })}
    >
      <ArrowLeft class="size-4" />
    </Button>
    <div class="min-w-0">
      <h1 class="truncate text-lg font-semibold">Dokploy deployment</h1>
      <p class="text-sm text-muted-foreground">
        Connect this repository to one Application or Docker Compose resource.
      </p>
    </div>
  </header>

  {#if loading}
    <div
      class="flex min-h-48 items-center justify-center rounded-xl border bg-card"
    >
      <Spinner class="size-5 animate-spin text-muted-foreground" />
    </div>
  {:else if loadError}
    <Alert.Root variant="destructive">
      <Alert.Title>Integration unavailable</Alert.Title>
      <Alert.Description>{loadError}</Alert.Description>
    </Alert.Root>
  {:else if detail && link && !showSetup}
    <div class="grid gap-4 xl:grid-cols-2">
      <IntegrationConnectionCard
        name={link.name}
        provider="dokploy"
        providerName={detail.connection_name}
        subtitle={`${link.project_name} / ${link.environment_name}`}
        description={`Deploys ${repository.name} from ${link.branch}. Build, domains, variables, and runtime settings stay in Dokploy.`}
        enabled={detail.enabled}
        statusLabel={detail.enabled ? "Push delivery on" : "Push delivery off"}
        detailLabel={link.kind === "application"
          ? "Application"
          : "Docker Compose"}
        busy={working}
        onenabledchange={setEnabled}
        onconfigure={() => openExternal(link.external_url)}
        configureLabel="Open in Dokploy"
        configureExternal
        onremove={() => (unlinkDialogOpen = true)}
      />

      <article
        class="flex min-h-52 flex-col rounded-xl border bg-card p-5 shadow-sm"
      >
        <div class="flex items-start gap-4">
          <span
            class="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
          >
            <Rocket class="size-5" />
          </span>
          <div class="min-w-0 flex-1">
            <p class="font-semibold">Deployment delivery</p>
            <p class="mt-1 text-sm leading-5 text-muted-foreground">
              Gitadel sends matching push events. Dokploy decides how the linked
              resource builds and runs.
            </p>
          </div>
        </div>
        <div
          class="mt-5 flex items-center justify-between rounded-lg border bg-muted/30 px-3 py-2.5"
        >
          <div>
            <p class="text-sm font-medium">Deploy on push</p>
            <p class="text-xs text-muted-foreground">Branch: {link.branch}</p>
          </div>
          <Switch
            checked={detail.enabled}
            disabled={working}
            aria-label="Deploy on push"
            onCheckedChange={setEnabled}
          />
        </div>
        <div class="mt-auto grid gap-2 pt-5 sm:grid-cols-2">
          <Button
            class="gap-2"
            disabled={working}
            onclick={() => void deployNow()}
          >
            {#if working}<Spinner class="size-4 animate-spin" />{:else}<Rocket data-icon="inline-start" />{/if}
            Deploy now
          </Button>
          <Button variant="outline" disabled={working} onclick={changeResource}
            >Change resource</Button
          >
        </div>
      </article>
    </div>
  {:else if detail}
    <section id="dokploy-setup" class="space-y-5">
      <Breadcrumb.Root>
        <Breadcrumb.List
          class="grid w-full grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto_minmax(0,1fr)] gap-0 overflow-hidden rounded-lg border bg-card"
        >
          <Breadcrumb.Item class="relative min-w-0 self-stretch">
            <Layers3
              class="pointer-events-none absolute left-3 top-1/2 z-10 size-4 -translate-y-1/2 {setupStep ===
              'environments'
                ? 'text-primary'
                : 'text-muted-foreground'}"
              aria-hidden="true"
            />
            {#if setupStep === "environments"}
              <Breadcrumb.Page
                class="grid h-full w-full min-w-0 gap-0.5 bg-primary/10 py-3 pr-3 pl-10 text-primary ring-1 ring-inset ring-primary/30"
              >
                <span
                  class="truncate text-xs font-medium uppercase tracking-wide text-primary/80"
                  >Environment</span
                >
                <span class="truncate">Choose environment</span>
              </Breadcrumb.Page>
            {:else}
              <Breadcrumb.Link
                href="#dokploy-setup"
                class="grid h-full w-full min-w-0 gap-0.5 py-3 pr-3 pl-10 hover:bg-muted/50"
                onclick={showEnvironmentStep}
              >
                <span
                  class="truncate text-xs font-medium uppercase tracking-wide text-muted-foreground"
                  >Environment</span
                >
                <span class="truncate text-foreground">
                  {selectedProject?.name} / {selectedEnvironment?.name}
                </span>
              </Breadcrumb.Link>
            {/if}
          </Breadcrumb.Item>
          <Breadcrumb.Separator class="self-stretch">
            <Separator orientation="vertical" />
          </Breadcrumb.Separator>
          <Breadcrumb.Item class="relative min-w-0 self-stretch">
            <Box
              class="pointer-events-none absolute left-3 top-1/2 z-10 size-4 -translate-y-1/2 {setupStep ===
              'resources'
                ? 'text-primary'
                : 'text-muted-foreground'}"
              aria-hidden="true"
            />
            {#if setupStep === "resources"}
              <Breadcrumb.Page
                class="grid h-full w-full min-w-0 gap-0.5 bg-primary/10 py-3 pr-3 pl-10 text-primary ring-1 ring-inset ring-primary/30"
              >
                <span
                  class="truncate text-xs font-medium uppercase tracking-wide text-primary/80"
                  >Resource</span
                >
                <span class="truncate">Choose resource</span>
              </Breadcrumb.Page>
            {:else if selectedEnvironment}
              <Breadcrumb.Link
                href="#dokploy-setup"
                class="grid h-full w-full min-w-0 gap-0.5 py-3 pr-3 pl-10 hover:bg-muted/50"
                onclick={showResourceStep}
              >
                <span
                  class="truncate text-xs font-medium uppercase tracking-wide text-muted-foreground"
                  >Resource</span
                >
                <span class="truncate text-foreground">
                  {selectedResource?.name ??
                    (retainedConfigureStep === "kind" ||
                    retainedConfigureStep === "create"
                      ? "New resource"
                      : "Resource")}
                </span>
              </Breadcrumb.Link>
            {:else}
              <span class="grid h-full min-w-0 gap-0.5 py-3 pr-3 pl-10">
                <span
                  class="truncate text-xs font-medium uppercase tracking-wide"
                  >Resource</span
                >
                <span class="truncate">Choose next</span>
              </span>
            {/if}
          </Breadcrumb.Item>
          <Breadcrumb.Separator class="self-stretch">
            <Separator orientation="vertical" />
          </Breadcrumb.Separator>
          <Breadcrumb.Item class="relative min-w-0 self-stretch">
            <Settings2
              class="pointer-events-none absolute left-3 top-1/2 z-10 size-4 -translate-y-1/2 {setupStep !==
                'environments' && setupStep !== 'resources'
                ? 'text-primary'
                : 'text-muted-foreground'}"
              aria-hidden="true"
            />
            {#if setupStep !== "environments" && setupStep !== "resources"}
              <Breadcrumb.Page
                class="grid h-full w-full min-w-0 gap-0.5 bg-primary/10 py-3 pr-3 pl-10 text-primary ring-1 ring-inset ring-primary/30"
              >
                <span
                  class="truncate text-xs font-medium uppercase tracking-wide text-primary/80"
                  >Configure</span
                >
                <span class="truncate">
                  {setupStep === "kind"
                    ? "Choose resource type"
                    : setupStep === "create"
                      ? "Create resource"
                      : "Connect resource"}
                </span>
              </Breadcrumb.Page>
            {:else if retainedConfigureStep}
              <Breadcrumb.Link
                href="#dokploy-setup"
                class="grid h-full w-full min-w-0 gap-0.5 py-3 pr-3 pl-10 hover:bg-muted/50"
                onclick={showConfigureStep}
              >
                <span
                  class="truncate text-xs font-medium uppercase tracking-wide text-muted-foreground"
                  >Configure</span
                >
                <span class="truncate text-foreground">
                  {retainedConfigureStep === "kind"
                    ? "Choose resource type"
                    : retainedConfigureStep === "create"
                      ? "Create resource"
                      : "Connect resource"}
                </span>
              </Breadcrumb.Link>
            {:else}
              <span class="grid h-full min-w-0 gap-0.5 py-3 pr-3 pl-10">
                <span
                  class="truncate text-xs font-medium uppercase tracking-wide"
                  >Configure</span
                >
                <span class="truncate">Final step</span>
              </span>
            {/if}
          </Breadcrumb.Item>
        </Breadcrumb.List>
      </Breadcrumb.Root>
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <p class="sr-only">
            {#if setupStep === "environments"}Step 1 of 3{:else if setupStep === "resources"}Step
              2 of 3{:else}Step 3 of 3{/if}
          </p>
          <h2 class="mt-1 text-base font-semibold">
            {#if setupStep === "environments"}
              Choose an environment
            {:else if setupStep === "resources"}
              Choose a resource
            {:else if setupStep === "kind"}
              Choose a resource type
            {:else if setupStep === "link"}
              Connect {selectedResource?.name}
            {:else}
              Create {selectedKind === "application"
                ? "an Application"
                : "a Docker Compose resource"}
            {/if}
          </h2>
          <p class="mt-1 text-sm text-muted-foreground">
            {#if setupStep === "environments"}
              Environments group deployable resources and shared configuration
              in Dokploy.
            {:else if setupStep === "resources"}
              Select an existing resource or create one inside {selectedEnvironment?.name}.
            {:else if setupStep === "kind"}
              Gitadel creates the resource and source link. Finish its build
              configuration in Dokploy.
            {:else if setupStep === "link"}
              Confirm the source path Gitadel should attach when this resource
              has no source yet.
            {:else}
              The new resource stays disabled until you finish setup in Dokploy.
            {/if}
          </p>
        </div>
        {#if setupStep === "environments" && link}
          <Button
            variant="outline"
            size="sm"
            onclick={() => (showSetup = false)}>Cancel</Button
          >
        {/if}
      </div>

      {#if catalogLoading && !catalog}
        <div
          class="flex min-h-48 items-center justify-center rounded-xl border bg-card"
        >
          <Spinner class="size-5 animate-spin text-muted-foreground" />
        </div>
      {:else if catalog && setupStep === "environments"}
        {#if catalog.projects.length === 0}
          <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            <IntegrationAddCard
              compact
              title="Create a project and environment"
              description="Create the first Dokploy project without leaving Gitadel."
              onclick={beginEnvironment}
            />
          </div>
        {:else}
          <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            <IntegrationAddCard
              compact
              title="New environment"
              description="Create an environment in one of your Dokploy projects."
              onclick={beginEnvironment}
            />
            {#each catalog.projects as project (project.id)}
              {#each project.environments as environment (environment.id)}
                <IntegrationConnectionCard
                  compact
                  name={project.name}
                  provider="dokploy"
                  providerName={environment.name}
                  subtitle="Dokploy environment"
                  description={environment.description ??
                    "Groups resources and shared deployment configuration."}
                  selected={selectedProjectId === project.id &&
                    selectedEnvironmentId === environment.id}
                  enabled
                  statusLabel={`${environment.resources.length} ${environment.resources.length === 1 ? "resource" : "resources"}`}
                  detailLabel="Choose"
                  onselect={() => chooseEnvironment(project, environment.id)}
                />
              {/each}
            {/each}
          </div>
        {/if}
      {:else if catalog && setupStep === "resources" && selectedEnvironment}
        <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          <IntegrationAddCard
            compact
            title="New resource"
            description={`Create an Application or Docker Compose resource in ${selectedEnvironment.name}.`}
            onclick={beginResourceKind}
          />
          {#each selectedEnvironment.resources as resource (resource.id)}
            <IntegrationConnectionCard
              compact
              name={resource.name}
              provider="dokploy"
              providerName={resource.kind === "application"
                ? "Application"
                : "Docker Compose"}
              subtitle={resource.server_name ?? "Dokploy server"}
              description="Connect this resource to the current repository."
              enabled={resource.status === "running" ||
                resource.status === "done"}
              selected={selectedResource?.id === resource.id}
              statusLabel={statusLabel(resource.status)}
              detailLabel="Choose"
              onselect={() => beginExisting(resource)}
            />
          {/each}
        </div>
      {:else if setupStep === "kind"}
        <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          <IntegrationConnectionCard
            compact
            name="Application"
            provider="dokploy"
            providerName="Single service"
            subtitle="Application resource"
            description="Use Dockerfile, Railpack, Nixpacks, Paketo, Heroku Buildpacks, or Static configuration in Dokploy."
            enabled
            statusLabel="New"
            detailLabel="Choose"
            onselect={() => beginCreate("application")}
          >
            {#snippet icon()}
              <AppWindow class="size-6 text-primary" />
            {/snippet}
          </IntegrationConnectionCard>
          <IntegrationConnectionCard
            compact
            name="Docker Compose"
            provider="dokploy"
            providerName="Multi-container service"
            subtitle="Compose resource"
            description="Use a Compose file from this repository; configure containers and runtime settings in Dokploy."
            enabled
            statusLabel="New"
            detailLabel="Choose"
            onselect={() => beginCreate("compose")}
          >
            {#snippet icon()}
              <SiDocker class="size-7 text-primary" />
            {/snippet}
          </IntegrationConnectionCard>
        </div>
      {:else if setupStep === "link" && selectedResource}
        <form
          class="max-w-2xl space-y-5 rounded-xl border bg-card p-5 shadow-sm"
          onsubmit={(event) => {
            event.preventDefault();
            void linkExisting();
          }}
        >
          <div class="flex items-start gap-4">
            <span
              class="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
            >
              <Box class="size-5" />
            </span>
            <div>
              <p class="font-semibold">{selectedResource.name}</p>
              <p class="text-sm text-muted-foreground">
                {selectedResource.kind === "application"
                  ? "Application"
                  : "Docker Compose"} in {selectedEnvironment?.name}
              </p>
            </div>
          </div>
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="grid gap-2 text-sm font-medium">
              Branch
              <Input bind:value={branch} required />
            </label>
            {#if selectedResource.kind === "application"}
              <label class="grid gap-2 text-sm font-medium">
                Repository path
                <Input bind:value={repositoryPath} required />
              </label>
            {:else}
              <label class="grid gap-2 text-sm font-medium">
                Compose file
                <Input bind:value={composePath} required />
              </label>
            {/if}
          </div>
          <p class="text-xs leading-5 text-muted-foreground">
            If this resource already uses this repository, Gitadel preserves its
            current source settings. A resource using another source will be
            rejected.
          </p>
          <div class="flex justify-end">
          <Button type="submit" class="gap-2" disabled={working}>
            {#if working}<Spinner class="size-4 animate-spin" />{:else}<Unlink data-icon="inline-start" class="rotate-45" />{/if}
            Connect resource
          </Button>
          </div>
        </form>
      {:else if catalog && setupStep === "create" && selectedEnvironment}
        <form
          class="max-w-2xl space-y-5 rounded-xl border bg-card p-5 shadow-sm"
          onsubmit={(event) => {
            event.preventDefault();
            void createResource();
          }}
        >
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="grid gap-2 text-sm font-medium">
              Name
              <Input bind:value={resourceName} required />
            </label>
            <label class="grid gap-2 text-sm font-medium">
              Branch
              <Input bind:value={branch} required />
            </label>
            {#if selectedKind === "compose"}
              <label class="grid gap-2 text-sm font-medium">
                Compose file
                <Input bind:value={composePath} required />
              </label>
            {/if}
            {#if catalog.servers.length > 0}
              <Field.Field>
                <Field.Label for="dokploy-deployment-server">
                  Deployment server
                </Field.Label>
                <Select.Root type="single" bind:value={serverId}>
                  <Select.Trigger id="dokploy-deployment-server" class="w-full">
                    {serverId === DOKPLOY_SERVER
                      ? "Dokploy server"
                      : (catalog.servers.find(
                          (server) => server.id === serverId,
                        )?.name ?? "Select a server")}
                  </Select.Trigger>
                  <Select.Content>
                    <Select.Group>
                      <Select.Item value={DOKPLOY_SERVER}
                        >Dokploy server</Select.Item
                      >
                      {#each catalog.servers as server (server.id)}
                        <Select.Item value={server.id}
                          >{server.name}</Select.Item
                        >
                      {/each}
                    </Select.Group>
                  </Select.Content>
                </Select.Root>
              </Field.Field>
            {/if}
          </div>
          <div
            class="rounded-lg border bg-muted/30 p-3 text-xs leading-5 text-muted-foreground"
          >
            Gitadel creates the resource and connects its Gitea source. Build
            type, variables, domains, ports, volumes, and deployment settings
            remain in Dokploy.
          </div>
          <div class="flex justify-end">
          <Button type="submit" class="gap-2" disabled={working}>
            {#if working}<Spinner class="size-4 animate-spin" />{:else}<Rocket data-icon="inline-start" />{/if}
            Create resource
          </Button>
          </div>
        </form>
      {/if}
    </section>
  {/if}


{#if operationError}
  <Alert.Root variant="destructive">
    <CircleAlert class="mt-0.5 size-4 shrink-0" />
    <Alert.Title>Deployment operation failed</Alert.Title>
    <Alert.Description>{operationError}</Alert.Description>
  </Alert.Root>
{/if}
</div>
<Dialog.Root bind:open={environmentDialogOpen}>
  <Dialog.Content>
    <form
      onsubmit={(event) => {
        event.preventDefault();
        void createEnvironment();
      }}
    >
      <Dialog.Header>
        <Dialog.Title>Create environment</Dialog.Title>
        <Dialog.Description>
          Name the environment, then choose an existing parent project or create
          a new one.
        </Dialog.Description>
      </Dialog.Header>
      <Field.Group class="py-5">
        <Field.Field>
          <Field.Label for="dokploy-environment-name">
            New environment name
          </Field.Label>
          <Input
            id="dokploy-environment-name"
            bind:value={environmentName}
            placeholder="staging"
            required
          />
          <Field.Description>
            This creates a new environment inside the selected project.
          </Field.Description>
        </Field.Field>
        <Field.Field>
          <Field.Label for="dokploy-environment-project">
            Parent project
          </Field.Label>
          <Select.Root type="single" bind:value={environmentProjectId}>
            <Select.Trigger id="dokploy-environment-project" class="w-full">
              {environmentProjectId === NEW_DOKPLOY_PROJECT
                ? "Create new project"
                : (catalog?.projects.find(
                    (project) => project.id === environmentProjectId,
                  )?.name ?? "Select a project")}
            </Select.Trigger>
            <Select.Content>
              <Select.Group>
                <Select.Item value={NEW_DOKPLOY_PROJECT}>
                  Create new project
                </Select.Item>
                {#if (catalog?.projects.length ?? 0) > 0}
                  <Select.Separator />
                  {#each catalog?.projects ?? [] as project (project.id)}
                    <Select.Item value={project.id}>{project.name}</Select.Item>
                  {/each}
                {/if}
              </Select.Group>
            </Select.Content>
          </Select.Root>
          <Field.Description>
            {environmentProjectId === NEW_DOKPLOY_PROJECT
              ? "Dokploy automatically adds a production environment to every new project."
              : "Dokploy requires every environment to belong to a project."}
          </Field.Description>
        </Field.Field>
        {#if environmentProjectId === NEW_DOKPLOY_PROJECT}
          <Field.Field>
            <Field.Label for="dokploy-project-name">
              New project name
            </Field.Label>
            <Input
              id="dokploy-project-name"
              bind:value={environmentProjectName}
              placeholder="Gitadel"
              required
            />
          </Field.Field>
        {/if}
        <Field.Field>
          <Field.Label for="dokploy-environment-description">
            Description <span class="font-normal text-muted-foreground"
              >Optional</span
            >
          </Field.Label>
          <Input
            id="dokploy-environment-description"
            bind:value={environmentDescription}
            placeholder="Preview deployments"
          />
        </Field.Field>
      </Field.Group>
      <Dialog.Footer>
        <Button
          type="button"
          variant="outline"
          onclick={() => (environmentDialogOpen = false)}
        >
          Cancel
        </Button>
        <Button
          type="submit"
          disabled={working ||
            !environmentProjectId ||
            !environmentName.trim() ||
            (environmentProjectId === NEW_DOKPLOY_PROJECT &&
              !environmentProjectName.trim())}
        >
          {#if working}<Spinner class="size-4 animate-spin" />{/if}
          Create environment
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={unlinkDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Unlink {link?.name}?</AlertDialog.Title>
      <AlertDialog.Description>
        Gitadel will stop sending push events and disable auto-deploy on this
        resource. The resource and its configuration remain in Dokploy.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={working}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        disabled={working}
        variant="destructive"
        onclick={(event) => {
          event.preventDefault();
          void unlinkResource();
        }}
      >
        {#if working}<Spinner class="size-4 animate-spin" />{/if}
        Unlink resource
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
