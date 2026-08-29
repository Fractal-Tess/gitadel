<script lang="ts">
  import { LoaderCircle } from "lucide-svelte";
  import { onMount } from "svelte";

  import {
    ApiFailure,
    jsonBody,
    mirrorIdentitySchema,
    requestEmpty,
    requestJson,
    type MirrorIdentity,
  } from "$lib/api.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { takeNamespaceMirrorSettings } from "$lib/namespace-preload.js";
  import type { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  type Target = { slug: string; label: string };
  type IdentityCard = { target: Target; identity: MirrorIdentity };

  let {
    state: account,
    namespace = null,
  }: {
    state: AccountSettingsState;
    namespace?: Target | null;
  } = $props();

  const app = useAppState();
  const timestampFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
  const targets = $derived<Target[]>(
    namespace
      ? [namespace]
      : [
          ...(app.authStatus?.user?.username
            ? [
                {
                  slug: app.authStatus.user.username,
                  label: "Personal repositories",
                },
              ]
            : []),
          ...account.organizations
            .filter((organization) => organization.role === "owner")
            .map((organization) => ({
              slug: organization.slug,
              label: organization.display_name || organization.slug,
            })),
        ],
  );

  let identities = $state<Record<string, MirrorIdentity[]>>({});
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let working = $state<Record<string, boolean>>({});
  let errors = $state<Record<string, string | null>>({});

  let modalOpen = $state(false);
  let editing = $state<IdentityCard | null>(null);
  let target = $state<Target | null>(null);
  let name = $state("");
  let serverUrl = $state("");
  let token = $state("");
  let saving = $state(false);
  let formError = $state<string | null>(null);
  let removeDialogOpen = $state(false);
  let pendingRemove = $state<IdentityCard | null>(null);

  const cards = $derived<IdentityCard[]>(
    targets.flatMap((currentTarget) =>
      (identities[currentTarget.slug] ?? []).map((identity) => ({
        target: currentTarget,
        identity,
      })),
    ),
  );
  const selectedTarget = $derived(target ?? editing?.target ?? null);

  onMount(() => {
    void load();
  });

  function message(caught: unknown, fallback: string): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  function identitiesPath(slug: string): string {
    return `/api/v1/namespaces/${encodeURIComponent(slug)}/mirror-identities`;
  }

  async function load(): Promise<void> {
    loading = true;
    loadError = null;
    try {
      const responses = await Promise.all(
        targets.map(async ({ slug }) => ({
          slug,
          values: (await takeNamespaceMirrorSettings(slug)).identities,
        })),
      );
      for (const response of responses) identities[response.slug] = response.values;
    } catch (caught) {
      loadError = message(caught, "Could not load mirror identities.");
    } finally {
      loading = false;
    }
  }

  function openCreate(nextTarget: Target): void {
    editing = null;
    target = nextTarget;
    name = "";
    serverUrl = "";
    token = "";
    formError = null;
    modalOpen = true;
  }

  function openEdit(card: IdentityCard): void {
    editing = card;
    target = card.target;
    name = card.identity.name;
    serverUrl = card.identity.instance_url ?? "";
    token = "";
    formError = null;
    modalOpen = true;
  }

  async function save(): Promise<void> {
    const currentTarget = selectedTarget;
    if (!currentTarget) return;
    if (!editing && !token.trim()) {
      formError = "Enter an access token.";
      return;
    }
    saving = true;
    formError = null;
    try {
      const identity = await requestJson(
        editing
          ? `${identitiesPath(currentTarget.slug)}/${encodeURIComponent(editing.identity.id)}`
          : identitiesPath(currentTarget.slug),
        mirrorIdentitySchema,
        {
          method: editing ? "PUT" : "POST",
          body: jsonBody({
            name,
            server_url: serverUrl,
            ...((!editing || token.trim()) && { token }),
          }),
        },
      );
      identities[currentTarget.slug] = editing
        ? (identities[currentTarget.slug] ?? []).map((current) =>
            current.id === identity.id ? identity : current,
          )
        : [...(identities[currentTarget.slug] ?? []), identity];
      token = "";
      modalOpen = false;
    } catch (caught) {
      formError = message(caught, "Could not save the mirror identity.");
    } finally {
      saving = false;
    }
  }

  async function remove(): Promise<void> {
    const card = pendingRemove;
    if (!card) return;
    working[card.identity.id] = true;
    errors[card.identity.id] = null;
    try {
      await requestEmpty(
        `${identitiesPath(card.target.slug)}/${encodeURIComponent(card.identity.id)}`,
        { method: "DELETE" },
      );
      identities[card.target.slug] = (identities[card.target.slug] ?? []).filter(
        (identity) => identity.id !== card.identity.id,
      );
      pendingRemove = null;
      removeDialogOpen = false;
    } catch (caught) {
      errors[card.identity.id] = message(
        caught,
        `Could not remove ${card.identity.name}.`,
      );
      removeDialogOpen = false;
    } finally {
      working[card.identity.id] = false;
    }
  }

  function providerName(provider: MirrorIdentity["provider"]): string {
    if (!provider) return "Git server";
    return {
      github: "GitHub",
      gitlab: "GitLab",
      gitea: "Gitea",
      forgejo: "Forgejo",
    }[provider];
  }

  function formatTimestamp(value: string | null): string {
    return value ? timestampFormatter.format(new Date(value)) : "Never";
  }
</script>

<section class="space-y-6" aria-labelledby="mirror-identities-heading">
  <header>
    <h2 id="mirror-identities-heading" class="text-lg font-semibold tracking-tight">
      Mirror identities
    </h2>
    <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
      Authentication available to mirrors owned by {namespace?.label ??
        "your namespaces"}. Gitadel detects the git server when an identity is
      saved.
    </p>
  </header>

  {#if loading}
    <p class="flex items-center gap-2 text-sm text-muted-foreground">
      <LoaderCircle class="size-4 animate-spin" />Loading identities…
    </p>
  {:else if loadError}
    <p class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive" role="alert">
      {loadError}
    </p>
  {:else}
    <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
      {#each targets as currentTarget (currentTarget.slug)}
        <IntegrationAddCard
          title="Add identity"
          description={`Add an access token for private repositories owned by ${currentTarget.label}.`}
          onclick={() => openCreate(currentTarget)}
        />
        <IntegrationConnectionCard
          name="Public"
          provider="public"
          providerName="No authentication"
          subtitle={currentTarget.label}
          description="Mirror public HTTPS repositories without storing or sending a credential."
          enabled={true}
          statusLabel="Built in"
          statusHealthy={true}
          detailLabel="Cannot be removed"
        />
      {/each}

      {#each cards as card (card.identity.id)}
        <IntegrationConnectionCard
          name={card.identity.name}
          provider={card.identity.provider ?? "token"}
          providerName={providerName(card.identity.provider)}
          subtitle={card.target.label}
          description={`Authenticates HTTPS mirrors from ${card.identity.instance_url ?? "its configured git server"}.`}
          enabled={true}
          statusLabel="Available"
          statusHealthy={true}
          detailLabel={`Last used ${formatTimestamp(card.identity.last_used_at)}`}
          busy={working[card.identity.id]}
          error={errors[card.identity.id]}
          configureLabel="Configure"
          onconfigure={() => openEdit(card)}
          onremove={() => {
            pendingRemove = card;
            removeDialogOpen = true;
          }}
        />
      {/each}
    </div>
  {/if}
</section>

<Dialog.Root bind:open={modalOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>
        {editing ? `Configure ${editing.identity.name}` : "Add identity"}
      </Dialog.Title>
      <Dialog.Description>
        {editing
          ? "Update this identity or replace its stored access token."
          : "Create a named access token for private repository mirrors."}
      </Dialog.Description>
    </Dialog.Header>
    <form
      class="grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      {#if selectedTarget}
        <div class="rounded-md border bg-card/20 p-3 text-sm">
          <span class="text-muted-foreground">Owner</span>
          <span class="ml-3 font-medium">{selectedTarget.label}</span>
          <span class="ml-2 font-mono text-xs text-muted-foreground">
            {selectedTarget.slug}
          </span>
        </div>
      {/if}
      {#if formError}
        <p class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive" role="alert">
          {formError}
        </p>
      {/if}
      <Field.Field>
        <Field.Label for="mirror-identity-name">Name</Field.Label>
        <Input id="mirror-identity-name" bind:value={name} maxlength={80} required />
      </Field.Field>
      <Field.Field>
        <Field.Label for="mirror-identity-server">Git server URL</Field.Label>
        <Input
          id="mirror-identity-server"
          type="url"
          bind:value={serverUrl}
          placeholder="https://github.com"
          required
        />
        <Field.Description>Enter the server origin, not a repository URL.</Field.Description>
      </Field.Field>
      <Field.Field>
        <Field.Label for="mirror-identity-token">Access token</Field.Label>
        <Input
          id="mirror-identity-token"
          type="password"
          bind:value={token}
          autocomplete="off"
          placeholder={editing ? "Leave blank to keep the current token" : "Required"}
          required={!editing}
        />
        <Field.Description>The token is checked against the server and is never shown again.</Field.Description>
      </Field.Field>
      <Dialog.Footer>
        <Button
          type="button"
          variant="ghost"
          onclick={() => (modalOpen = false)}>Cancel</Button
        >
        <Button
          type="submit"
          disabled={saving ||
            !name.trim() ||
            !serverUrl.trim() ||
            (!editing && !token.trim())}
        >
          {saving ? "Checking…" : editing ? "Save identity" : "Add identity"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={removeDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Remove {pendingRemove?.identity.name ?? "mirror identity"}?</AlertDialog.Title>
      <AlertDialog.Description>
        It must first be unlinked from every mirror. This cannot be undone.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={() => void remove()}>Remove</AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
