<script lang="ts">
  import { Check, Clipboard, KeyRound, Trash2 } from "lucide-svelte";
  import { toast } from "svelte-sonner";

  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import type { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";

  let { state: account }: { state: AccountSettingsState } = $props();
  let revokeDialogOpen = $state(false);
  let pendingApplication = $state<{ id: string; name: string } | null>(null);

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

  function requestRevoke(id: string, name: string) {
    pendingApplication = { id, name };
    revokeDialogOpen = true;
  }

  async function revokeApplication() {
    if (!pendingApplication) return;
    await account.deleteOauthApplication(pendingApplication.id);
    if (!account.error) {
      revokeDialogOpen = false;
      pendingApplication = null;
    }
  }
</script>

<div class="grid gap-5 lg:grid-cols-[minmax(0,1.2fr)_minmax(20rem,0.8fr)]">
  <Card.Root class="ring-foreground/20">
    <Card.Header class="border-b">
      <div class="flex items-start gap-3">
        <KeyRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <Card.Title>OAuth applications</Card.Title>
          <Card.Description>
            Register clients that need access to your Gitadel account.
          </Card.Description>
        </div>
      </div>
    </Card.Header>
    <Card.Content class="grid gap-5">
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
                The client secret is shown once. Copy both values into your
                client before leaving this page.
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
                    void copyCredential(
                      account.createdOauthClientId!,
                      "Client ID",
                    )}
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

      <ul class="divide-y rounded-lg border">
        {#each account.oauthApplications as application (application.id)}
          <li class="flex items-start justify-between gap-4 p-4">
            <div class="min-w-0">
              <p class="font-medium">{application.name}</p>
              <dl class="mt-2 grid gap-1 text-xs text-muted-foreground">
                <div class="flex min-w-0 gap-2">
                  <dt class="shrink-0 font-medium text-foreground/80">
                    Client ID
                  </dt>
                  <dd class="truncate font-mono">{application.client_id}</dd>
                </div>
                <div class="flex min-w-0 gap-2">
                  <dt class="shrink-0 font-medium text-foreground/80">
                    Redirect
                  </dt>
                  <dd class="truncate">{application.redirect_uri}</dd>
                </div>
              </dl>
            </div>
            <Button
              type="button"
              size="sm"
              variant="outline"
              class="shrink-0 gap-2"
              onclick={() => requestRevoke(application.id, application.name)}
            >
              <Trash2 class="size-3.5" />Revoke
            </Button>
          </li>
        {:else}
          <li class="p-5 text-sm text-muted-foreground">
            No OAuth applications yet. Create one with the redirect URI provided
            by the client you want to connect.
          </li>
        {/each}
      </ul>
    </Card.Content>
  </Card.Root>

  <div class="grid content-start gap-5">
    <section class="rounded-lg border bg-muted/25 p-5">
      <h2 class="text-sm font-semibold">Connect a client</h2>
      <ol class="mt-3 grid gap-2 text-sm leading-6 text-muted-foreground">
        <li>
          <strong class="text-foreground">1.</strong> Copy the redirect URI from the
          client you want to connect.
        </li>
        <li>
          <strong class="text-foreground">2.</strong> Create an application below
          using that exact URI.
        </li>
        <li>
          <strong class="text-foreground">3.</strong> Copy the generated client ID
          and secret back into the client.
        </li>
      </ol>
      <p class="mt-3 text-xs leading-5 text-muted-foreground">
        Use this Gitadel installation as the server URL. The client will request
        access when you authorize it.
      </p>
    </section>

    <form
      class="grid gap-4 rounded-lg border bg-card/25 p-5"
      onsubmit={(event) => {
        event.preventDefault();
        void account.createOauthApplication();
      }}
    >
      <div>
        <h2 class="text-sm font-semibold">Create application</h2>
        <p class="mt-1 text-xs leading-5 text-muted-foreground">
          The redirect URI must match the client's value exactly, including the
          scheme and path.
        </p>
      </div>
      <Field.Field>
        <Field.Label for="oauth-application-name">Name</Field.Label>
        <Input
          id="oauth-application-name"
          bind:value={account.oauthApplicationName}
          placeholder="My integration"
          maxlength={128}
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
      <Button type="submit" disabled={account.working}>
        Create OAuth application
      </Button>
    </form>
  </div>

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
</div>
