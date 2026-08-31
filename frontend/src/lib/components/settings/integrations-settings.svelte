<script lang="ts">
  import { onMount } from "svelte";
  import { toast } from "svelte-sonner";
  import { Spinner } from "$lib/components/ui/spinner/index.js";

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
    namespaceIntegrationSchema,
    type IntegrationProvider,
    type IntegrationSourceConnection,
    type NamespaceIntegration,
  } from "$lib/api/integrations.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import IntegrationConnectionEditor, {
    type ConnectionEditorValue,
  } from "$lib/components/integrations/integration-connection-editor.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { takeNamespaceIntegrations } from "$lib/namespace-preload.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  type Target = { slug: string; label: string };
  let {
    namespace,
  }: {
    namespace: Target;
  } = $props();

  const targets = $derived<Target[]>([namespace]);
  const app = useAppState();
  type Provider = IntegrationProvider;
  type Integration = NamespaceIntegration;
  type Card = { target: Target; integration: Integration };

  let integrations = $state<Record<string, Integration[]>>({});
  let providerCatalog = $state<Provider[]>([]);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let working = $state<Record<string, boolean>>({});

  let modalOpen = $state(false);
  let editing = $state<Card | null>(null);
  let editorRevision = $state(0);
  let editorMode = $state<"connection" | "source">("connection");
  let removeDialogOpen = $state(false);
  let pendingRemoveCard = $state<Card | null>(null);
  let sourceConnection = $state<IntegrationSourceConnection | null>(null);
  let sourceLoading = $state(false);
  let sourceWorking = $state(false);
  let sourceError = $state<string | null>(null);

  const cards = $derived<Card[]>(
    targets.flatMap((target) =>
      (integrations[target.slug] ?? []).map((integration) => ({
        target,
        integration,
      })),
    ),
  );
  const editorTitle = $derived(
    editing ? `Configure ${editing.integration.name}` : "Add integration",
  );

  onMount(() => {
    void load();
  });

  function message(caught: unknown, fallback: string) {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  function integrationPath(slug: string, id: string) {
    return `/api/v1/namespaces/${encodeURIComponent(slug)}/integrations/${encodeURIComponent(id)}`;
  }

  function sourcePath(card: Card) {
    return `${integrationPath(card.target.slug, card.integration.id)}/source`;
  }
  async function load() {
    const scope = app.authorizationScope;
    loading = true;
    loadError = null;
    try {
      const responses = await Promise.all(
        targets.map(({ slug }) => takeNamespaceIntegrations(slug, scope)),
      );
      if (app.authorizationScope !== scope) return;
      for (const integrationResponse of responses) {
        integrations[integrationResponse.namespace] =
          integrationResponse.integrations;
        if (providerCatalog.length === 0)
          providerCatalog = integrationResponse.providers;
      }
    } catch (caught) {
      loadError = message(caught, "Could not load the integration catalog.");
    } finally {
      loading = false;
    }
  }

  function openCreate() {
    editing = null;
    sourceConnection = null;
    sourceError = null;
    editorMode = "connection";
    editorRevision += 1;
    modalOpen = true;
  }

  function openConfigure(card: Card) {
    editing = card;
    sourceConnection = null;
    sourceError = null;
    editorRevision += 1;
    const sourceRequired =
      providerCatalog.find(
        (provider) => provider.slug === card.integration.provider,
      )?.source_required ?? false;
    editorMode =
      sourceRequired && card.integration.source?.ready !== true
        ? "source"
        : "connection";
    modalOpen = true;
    if (
      providerCatalog.find(
        (provider) => provider.slug === card.integration.provider,
      )?.source_required
    ) {
      void loadSource(card);
    }
  }

  function updateCard(slug: string, updated: Integration) {
    integrations[slug] = (integrations[slug] ?? []).map((integration) =>
      integration.id === updated.id ? updated : integration,
    );
  }

  function updateSourceSummary(
    card: Card,
    response: IntegrationSourceConnection | null,
  ) {
    const binding = response?.binding;
    const updated: Integration = {
      ...card.integration,
      source: binding
        ? {
            id: binding.id,
            name: binding.name,
            managed: binding.managed,
            ready: binding.ready,
          }
        : undefined,
    };
    updateCard(card.target.slug, updated);
    editing = { target: card.target, integration: updated };
  }

  async function loadSource(card = editing) {
    if (!card) return;
    sourceLoading = true;
    sourceError = null;
    try {
      sourceConnection = await requestJson(
        sourcePath(card),
        integrationSourceConnectionSchema,
      );
      updateSourceSummary(card, sourceConnection);
    } catch (caught) {
      sourceError = message(caught, "Could not load the repository source.");
    } finally {
      sourceLoading = false;
    }
  }

  async function bindSource(sourceId: string) {
    const card = editing;
    if (!card) return;
    sourceWorking = true;
    sourceError = null;
    try {
      sourceConnection = await requestJson(
        sourcePath(card),
        integrationSourceConnectionSchema,
        {
          method: "POST",
          body: jsonBody({ action: "bind", source_id: sourceId }),
        },
      );
      updateSourceSummary(card, sourceConnection);
    } catch (caught) {
      toast.error(message(caught, "Could not connect the Dokploy source."));
    } finally {
      sourceWorking = false;
    }
  }

  async function createSource(name: string) {
    const card = editing;
    if (!card) return;
    sourceWorking = true;
    sourceError = null;
    try {
      sourceConnection = await requestJson(
        sourcePath(card),
        integrationSourceConnectionSchema,
        {
          method: "POST",
          body: jsonBody({ action: "create", name }),
        },
      );
      updateSourceSummary(card, sourceConnection);
    } catch (caught) {
      toast.error(message(caught, "Could not create the Dokploy source."));
    } finally {
      sourceWorking = false;
    }
  }

  async function disconnectSource() {
    const card = editing;
    if (!card) return;
    sourceWorking = true;
    sourceError = null;
    try {
      await requestEmpty(sourcePath(card), { method: "DELETE" });
      updateSourceSummary(card, null);
      await loadSource(editing);
    } catch (caught) {
      toast.error(message(caught, "Could not disconnect the Dokploy source."));
    } finally {
      sourceWorking = false;
    }
  }

  async function saveConnection(value: ConnectionEditorValue) {
    if (editing) {
      const updated = await requestJson(
        integrationPath(value.target, editing.integration.id),
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
      const binding = sourceConnection?.binding;
      updateCard(
        value.target,
        binding
          ? {
              ...updated,
              source: {
                id: binding.id,
                name: binding.name,
                managed: binding.managed,
                ready: binding.ready,
              },
            }
          : updated,
      );
    } else {
      const created = await requestJson(
        `/api/v1/namespaces/${encodeURIComponent(value.target)}/integrations`,
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
      integrations[value.target] = [
        ...(integrations[value.target] ?? []),
        created,
      ];
      const target =
        targets.find((candidate) => candidate.slug === value.target) ??
        ({ slug: value.target, label: value.target } satisfies Target);
      const card = { target, integration: created };
      const sourceRequired =
        providerCatalog.find((provider) => provider.slug === created.provider)
          ?.source_required ?? false;
      if (sourceRequired) {
        editing = card;
        editorMode = "source";
        editorRevision += 1;
        await loadSource(card);
        return;
      }
    }
    modalOpen = false;
  }

  async function testNewConnection(value: ConnectionEditorValue) {
    return requestJson(
      `/api/v1/namespaces/${encodeURIComponent(value.target)}/integrations/test`,
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

  async function revealCredential() {
    if (!editing) throw new Error("No connection is selected.");
    const credential = await requestJson(
      `${integrationPath(editing.target.slug, editing.integration.id)}/credential`,
      integrationCredentialSchema,
    );
    return credential.api_key;
  }

  async function setEnabled(card: Card, enabled: boolean) {
    const id = card.integration.id;
    working[id] = true;
    try {
      const updated = await requestJson(
        integrationPath(card.target.slug, id),
        namespaceIntegrationSchema,
        { method: "PUT", body: jsonBody({ enabled }) },
      );
      updateCard(card.target.slug, updated);
    } catch (caught) {
      toast.error(
        message(caught, `Could not update ${card.integration.name}.`),
      );
    } finally {
      working[id] = false;
    }
  }

  async function testConnection() {
    if (!editing) throw new Error("No connection is selected.");
    return requestJson(
      `${integrationPath(editing.target.slug, editing.integration.id)}/test`,
      integrationTestResultSchema,
      { method: "POST" },
    );
  }

  function requestRemove(card: Card) {
    pendingRemoveCard = card;
    removeDialogOpen = true;
  }

  async function confirmRemove() {
    const card = pendingRemoveCard;
    if (!card) return;
    const id = card.integration.id;
    working[id] = true;
    try {
      await requestEmpty(integrationPath(card.target.slug, id), {
        method: "DELETE",
      });
      integrations[card.target.slug] = (
        integrations[card.target.slug] ?? []
      ).filter((integration) => integration.id !== id);
      removeDialogOpen = false;
      pendingRemoveCard = null;
    } catch (caught) {
      toast.error(
        message(caught, `Could not remove ${card.integration.name}.`),
      );
      removeDialogOpen = false;
    } finally {
      working[id] = false;
    }
  }
</script>

<section class="space-y-6" aria-labelledby="integrations-heading">
  <header>
    <h2 id="integrations-heading" class="text-lg font-semibold tracking-tight">
      Integrations
    </h2>
    <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
      Services connected to {namespace.label}. Repositories under this owner can
      choose which connection to use.
    </p>
  </header>

  {#if loading}
    <p class="flex items-center gap-2 text-sm text-muted-foreground">
      <Spinner class="size-4" />Loading integrations…
    </p>
  {:else if loadError}
    <Alert.Root variant="destructive">
      <Alert.Title>Integrations unavailable</Alert.Title>
      <Alert.Description>{loadError}</Alert.Description>
    </Alert.Root>
  {:else}
    <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
      <IntegrationAddCard
        description={`Connect another service to ${namespace.label}.`}
        onclick={openCreate}
      />
      {#each cards as card (card.integration.id)}
        {@const cardProvider =
          providerCatalog.find(
            (provider) => provider.slug === card.integration.provider,
          ) ?? null}
        <IntegrationConnectionCard
          name={card.integration.name}
          provider={card.integration.provider}
          providerName={card.integration.provider_name}
          subtitle={card.target.label}
          description={cardProvider?.description ??
            "Connected external service."}
          enabled={card.integration.enabled}
          statusLabel={!card.integration.enabled
            ? "Disabled"
            : cardProvider?.source_required
              ? card.integration.source?.ready === true
                ? "Connected"
                : card.integration.source?.ready === false
                  ? "Authorization required"
                  : card.integration.source
                    ? "Source linked"
                    : "Setup incomplete"
              : "Enabled"}
          statusHealthy={card.integration.enabled &&
            (!cardProvider?.source_required ||
              card.integration.source?.ready === true)}
          detailLabel={card.integration.source?.name ?? card.target.slug}
          busy={working[card.integration.id]}
          onenabledchange={(enabled) => void setEnabled(card, enabled)}
          onconfigure={() => openConfigure(card)}
          onremove={() => requestRemove(card)}
        />
      {/each}
    </div>
  {/if}
</section>

<Dialog.Root bind:open={modalOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{editorTitle}</Dialog.Title>
      <Dialog.Description>
        {editing
          ? "Update this connection or reveal its stored credential."
          : "Choose a provider, then create a named connection."}
      </Dialog.Description>
    </Dialog.Header>
    {#key editorRevision}
      <IntegrationConnectionEditor
        providers={providerCatalog}
        {targets}
        integration={editing?.integration ?? null}
        initialTarget={editing?.target.slug ?? targets[0]?.slug ?? ""}
        onsave={saveConnection}
        oncancel={() => (modalOpen = false)}
        onreveal={editing ? revealCredential : null}
        ontest={editing ? testConnection : testNewConnection}
        {sourceConnection}
        {sourceLoading}
        {sourceWorking}
        {sourceError}
        onrefreshsource={editing ? () => loadSource(editing) : null}
        onbindsource={editing ? bindSource : null}
        mode={editorMode}
        requireSuccessfulTest={!editing}
        oncreatesource={editing ? createSource : null}
        ondisconnectsource={editing?.integration.source
          ? disconnectSource
          : null}
      />
    {/key}
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={removeDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title
        >Remove {pendingRemoveCard?.integration.name ??
          "integration"}?</AlertDialog.Title
      >
      <AlertDialog.Description>
        Repository links using it will also be removed. This cannot be undone.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => void confirmRemove()}>Remove</AlertDialog.Action
      >
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
