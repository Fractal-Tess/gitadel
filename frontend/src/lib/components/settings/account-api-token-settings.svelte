<script lang="ts">
  import { getLocalTimeZone, today } from "@internationalized/date";
  import CalendarDays from "@lucide/svelte/icons/calendar-days";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import Clipboard from "@lucide/svelte/icons/clipboard";
  import KeySquare from "@lucide/svelte/icons/key-square";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import { onMount } from "svelte";
  import { toast } from "svelte-sonner";
  import { z } from "zod";

  import {
    authorizedApplicationSchema,
    type AuthorizedApplication,
  } from "$lib/api/account.js";
  import { errorMessage, requestEmpty, requestJson } from "$lib/api/transport.js";
  import { copyText } from "$lib/clipboard.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import CredentialRemovalDialog, {
    type CredentialRemoval,
  } from "$lib/components/settings/credential-removal-dialog.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Calendar } from "$lib/components/ui/calendar/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Popover from "$lib/components/ui/popover/index.js";
  import { formatDateTime } from "$lib/format.js";
  import type { CredentialsSettingsState } from "$lib/settings/account/credentials-settings-state.svelte.js";

  let { credentials }: { credentials: CredentialsSettingsState } = $props();
  let tokenExpiryOpen = $state(false);
  let apiTokenDialogOpen = $state(false);
  let tokenRevealOpen = $state(false);
  let deleteDialogOpen = $state(false);
  let pendingDeletion = $state<CredentialRemoval | null>(null);
  let authorizations = $state.raw<AuthorizedApplication[] | null>(null);
  let authorizationsError = $state<string | null>(null);
  let revokingClientId = $state<string | null>(null);

  onMount(loadAuthorizations);

  async function loadAuthorizations(): Promise<void> {
    try {
      authorizations = await requestJson(
        "/api/v1/me/authorizations",
        z.array(authorizedApplicationSchema),
      );
      authorizationsError = null;
    } catch (caught) {
      authorizationsError = errorMessage(caught);
    }
  }

  async function revokeAuthorization(
    application: AuthorizedApplication,
  ): Promise<void> {
    revokingClientId = application.client_id;
    try {
      await requestEmpty(
        `/api/v1/me/authorizations/${encodeURIComponent(application.client_id)}`,
        { method: "DELETE" },
      );
      authorizations = (authorizations ?? []).filter(
        (entry) => entry.client_id !== application.client_id,
      );
      toast.success(`${application.application_name} no longer has access`);
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      revokingClientId = null;
    }
  }

  async function createApiToken() {
    credentials.createdToken = null;
    if ((await credentials.createApiToken()) && credentials.createdToken) {
      apiTokenDialogOpen = false;
      tokenRevealOpen = true;
    }
  }

  async function copyCreatedToken() {
    const token = credentials.createdToken;
    if (!token) return;
    try {
      await copyText(token);
      toast.success("API token copied");
    } catch {
      toast.error("Could not copy the api token", {
        description: "Select the token and copy it manually.",
      });
    }
  }

  function setTokenRevealOpen(open: boolean) {
    tokenRevealOpen = open;
    if (!open) credentials.createdToken = null;
  }

  function requestDeletion(target: CredentialRemoval) {
    pendingDeletion = target;
    deleteDialogOpen = true;
  }

  async function confirmDeletion() {
    const target = pendingDeletion;
    if (!target) return;
    if (await credentials.revokeToken(target.id)) {
      deleteDialogOpen = false;
      pendingDeletion = null;
    }
  }

  function tokenExpiry(expiresAt: string | null): string {
    return expiresAt
      ? `Expires ${new Date(expiresAt).toLocaleDateString()}`
      : "Never expires";
  }
</script>

<section class="space-y-6">
  <div class="motion-list grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
    <IntegrationAddCard
      title="New API token"
      description="Create a scoped credential for scripts and integrations."
      onclick={() => (apiTokenDialogOpen = true)}
    />

    {#each credentials.tokens as token (token.id)}
      <article
        class="flex min-h-72 flex-col rounded-xl border bg-card/40 p-5 shadow-sm transition-colors hover:bg-card/60"
      >
        <header class="flex items-start gap-3">
          <span
            class="grid size-10 shrink-0 place-items-center rounded-lg border bg-muted"
            aria-hidden="true"
          >
            <KeySquare class="size-5 text-primary" />
          </span>
          <span class="min-w-0 flex-1">
            <span class="block truncate font-medium">{token.name}</span>
            <span class="mt-0.5 block truncate text-xs text-muted-foreground">
              {token.scopes.join(", ")}
            </span>
          </span>
        </header>
        <dl class="mt-4 grid gap-3 text-xs">
          <div>
            <dt class="text-muted-foreground">Expiration</dt>
            <dd class="mt-0.5">{tokenExpiry(token.expires_at)}</dd>
          </div>
          <div class="grid grid-cols-2 gap-3">
            <span>
              <dt class="text-muted-foreground">Created</dt>
              <dd class="mt-0.5">
                <time datetime={token.created_at}>
                  {formatDateTime(token.created_at)}
                </time>
              </dd>
            </span>
            <span>
              <dt class="text-muted-foreground">Last used</dt>
              <dd class="mt-0.5">
                {#if token.last_used_at}
                  <time datetime={token.last_used_at}>
                    {formatDateTime(token.last_used_at)}
                  </time>
                {:else}
                  Never
                {/if}
              </dd>
            </span>
          </div>
        </dl>
        <div class="mt-auto flex justify-end pt-4">
          <Button
            size="sm"
            variant="outline"
            onclick={() =>
              requestDeletion({
                kind: "api-token",
                id: token.id,
                name: token.name,
              })}
          >
            Revoke
          </Button>
        </div>
      </article>
    {/each}
  </div>

  <section
    class="rounded-xl border bg-card/40 p-5 shadow-sm"
    aria-labelledby="authorized-applications-heading"
  >
    <header class="flex items-start gap-3">
      <ShieldCheck class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div>
        <h2 id="authorized-applications-heading" class="font-medium">
          Authorized applications
        </h2>
        <p class="mt-1 text-xs leading-5 text-muted-foreground">
          OAuth applications that hold access tokens for your account. Tokens
          expire after 30 days and cannot be refreshed, so an application asks
          again when it needs continued access. Revoking signs it out now.
        </p>
      </div>
    </header>
    {#if authorizationsError}
      <p class="mt-4 text-sm text-destructive">{authorizationsError}</p>
    {:else if authorizations === null}
      <p class="mt-4 text-sm text-muted-foreground">
        Loading authorized applications…
      </p>
    {:else if authorizations.length === 0}
      <p class="mt-4 text-sm text-muted-foreground">
        No application currently has access to your account.
      </p>
    {:else}
      <ul class="motion-list mt-4 grid divide-y rounded-lg border">
        {#each authorizations as application (application.client_id)}
          <li class="grid gap-3 px-4 py-3 text-sm sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
            <div class="min-w-0">
              <p class="truncate font-medium">{application.application_name}</p>
              <p class="mt-0.5 truncate font-mono text-xs text-muted-foreground">
                {application.client_id}
              </p>
              <dl class="mt-2 grid gap-x-6 gap-y-1 text-xs sm:grid-cols-2">
                <div>
                  <dt class="text-muted-foreground">Scopes</dt>
                  <dd>{application.scopes.join(", ")}</dd>
                </div>
                <div>
                  <dt class="text-muted-foreground">Authorized</dt>
                  <dd>
                    <time datetime={application.created_at}>
                      {formatDateTime(application.created_at)}
                    </time>
                  </dd>
                </div>
                <div>
                  <dt class="text-muted-foreground">Last used</dt>
                  <dd>{formatDateTime(application.last_used_at)}</dd>
                </div>
                <div>
                  <dt class="text-muted-foreground">Expires</dt>
                  <dd>
                    <time datetime={application.expires_at}>
                      {formatDateTime(application.expires_at)}
                    </time>
                  </dd>
                </div>
              </dl>
            </div>
            <Button
              size="sm"
              variant="outline"
              disabled={revokingClientId !== null}
              onclick={() => void revokeAuthorization(application)}
            >
              {revokingClientId === application.client_id
                ? "Revoking…"
                : "Revoke"}
            </Button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <Dialog.Root bind:open={apiTokenDialogOpen}>
    <Dialog.Content
      class="max-h-[calc(100svh-2rem)] overflow-y-auto ring-foreground/20 sm:max-w-lg"
    >
      <Dialog.Header>
        <Dialog.Title>Create an API token</Dialog.Title>
        <Dialog.Description>
          Choose the narrowest permissions and expiration that fit your use
          case.
        </Dialog.Description>
      </Dialog.Header>
      <form
        class="grid gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void createApiToken();
        }}
      >
        <Field.Field>
          <Field.Label for="token-name">Token name</Field.Label>
          <Input
            id="token-name"
            bind:value={credentials.tokenName}
            placeholder="Deployment script"
            required
          />
        </Field.Field>

        <Field.Field>
          <Field.Label for="token-expiry">Expiration date</Field.Label>
          <div class="flex gap-2">
            <Popover.Root bind:open={tokenExpiryOpen}>
              <Popover.Trigger id="token-expiry">
                {#snippet child({ props })}
                  <Button
                    {...props}
                    variant="outline"
                    class="min-w-0 flex-1 justify-between font-normal"
                  >
                    <span class="truncate">
                      {credentials.tokenExpiresOn
                        ? credentials.tokenExpiresOn
                            .toDate(getLocalTimeZone())
                            .toLocaleDateString(undefined, {
                              dateStyle: "medium",
                            })
                        : "Never expires"}
                    </span>
                    <ChevronDown
                      class="size-4 shrink-0 text-muted-foreground"
                    />
                  </Button>
                {/snippet}
              </Popover.Trigger>
              <Popover.Content class="w-auto overflow-hidden p-0" align="start">
                <Calendar
                  type="single"
                  bind:value={credentials.tokenExpiresOn}
                  minValue={today(getLocalTimeZone()).add({ days: 1 })}
                  maxValue={today(getLocalTimeZone()).add({ days: 3650 })}
                  captionLayout="dropdown"
                  initialFocus
                  onValueChange={() => (tokenExpiryOpen = false)}
                />
              </Popover.Content>
            </Popover.Root>
            {#if credentials.tokenExpiresOn}
              <Button
                type="button"
                variant="outline"
                aria-label="Clear expiration date"
                onclick={() => (credentials.tokenExpiresOn = undefined)}
              >
                <CalendarDays class="size-4" />
              </Button>
            {/if}
          </div>
        </Field.Field>

        <Field.Set class="gap-2">
          <Field.Legend variant="label">Token permissions</Field.Legend>
          <Field.Description>
            Grant only the access this token needs.
          </Field.Description>
          <div class="divide-y overflow-hidden rounded-lg border">
            <div
              class={credentials.tokenRead
                ? "flex items-start justify-between gap-4 bg-primary/5 px-3.5 py-3"
                : "flex items-start justify-between gap-4 px-3.5 py-3"}
            >
              <div class="min-w-0">
                <label class="cursor-pointer font-medium" for="scope-read">
                  Read access
                </label>
                <p class="mt-0.5 text-xs leading-5 text-muted-foreground">
                  Browse repositories, source, organizations, and account data.
                </p>
              </div>
              <Checkbox
                class="mt-0.5"
                id="scope-read"
                bind:checked={credentials.tokenRead}
              />
            </div>
            <div
              class={credentials.tokenWrite
                ? "flex items-start justify-between gap-4 bg-primary/5 px-3.5 py-3"
                : "flex items-start justify-between gap-4 px-3.5 py-3"}
            >
              <div class="min-w-0">
                <label class="cursor-pointer font-medium" for="scope-write">
                  Write access
                </label>
                <p class="mt-0.5 text-xs leading-5 text-muted-foreground">
                  Create repositories and make changes through the API.
                </p>
              </div>
              <Checkbox
                class="mt-0.5"
                id="scope-write"
                bind:checked={credentials.tokenWrite}
              />
            </div>
            <div
              class={credentials.tokenSshKeys
                ? "flex items-start justify-between gap-4 bg-primary/5 px-3.5 py-3"
                : "flex items-start justify-between gap-4 px-3.5 py-3"}
            >
              <div class="min-w-0">
                <label class="cursor-pointer font-medium" for="scope-ssh-keys">
                  Manage SSH keys
                </label>
                <p class="mt-0.5 text-xs leading-5 text-muted-foreground">
                  Add and remove SSH keys for your account.
                </p>
              </div>
              <Checkbox
                class="mt-0.5"
                id="scope-ssh-keys"
                bind:checked={credentials.tokenSshKeys}
              />
            </div>
          </div>
        </Field.Set>

        <Dialog.Footer>
          <Dialog.Close>
            {#snippet child({ props })}
              <Button {...props} type="button" variant="outline">Cancel</Button>
            {/snippet}
          </Dialog.Close>
          <Button type="submit" disabled={credentials.working}>
            {credentials.working ? "Creating…" : "Create token"}
          </Button>
        </Dialog.Footer>
      </form>
    </Dialog.Content>
  </Dialog.Root>

  <Dialog.Root open={tokenRevealOpen} onOpenChange={setTokenRevealOpen}>
    <Dialog.Content
      class="ring-foreground/20 sm:max-w-lg"
      showCloseButton={false}
    >
      <Dialog.Header>
        <Dialog.Title>Copy your API token</Dialog.Title>
        <Dialog.Description>
          This token is shown only once. Copy it before closing this dialog.
        </Dialog.Description>
      </Dialog.Header>
      <div class="rounded-lg border bg-muted/40 p-3">
        <code class="block select-all break-all text-sm">
          {credentials.createdToken}
        </code>
      </div>
      <p class="text-xs leading-5 text-muted-foreground">
        Gitadel stores only a secure hash. This value cannot be recovered later.
      </p>
      <Dialog.Footer>
        <Button variant="outline" onclick={copyCreatedToken}>
          <Clipboard data-icon="inline-start" />Copy token
        </Button>
        <Button onclick={() => setTokenRevealOpen(false)}>Done</Button>
      </Dialog.Footer>
    </Dialog.Content>
  </Dialog.Root>
</section>

<CredentialRemovalDialog
  bind:open={deleteDialogOpen}
  target={pendingDeletion}
  working={credentials.working}
  onconfirm={() => void confirmDeletion()}
/>
