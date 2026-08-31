<script lang="ts">
  import { goto } from "$app/navigation";
  import { toast } from "svelte-sonner";
  import { resolve } from "$app/paths";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import Box from "@lucide/svelte/icons/box";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import PlugZap from "@lucide/svelte/icons/plug-zap";

  import {
    ApiFailure,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import {
    integrationCredentialSchema,
    integrationSourceConnectionSchema,
    integrationTestResultSchema,
    dokployResourceLinkSchema,
    namespaceIntegrationSchema,
    repositoryIntegrationSchema,
    repositoryIntegrationsSchema,
    type IntegrationProvider,
    type IntegrationSourceConnection,
    type NamespaceIntegration,
    type RepositoryIntegration,
    type RepositoryIntegrationConnection,
  } from "$lib/api/integrations.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import IntegrationConnectionEditor, {
    type ConnectionEditorValue,
  } from "$lib/components/integrations/integration-connection-editor.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as Breadcrumb from "$lib/components/ui/breadcrumb/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Separator } from "$lib/components/ui/separator/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  const namespace = $derived(repository.repository?.namespace ?? null);
  const name = $derived(repository.repository?.name ?? null);
  const identity = $derived(namespace && name ? `${namespace}/${name}` : null);

  let integrations = $state<RepositoryIntegration[]>([]);
  let connections = $state<RepositoryIntegrationConnection[]>([]);
  let providers = $state<IntegrationProvider[]>([]);
  let loading = $state(false);
  let loadError = $state<string | null>(null);
  let integrationIdentity = $state<string | null>(null);
  let working = $state<Record<string, boolean>>({});
  let createOpen = $state(false);
  let formConnection = $state("");
  let formName = $state("");
  let creating = $state(false);
  let createStep = $state<"connections" | "connection" | "source" | "instance">(
    "connections",
  );
  let editorRevision = $state(0);
  let stagedConnection = $state<NamespaceIntegration | null>(null);
  let sourceConnection = $state<IntegrationSourceConnection | null>(null);
  let sourceLoading = $state(false);
  let sourceWorking = $state(false);
  let sourceError = $state<string | null>(null);
  let removeOpen = $state(false);
  let pendingRemove = $state<RepositoryIntegration | null>(null);
  const availableConnections = $derived(
    connections.filter((connection) => connection.configured),
  );
  const selectedConnection = $derived(
    connections.find((connection) => connection.id === formConnection) ?? null,
  );

  function integrationsPath(repoNamespace: string, repoName: string) {
    return `/api/v1/repos/${encodeURIComponent(repoNamespace)}/${encodeURIComponent(repoName)}/integrations`;
  }

  function namespaceIntegrationPath(id: string) {
    if (!namespace) throw new Error("The repository namespace is unavailable.");
    return `/api/v1/namespaces/${encodeURIComponent(namespace)}/integrations/${encodeURIComponent(id)}`;
  }

  function stagedSourcePath() {
    if (!stagedConnection) throw new Error("No connection is selected.");
    return `${namespaceIntegrationPath(stagedConnection.id)}/source`;
  }

  function errorMessage(caught: unknown, fallback: string) {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  $effect(() => {
    if (!namespace || !name || !identity) {
      integrations = [];
      connections = [];
      providers = [];
      loading = false;
      loadError = null;
      integrationIdentity = null;
      return;
    }

    const path = integrationsPath(namespace, name);
    const current = identity;
    let cancelled = false;
    integrationIdentity = current;
    integrations = [];
    connections = [];
    providers = [];
    loading = true;
    loadError = null;
    working = {};

    async function load() {
      try {
        const response = await requestJson(path, repositoryIntegrationsSchema);
        if (cancelled || integrationIdentity !== current) return;
        integrations = response.integrations;
        connections = response.connections;
        providers = response.providers;
      } catch (caught) {
        if (cancelled || integrationIdentity !== current) return;
        loadError = errorMessage(caught, "Could not load integrations.");
      } finally {
        if (!cancelled && integrationIdentity === current) loading = false;
      }
    }

    void load();
    return () => {
      cancelled = true;
    };
  });

  function openCreate() {
    formConnection = "";
    formName = "";
    stagedConnection = null;
    sourceConnection = null;
    sourceError = null;
    createStep = "connections";
    createOpen = true;
  }

  function selectConnection(connection: RepositoryIntegrationConnection) {
    formConnection = connection.id;
    formName = connection.name;
    createStep = "instance";
  }

  function openConnectionEditor() {
    editorRevision += 1;
    createStep = "connection";
  }

  async function testNewConnection(value: ConnectionEditorValue) {
    if (!namespace) throw new Error("The repository namespace is unavailable.");
    return requestJson(
      `/api/v1/namespaces/${encodeURIComponent(namespace)}/integrations/test`,
      integrationTestResultSchema,
      {
        method: "POST",
        body: jsonBody({
          provider: value.provider,
          url: value.url,
          api_key: value.apiKey,
        }),
      },
    );
  }

  async function createConnection(value: ConnectionEditorValue) {
    if (!namespace) throw new Error("The repository namespace is unavailable.");
    const created = await requestJson(
      `/api/v1/namespaces/${encodeURIComponent(namespace)}/integrations`,
      namespaceIntegrationSchema,
      {
        method: "POST",
        body: jsonBody({
          provider: value.provider,
          name: value.name,
          url: value.url,
          internal_url: value.internalUrl,
          api_key: value.apiKey,
        }),
      },
    );
    const sourceRequired =
      providers.find((provider) => provider.slug === created.provider)
        ?.source_required ?? false;
    const connection: RepositoryIntegrationConnection = {
      id: created.id,
      provider: created.provider,
      provider_name: created.provider_name,
      name: created.name,
      configured:
        created.enabled && (!sourceRequired || created.source?.ready === true),
    };
    connections = [...connections, connection];
    if (!sourceRequired) {
      selectConnection(connection);
      return;
    }
    stagedConnection = created;
    sourceConnection = null;
    sourceError = null;
    editorRevision += 1;
    createStep = "source";
    await loadStagedSource();
  }

  function updateStagedSource(response: IntegrationSourceConnection | null) {
    sourceConnection = response;
    if (!stagedConnection) return;
    const binding = response?.binding;
    stagedConnection = {
      ...stagedConnection,
      source: binding
        ? {
            id: binding.id,
            name: binding.name,
            managed: binding.managed,
            ready: binding.ready,
          }
        : undefined,
    };
    connections = connections.map((connection) =>
      connection.id === stagedConnection?.id
        ? {
            ...connection,
            name: stagedConnection.name,
            configured: stagedConnection.enabled && binding?.ready === true,
          }
        : connection,
    );
    if (binding?.ready) {
      const connection = connections.find(
        (candidate) => candidate.id === stagedConnection?.id,
      );
      if (connection) selectConnection(connection);
    }
  }

  async function loadStagedSource() {
    if (!stagedConnection) return;
    sourceLoading = true;
    sourceError = null;
    try {
      updateStagedSource(
        await requestJson(
          stagedSourcePath(),
          integrationSourceConnectionSchema,
        ),
      );
    } catch (caught) {
      sourceError = errorMessage(
        caught,
        "Could not load the repository source.",
      );
    } finally {
      sourceLoading = false;
    }
  }

  async function bindStagedSource(sourceId: string) {
    sourceWorking = true;
    sourceError = null;
    try {
      updateStagedSource(
        await requestJson(
          stagedSourcePath(),
          integrationSourceConnectionSchema,
          {
            method: "POST",
            body: jsonBody({ action: "bind", source_id: sourceId }),
          },
        ),
      );
    } catch (caught) {
      toast.error(
        errorMessage(caught, "Could not connect the Dokploy source."),
      );
    } finally {
      sourceWorking = false;
    }
  }

  async function createStagedSource(sourceName: string) {
    sourceWorking = true;
    sourceError = null;
    try {
      updateStagedSource(
        await requestJson(
          stagedSourcePath(),
          integrationSourceConnectionSchema,
          {
            method: "POST",
            body: jsonBody({ action: "create", name: sourceName }),
          },
        ),
      );
    } catch (caught) {
      toast.error(
        errorMessage(caught, "Could not create the Dokploy source."),
      );
    } finally {
      sourceWorking = false;
    }
  }

  async function disconnectStagedSource() {
    sourceWorking = true;
    sourceError = null;
    try {
      await requestEmpty(stagedSourcePath(), { method: "DELETE" });
      updateStagedSource(null);
      await loadStagedSource();
    } catch (caught) {
      toast.error(
        errorMessage(caught, "Could not disconnect the Dokploy source."),
      );
    } finally {
      sourceWorking = false;
    }
  }

  async function saveStagedConnection(value: ConnectionEditorValue) {
    if (!stagedConnection) throw new Error("No connection is selected.");
    const updated = await requestJson(
      namespaceIntegrationPath(stagedConnection.id),
      namespaceIntegrationSchema,
      {
        method: "PUT",
        body: jsonBody({
          name: value.name,
          url: value.url,
          internal_url: value.internalUrl,
          api_key: value.apiKey || null,
        }),
      },
    );
    stagedConnection = updated;
    connections = connections.map((connection) =>
      connection.id === updated.id
        ? { ...connection, name: updated.name }
        : connection,
    );
  }

  async function revealStagedCredential() {
    if (!stagedConnection) throw new Error("No connection is selected.");
    const credential = await requestJson(
      `${namespaceIntegrationPath(stagedConnection.id)}/credential`,
      integrationCredentialSchema,
    );
    return credential.api_key;
  }

  async function testStagedConnection() {
    if (!stagedConnection) throw new Error("No connection is selected.");
    return requestJson(
      `${namespaceIntegrationPath(stagedConnection.id)}/test`,
      integrationTestResultSchema,
      { method: "POST" },
    );
  }

  async function createIntegration() {
    if (!namespace || !name || !formConnection) return;
    creating = true;
    try {
      const created = await requestJson(
        integrationsPath(namespace, name),
        repositoryIntegrationSchema,
        {
          method: "POST",
          body: jsonBody({
            integration_id: formConnection,
            name: formName,
          }),
        },
      );
      integrations = [...integrations, created];
      createOpen = false;
    } catch (caught) {
      toast.error(errorMessage(caught, "Could not add the integration."));
    } finally {
      creating = false;
    }
  }

  async function updateIntegration(
    integration: RepositoryIntegration,
    enabled: boolean,
  ) {
    if (!namespace || !name) return;
    working[integration.id] = true;
    try {
      const updated = await requestJson(
        `${integrationsPath(namespace, name)}/${encodeURIComponent(integration.id)}`,
        repositoryIntegrationSchema,
        { method: "PUT", body: jsonBody({ enabled }) },
      );
      const index = integrations.findIndex(
        (candidate) => candidate.id === integration.id,
      );
      if (index !== -1) integrations[index] = updated;
    } catch (caught) {
      toast.error(
        errorMessage(caught, `Could not update ${integration.name}.`),
      );
    } finally {
      working[integration.id] = false;
    }
  }

  function requestRemove(integration: RepositoryIntegration) {
    pendingRemove = integration;
    removeOpen = true;
  }

  async function removeIntegration() {
    if (!namespace || !name || !pendingRemove) return;
    const integration = pendingRemove;
    working[integration.id] = true;
    try {
      await requestEmpty(
        `${integrationsPath(namespace, name)}/${encodeURIComponent(integration.id)}`,
        { method: "DELETE" },
      );
      integrations = integrations.filter(
        (candidate) => candidate.id !== integration.id,
      );
      removeOpen = false;
      pendingRemove = null;
    } catch (caught) {
      toast.error(
        errorMessage(caught, `Could not remove ${integration.name}.`),
      );
      removeOpen = false;
    } finally {
      working[integration.id] = false;
    }
  }

  function linkedDokployResource(integration: RepositoryIntegration) {
    if (integration.provider !== "dokploy" || !integration.resource)
      return null;
    const result = dokployResourceLinkSchema.safeParse(integration.resource);
    return result.success ? result.data : null;
  }

  function connectionConfigured(integration: RepositoryIntegration) {
    return (
      connections.find(
        (connection) => connection.id === integration.connection_id,
      )?.configured ?? false
    );
  }

  function integrationStatus(
    integration: RepositoryIntegration,
    resource: ReturnType<typeof linkedDokployResource>,
  ) {
    if (!connectionConfigured(integration)) return "Connection setup required";
    if (!integration.enabled) return "Disabled";
    if (resource) return "Push delivery on";
    if (integration.repository_setup) return "Repository setup required";
    return "Enabled";
  }

  function describe(integration: RepositoryIntegration) {
    if (!connectionConfigured(integration)) {
      return `${integration.connection_name} needs a repository source before deployments can run.`;
    }
    if (!integration.configured) {
      return `${integration.connection_name} is currently disabled.`;
    }
    const resource = linkedDokployResource(integration);
    if (resource) {
      return `Source ${integration.enabled ? "enabled" : "disabled"} · ${resource.project_name} / ${resource.environment_name}`;
    }
    if (!integration.enabled) return "Disabled for this repository.";
    if (!integration.repository_setup) return "Enabled for this repository.";
    return "Setup required before push deployments can be enabled.";
  }
</script>

<section class="space-y-6" aria-labelledby="repository-integrations-heading">
  <header>
    <h2
      id="repository-integrations-heading"
      class="text-lg font-semibold tracking-tight"
    >
      Integrations
    </h2>
    <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
      Add independently configured service instances to this repository. The
      same connection can be added more than once for separate targets.
    </p>
  </header>

  {#if loading}
    <p class="flex items-center gap-2 text-sm text-muted-foreground">
      <Spinner class="size-4 animate-spin" />Loading integrations…
    </p>
  {:else if loadError}
    <Alert.Root variant="destructive">
      <CircleAlert class="size-4 shrink-0" />
      <Alert.Title>Integrations unavailable</Alert.Title>
      <Alert.Description>{loadError}</Alert.Description>
    </Alert.Root>
  {:else}
    <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
      <IntegrationAddCard
        description="Add another service instance or reuse an existing connection."
        onclick={openCreate}
      />
      {#each integrations as integration (integration.id)}
        {@const resource = linkedDokployResource(integration)}
        {@const connectionReady = connectionConfigured(integration)}
        <IntegrationConnectionCard
          name={resource?.name ?? integration.name}
          provider={integration.provider}
          providerName={integration.provider_name}
          subtitle={resource
            ? `${resource.project_name} / ${resource.environment_name}`
            : integration.connection_name}
          description={describe(integration)}
          statusLabel={integrationStatus(integration, resource)}
          statusHealthy={connectionReady && integration.enabled}
          enabled={integration.enabled}
          detailLabel={!connectionReady
            ? "Set up source"
            : resource
              ? resource.kind === "application"
                ? "Application"
                : "Docker Compose"
              : "Setup required"}
          busy={working[integration.id]}
          onenabledchange={integration.configured && connectionReady
            ? (enabled) => void updateIntegration(integration, enabled)
            : null}
          onconfigure={!connectionReady
            ? () =>
                void goto(
                  resolve("/[namespace]/integrations", {
                    namespace: repository.namespace,
                  }),
                )
            : integration.configured && integration.repository_setup
              ? () =>
                  repository.navigate("integrations", {
                    provider: integration.id,
                  })
              : null}
          configureLabel={!connectionReady ? "Set up connection" : "Configure"}
          onremove={() => requestRemove(integration)}
        />
      {/each}
    </div>
  {/if}
</section>

<Dialog.Root bind:open={createOpen}>
  <Dialog.Content
    id="repository-integration-setup"
    class="ring-foreground/20 sm:max-w-2xl"
  >
    <Dialog.Header>
      <Dialog.Title>
        {createStep === "connections"
          ? "Choose a connection"
          : createStep === "connection"
            ? "Create connection"
            : createStep === "source"
              ? "Connect repository source"
              : "Name repository integration"}
      </Dialog.Title>
      <Dialog.Description>
        {createStep === "connections"
          ? "Select a configured connection, or create one without leaving this repository."
          : createStep === "connection"
            ? `Create a connection owned by ${namespace ?? "this namespace"}.`
            : createStep === "source"
              ? "Choose or create the exact Dokploy Gitea provider that will clone this repository."
              : `Create an independent repository instance using ${selectedConnection?.name ?? "this connection"}.`}
      </Dialog.Description>
    </Dialog.Header>
    <Breadcrumb.Root>
      <Breadcrumb.List
        class="grid w-full grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto_minmax(0,1fr)] gap-0 overflow-hidden rounded-lg border bg-card"
      >
        <Breadcrumb.Item class="relative min-w-0 self-stretch">
          <PlugZap
            class="pointer-events-none absolute left-3 top-1/2 z-10 size-4 -translate-y-1/2 {createStep ===
              'connections' || createStep === 'connection'
              ? 'text-primary'
              : 'text-muted-foreground'}"
            aria-hidden="true"
          />
          {#if createStep === "connections" || createStep === "connection"}
            <Breadcrumb.Page
              class="grid h-full w-full min-w-0 gap-0.5 bg-primary/10 py-3 pr-3 pl-10 text-primary ring-1 ring-inset ring-primary/30"
            >
              <span
                class="truncate text-xs font-medium uppercase tracking-wide text-primary/80"
                >Connection</span
              >
              <span class="truncate">
                {createStep === "connection"
                  ? "Create connection"
                  : "Choose connection"}
              </span>
            </Breadcrumb.Page>
          {:else}
            <span class="grid h-full min-w-0 gap-0.5 py-3 pr-3 pl-10">
              <span class="truncate text-xs font-medium uppercase tracking-wide"
                >Connection</span
              >
              <span class="truncate text-foreground">
                {stagedConnection?.name ??
                  selectedConnection?.name ??
                  "Connection ready"}
              </span>
            </span>
          {/if}
        </Breadcrumb.Item>
        <Breadcrumb.Separator class="self-stretch">
          <Separator orientation="vertical" />
        </Breadcrumb.Separator>
        <Breadcrumb.Item class="relative min-w-0 self-stretch">
          <GitBranch
            class="pointer-events-none absolute left-3 top-1/2 z-10 size-4 -translate-y-1/2 {createStep ===
            'source'
              ? 'text-primary'
              : 'text-muted-foreground'}"
            aria-hidden="true"
          />
          {#if createStep === "source"}
            <Breadcrumb.Page
              class="grid h-full w-full min-w-0 gap-0.5 bg-primary/10 py-3 pr-3 pl-10 text-primary ring-1 ring-inset ring-primary/30"
            >
              <span
                class="truncate text-xs font-medium uppercase tracking-wide text-primary/80"
                >Source</span
              >
              <span class="truncate">Connect source</span>
            </Breadcrumb.Page>
          {:else}
            <span class="grid h-full min-w-0 gap-0.5 py-3 pr-3 pl-10">
              <span class="truncate text-xs font-medium uppercase tracking-wide"
                >Source</span
              >
              <span
                class="truncate {createStep === 'instance'
                  ? 'text-foreground'
                  : ''}"
              >
                {createStep === "instance"
                  ? "Source ready"
                  : "Next if required"}
              </span>
            </span>
          {/if}
        </Breadcrumb.Item>
        <Breadcrumb.Separator class="self-stretch">
          <Separator orientation="vertical" />
        </Breadcrumb.Separator>
        <Breadcrumb.Item class="relative min-w-0 self-stretch">
          <Box
            class="pointer-events-none absolute left-3 top-1/2 z-10 size-4 -translate-y-1/2 {createStep ===
            'instance'
              ? 'text-primary'
              : 'text-muted-foreground'}"
            aria-hidden="true"
          />
          {#if createStep === "instance"}
            <Breadcrumb.Page
              class="grid h-full w-full min-w-0 gap-0.5 bg-primary/10 py-3 pr-3 pl-10 text-primary ring-1 ring-inset ring-primary/30"
            >
              <span
                class="truncate text-xs font-medium uppercase tracking-wide text-primary/80"
                >Instance</span
              >
              <span class="truncate">Name target</span>
            </Breadcrumb.Page>
          {:else}
            <span class="grid h-full min-w-0 gap-0.5 py-3 pr-3 pl-10">
              <span class="truncate text-xs font-medium uppercase tracking-wide"
                >Instance</span
              >
              <span class="truncate">Final step</span>
            </span>
          {/if}
        </Breadcrumb.Item>
      </Breadcrumb.List>
    </Breadcrumb.Root>

    {#if createStep === "connections"}
      <div class="grid max-h-[60vh] gap-3 overflow-y-auto pr-1 sm:grid-cols-2">
        <IntegrationAddCard
          title="New connection"
          description="Connect another service and configure its credentials now."
          compact
          onclick={openConnectionEditor}
        />
        {#each availableConnections as connection (connection.id)}
          <IntegrationConnectionCard
            name={connection.name}
            provider={connection.provider}
            providerName={connection.provider_name}
            subtitle={namespace ?? "Repository namespace"}
            description="Use this configured connection for a new repository integration instance."
            enabled={connection.configured}
            statusLabel="Connected"
            compact
            onselect={() => selectConnection(connection)}
          />
        {/each}
      </div>
      <Dialog.Footer>
        <Button
          type="button"
          variant="ghost"
          onclick={() => (createOpen = false)}
        >
          Cancel
        </Button>
      </Dialog.Footer>
    {:else if createStep === "connection"}
      {#key editorRevision}
        <IntegrationConnectionEditor
          {providers}
          targets={namespace ? [{ slug: namespace, label: namespace }] : []}
          initialTarget={namespace ?? ""}
          requireSuccessfulTest
          onsave={createConnection}
          oncancel={() => (createStep = "connections")}
          ontest={testNewConnection}
        />
      {/key}
    {:else if createStep === "source" && stagedConnection}
      {#key editorRevision}
        <IntegrationConnectionEditor
          {providers}
          targets={namespace ? [{ slug: namespace, label: namespace }] : []}
          initialTarget={namespace ?? ""}
          integration={stagedConnection}
          onsave={saveStagedConnection}
          oncancel={() => (createOpen = false)}
          onreveal={revealStagedCredential}
          ontest={testStagedConnection}
          {sourceConnection}
          {sourceLoading}
          {sourceWorking}
          {sourceError}
          onrefreshsource={loadStagedSource}
          onbindsource={bindStagedSource}
          oncreatesource={createStagedSource}
          ondisconnectsource={sourceConnection?.binding
            ? disconnectStagedSource
            : null}
        />
      {/key}
    {:else}
      <form
        class="grid gap-5"
        onsubmit={(event) => {
          event.preventDefault();
          void createIntegration();
        }}
      >
        <label class="grid gap-1.5 text-sm font-medium">
          Instance name
          <Input
            bind:value={formName}
            maxlength={80}
            placeholder="Production deployment"
            required
          />
          <span class="text-xs font-normal text-muted-foreground">
            Use a name that distinguishes this target from other instances.
          </span>
        </label>
        <Dialog.Footer class="gap-2 sm:justify-between">
          <Button
            type="button"
            variant="ghost"
            onclick={() => (createStep = "connections")}
          >
            <ArrowLeft data-icon="inline-start" />Back
          </Button>
          <Button type="submit" disabled={creating || !formConnection}>
            {#if creating}<Spinner class="size-4 animate-spin" />{/if}
            {creating ? "Adding…" : "Add integration"}
          </Button>
        </Dialog.Footer>
      </form>
    {/if}
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={removeOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>
        Remove {pendingRemove?.name ?? "integration"}?
      </AlertDialog.Title>
      <AlertDialog.Description>
        This removes its repository-specific target and configuration. The
        namespace connection remains available.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => void removeIntegration()}
      >
        Remove integration
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
