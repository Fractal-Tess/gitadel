<script lang="ts">
  import { resolve } from "$app/paths";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import PlugZap from "@lucide/svelte/icons/plug-zap";
  import Rocket from "@lucide/svelte/icons/rocket";

  import {
    buildChoices,
    connectionConfigured,
    createRepositoryIntegration,
    loadRepositoryDeploymentSuggestions,
    loadRepositoryIntegrations,
    type BuildChoice,
    type RepositoryIntegration,
    type RepositoryIntegrationConnection,
  } from "$lib/api/integrations.js";
  import { errorMessage } from "$lib/api/transport.js";
  import RepositoryDeployIntegration from "$lib/components/repository/repository-deploy-integration.svelte";
  import RepositoryIntegrationConfigure from "$lib/components/repository/repository-integration-configure.svelte";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  let integrations = $state.raw<RepositoryIntegration[]>([]);
  let connections = $state.raw<RepositoryIntegrationConnection[]>([]);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  /** Builds Gitadel detected, shown before anything is set up. */
  let detected = $state.raw<BuildChoice[]>([]);
  let chosenConnection = $state("");
  let creating = $state(false);
  let createError = $state<string | null>(null);

  const deployments = $derived(
    integrations.filter((integration) => integration.repository_setup),
  );
  const readyConnections = $derived(
    connections.filter((connection) => connection.configured),
  );
  const unfinishedConnections = $derived(
    connections.filter((connection) => !connection.configured),
  );
  const selectedConnection = $derived(
    readyConnections.find((connection) => connection.id === chosenConnection) ??
      readyConnections[0] ??
      null,
  );
  const personal = $derived(
    repository.namespace === repository.authStatus?.user?.username,
  );
  const connectionsHref = $derived(
    resolve("/[namespace]/integrations", { namespace: repository.namespace }),
  );

  $effect(() => {
    if (repository.integrationProvider) return;
    let cancelled = false;
    const namespace = repository.namespace;
    const name = repository.name;
    loading = true;
    loadError = null;

    async function load() {
      try {
        const response = await loadRepositoryIntegrations(namespace, name);
        if (cancelled) return;
        integrations = response.integrations;
        connections = response.connections;
        if (
          !response.integrations.some(
            (integration) => integration.repository_setup,
          )
        ) {
          void loadDetected(namespace, name, () => cancelled);
        }
      } catch (caught) {
        if (!cancelled)
          loadError = errorMessage(caught, "Could not load deployments.");
      } finally {
        if (!cancelled) loading = false;
      }
    }

    void load();
    return () => {
      cancelled = true;
    };
  });

  /** Best effort: the setup form reads the repository again anyway. */
  async function loadDetected(
    namespace: string,
    name: string,
    cancelled: () => boolean,
  ) {
    try {
      const suggestions = await loadRepositoryDeploymentSuggestions(
        namespace,
        name,
      );
      if (!cancelled())
        detected = buildChoices(suggestions.suggestions).filter(
          (choice) => choice.suggestion,
        );
    } catch {
      if (!cancelled()) detected = [];
    }
  }

  async function createDeployment() {
    const connection = selectedConnection;
    if (!connection || creating) return;
    creating = true;
    createError = null;
    try {
      const created = await createRepositoryIntegration(
        repository.namespace,
        repository.name,
        {
          integration_id: connection.id,
          name: repository.repository?.name ?? repository.name,
        },
      );
      integrations = [...integrations, created];
    } catch (caught) {
      createError = errorMessage(caught, "Could not add the deployment.");
    } finally {
      creating = false;
    }
  }
</script>

{#if repository.integrationProvider}
  <RepositoryIntegrationConfigure state={repository} />
{:else}
  <div class="space-y-6">
    <header class="min-w-0">
      <h1 class="text-lg font-semibold">Deploy</h1>
      <p class="text-sm text-muted-foreground">
        Deploy {repository.name} with Dokploy and redeploy it on every push.
      </p>
    </header>

    {#if loading}
      <div
        class="flex min-h-48 items-center justify-center rounded-xl border bg-card"
      >
        <Spinner class="size-5 animate-spin text-muted-foreground" />
      </div>
    {:else if loadError}
      <Alert.Root variant="destructive">
        <CircleAlert class="size-4 shrink-0" />
        <Alert.Title>Deployments unavailable</Alert.Title>
        <Alert.Description>{loadError}</Alert.Description>
      </Alert.Root>
    {:else if deployments.length > 0}
      <div class="motion-list grid gap-4">
        {#each deployments as integration (integration.id)}
          <RepositoryDeployIntegration
            state={repository}
            {integration}
            connectionReady={connectionConfigured(connections, integration)}
          />
        {/each}
      </div>
    {:else if selectedConnection}
      <section
        class="max-w-2xl space-y-5 rounded-xl border bg-card p-5 shadow-sm"
        aria-labelledby="deploy-start-heading"
      >
        <div class="flex items-start gap-4">
          <span
            class="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
          >
            <Rocket class="size-5" aria-hidden="true" />
          </span>
          <div class="min-w-0">
            <h2 id="deploy-start-heading" class="font-semibold">
              This repository is not deployed yet
            </h2>
            <p class="mt-1 text-sm leading-6 text-muted-foreground">
              Gitadel creates the resource in Dokploy, points it at this
              repository, and redeploys it on every push.
            </p>
          </div>
        </div>

        {#if detected.length > 0}
          <div>
            <p class="text-sm font-medium">Gitadel found</p>
            <ul class="mt-1 grid gap-0.5 text-sm text-muted-foreground">
              {#each detected as choice (choice.id)}
                <li class="break-words">{choice.label}</li>
              {/each}
            </ul>
          </div>
        {/if}

        {#if readyConnections.length > 1}
          <Field.Field>
            <Field.Label for="deploy-connection">Dokploy connection</Field.Label>
            <Select.Root
              type="single"
              value={selectedConnection.id}
              onValueChange={(value) => (chosenConnection = value)}
              disabled={creating}
            >
              <Select.Trigger id="deploy-connection" class="w-full">
                {selectedConnection.name}
              </Select.Trigger>
              <Select.Content>
                <Select.Group>
                  {#each readyConnections as connection (connection.id)}
                    <Select.Item value={connection.id}>
                      {connection.name}
                    </Select.Item>
                  {/each}
                </Select.Group>
              </Select.Content>
            </Select.Root>
          </Field.Field>
        {/if}

        {#if createError}
          <Alert.Root variant="destructive">
            <CircleAlert class="size-4 shrink-0" />
            <Alert.Title>Could not start setup</Alert.Title>
            <Alert.Description>{createError}</Alert.Description>
          </Alert.Root>
        {/if}

        <Button
          class="gap-2"
          disabled={creating}
          onclick={() => void createDeployment()}
        >
          {#if creating}
            <Spinner class="size-4 animate-spin" />
          {:else}
            <Rocket data-icon="inline-start" />
          {/if}
          Set up deployment with {selectedConnection.name}
        </Button>
      </section>
    {:else}
      <section
        class="max-w-2xl space-y-4 rounded-xl border bg-card p-5 shadow-sm"
        aria-labelledby="deploy-connect-heading"
      >
        <div class="flex items-start gap-4">
          <span
            class="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
          >
            <PlugZap class="size-5" aria-hidden="true" />
          </span>
          <div class="min-w-0">
            <h2 id="deploy-connect-heading" class="font-semibold">
              Connect Dokploy first
            </h2>
            <p class="mt-1 text-sm leading-6 text-muted-foreground">
              {#if personal}
                Add a Dokploy connection once under your account's Integrations.
                Every repository you own can then deploy with it.
              {:else}
                Add a Dokploy connection once under the {repository.namespace}
                Integrations. Every repository in {repository.namespace} can then
                deploy with it.
              {/if}
            </p>
            {#if unfinishedConnections.length > 0}
              <p class="mt-2 text-sm leading-6 text-muted-foreground">
                {unfinishedConnections
                  .map((connection) => connection.name)
                  .join(", ")}
                {unfinishedConnections.length === 1 ? "is" : "are"} not finished
                yet.
              </p>
            {/if}
          </div>
        </div>
        <Button variant="outline" class="gap-2" href={connectionsHref}>
          <PlugZap data-icon="inline-start" />
          {unfinishedConnections.length > 0
            ? "Finish the connection"
            : "Add a Dokploy connection"}
        </Button>
      </section>
    {/if}
  </div>
{/if}
