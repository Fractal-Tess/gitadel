<script lang="ts">
  import Check from "@lucide/svelte/icons/check";
  import Clipboard from "@lucide/svelte/icons/clipboard";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import { toast } from "svelte-sonner";

  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import type { OAuthSettingsState } from "$lib/settings/account/oauth-settings-state.svelte.js";

  let {
    state: account,
    showHeader = true,
  }: {
    state: OAuthSettingsState;
    showHeader?: boolean;
  } = $props();
  let createDialogOpen = $state(false);
  let revokeDialogOpen = $state(false);
  let pendingApplication = $state<{ id: string; name: string } | null>(null);

  const timestampFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });

  async function copyCredential(value: string, label: string) {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(value);
      } else {
        const carrier = document.createElement("textarea");
        carrier.value = value;
        carrier.setAttribute("readonly", "");
        carrier.style.position = "fixed";
        carrier.style.opacity = "0";
        document.body.append(carrier);
        carrier.select();
        document.execCommand("copy");
        carrier.remove();
      }
      toast.success(`${label} copied`);
    } catch {
      toast.error(`Could not copy the ${label.toLowerCase()}`, {
        description: "Select it and copy it manually instead.",
      });
    }
  }

  async function createApplication() {
    if (await account.createOauthApplication()) createDialogOpen = false;
  }

  function requestRevoke(id: string, name: string) {
    pendingApplication = { id, name };
    revokeDialogOpen = true;
  }

  async function revokeApplication() {
    if (!pendingApplication) return;
    if (await account.deleteOauthApplication(pendingApplication.id)) {
      revokeDialogOpen = false;
      pendingApplication = null;
    }
  }
</script>

<section
  class="space-y-6"
  aria-labelledby={showHeader ? "oauth-applications-heading" : undefined}
  aria-label={showHeader ? undefined : "OAuth applications"}
>
  {#if showHeader}
    <header>
      <h2
        id="oauth-applications-heading"
        class="text-lg font-semibold tracking-tight"
      >
        OAuth applications
      </h2>
      <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
        Register clients that need access to your Gitadel account.
      </p>
    </header>
  {/if}

  {#if account.createdOauthClientId && account.createdOauthClientSecret}
    <section
      class="grid gap-3 rounded-lg border border-amber-400/35 bg-amber-400/5 p-4"
      aria-labelledby="new-oauth-credentials"
    >
      <div class="flex items-start gap-3">
        <Check class="mt-0.5 size-4 shrink-0 text-amber-300" />
        <div>
          <h3 id="new-oauth-credentials" class="text-sm font-semibold">
            Save these credentials now
          </h3>
          <p class="mt-1 text-xs leading-5 text-muted-foreground">
            The client secret is shown once. Copy both values before leaving
            this page.
          </p>
        </div>
      </div>

      <div class="grid gap-3 sm:grid-cols-2">
        <Field.Field>
          <Field.Label for="oauth-client-id">Client ID</Field.Label>
          <div class="flex gap-2">
            <Input
              id="oauth-client-id"
              class="font-mono text-xs"
              value={account.createdOauthClientId}
              readonly
            />
            <Button
              type="button"
              variant="outline"
              aria-label="Copy client ID"
              onclick={() =>
                void copyCredential(account.createdOauthClientId!, "Client ID")}
            >
              <Clipboard class="size-4" />
            </Button>
          </div>
        </Field.Field>
        <Field.Field>
          <Field.Label for="oauth-client-secret">Client secret</Field.Label>
          <div class="flex gap-2">
            <Input
              id="oauth-client-secret"
              class="font-mono text-xs"
              value={account.createdOauthClientSecret}
              readonly
            />
            <Button
              type="button"
              variant="outline"
              aria-label="Copy client secret"
              onclick={() =>
                void copyCredential(
                  account.createdOauthClientSecret!,
                  "Client secret",
                )}
            >
              <Clipboard class="size-4" />
            </Button>
          </div>
        </Field.Field>
      </div>
    </section>
  {/if}

  <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
    <IntegrationAddCard
      title="New application"
      description="Register a client with its exact OAuth redirect URI."
      onclick={() => (createDialogOpen = true)}
    />

    {#each account.oauthApplications as application (application.id)}
      <article
        class="flex min-h-64 flex-col rounded-xl border bg-card/40 p-5 shadow-sm transition-colors hover:bg-card/60"
      >
        <header class="flex items-start gap-4">
          <span
            class="grid size-12 shrink-0 place-items-center rounded-xl border bg-muted"
            aria-hidden="true"
          >
            <KeyRound class="size-6 text-primary" />
          </span>
          <span class="min-w-0 flex-1">
            <span class="block truncate font-semibold">{application.name}</span>
            <span class="mt-0.5 block text-xs text-muted-foreground">
              OAuth application
            </span>
          </span>
        </header>

        <dl class="mt-5 grid gap-3 text-xs">
          <div class="min-w-0">
            <dt class="text-muted-foreground">Client ID</dt>
            <dd class="mt-0.5 truncate font-mono" title={application.client_id}>
              {application.client_id}
            </dd>
          </div>
          <div class="min-w-0">
            <dt class="text-muted-foreground">Redirect URI</dt>
            <dd class="mt-0.5 truncate" title={application.redirect_uri}>
              {application.redirect_uri}
            </dd>
          </div>
          <div>
            <dt class="text-muted-foreground">Created</dt>
            <dd class="mt-0.5">
              <time datetime={application.created_at}>
                {timestampFormatter.format(new Date(application.created_at))}
              </time>
            </dd>
          </div>
        </dl>

        <div class="mt-auto flex flex-wrap justify-end gap-2 pt-5">
          <Button
            type="button"
            size="sm"
            variant="outline"
            class="gap-2"
            onclick={() => copyCredential(application.client_id, "Client ID")}
          >
            <Clipboard class="size-3.5" />Copy client ID
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            class="gap-2"
            onclick={() => requestRevoke(application.id, application.name)}
          >
            <Trash2 class="size-3.5" />Revoke
          </Button>
        </div>
      </article>
    {/each}
  </div>

  <Dialog.Root bind:open={createDialogOpen}>
    <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
      <Dialog.Header>
        <Dialog.Title>Create OAuth application</Dialog.Title>
        <Dialog.Description>
          Enter the redirect URI supplied by the client. It must match exactly,
          including its scheme and path.
        </Dialog.Description>
      </Dialog.Header>
      <form
        class="grid gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void createApplication();
        }}
      >
        <Field.Field>
          <Field.Label for="oauth-application-name">Name</Field.Label>
          <Input
            id="oauth-application-name"
            bind:value={account.oauthApplicationName}
            placeholder="My integration"
            maxlength={128}
            autofocus
            required
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="oauth-redirect-uri">Redirect URI</Field.Label>
          <Input
            id="oauth-redirect-uri"
            type="url"
            bind:value={account.oauthRedirectUri}
            placeholder="https://app.example.com/oauth/callback"
            autocomplete="url"
            required
          />
          <Field.Description>
            This value is provided by the client you are connecting.
          </Field.Description>
        </Field.Field>
        <Dialog.Footer>
          <Dialog.Close>
            {#snippet child({ props })}
              <Button {...props} type="button" variant="outline">Cancel</Button>
            {/snippet}
          </Dialog.Close>
          <Button type="submit" disabled={account.working}>
            {account.working ? "Creating…" : "Create application"}
          </Button>
        </Dialog.Footer>
      </form>
    </Dialog.Content>
  </Dialog.Root>

  <AlertDialog.Root bind:open={revokeDialogOpen}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title>
          Revoke {pendingApplication?.name ?? "application"}?
        </AlertDialog.Title>
        <AlertDialog.Description>
          This client will lose access immediately.
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action
          variant="destructive"
          disabled={account.working}
          onclick={() => void revokeApplication()}
        >
          Revoke application
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>
</section>
