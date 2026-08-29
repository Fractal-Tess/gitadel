<script lang="ts">
  import { getLocalTimeZone, today } from "@internationalized/date";
  import BookOpen from "@lucide/svelte/icons/book-open";
  import CalendarDays from "@lucide/svelte/icons/calendar-days";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import Clipboard from "@lucide/svelte/icons/clipboard";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import KeySquare from "@lucide/svelte/icons/key-square";
  import LockKeyhole from "@lucide/svelte/icons/lock-keyhole";
  import Terminal from "@lucide/svelte/icons/terminal";
  import UserRound from "@lucide/svelte/icons/user-round";
  import { onMount } from "svelte";
  import { toast } from "svelte-sonner";

  import AccountAvatarSettings from "$lib/components/settings/account-avatar-settings.svelte";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Calendar } from "$lib/components/ui/calendar/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Popover from "$lib/components/ui/popover/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import type { CredentialsSettingsState } from "$lib/settings/account/credentials-settings-state.svelte.js";
  import type {
    PasswordSettingsState,
    ProfileSettingsState,
  } from "$lib/settings/account/profile-settings-state.svelte.js";

  type DeletionTarget = {
    kind: "passkey" | "ssh-key" | "api-token";
    id: string;
    name: string;
  };

  let {
    profile,
    password,
    credentials,
    view,
  }: {
    profile: ProfileSettingsState;
    password: PasswordSettingsState;
    credentials: CredentialsSettingsState;
    view: "account" | "authentication" | "ssh-keys" | "api-tokens";
  } = $props();
  const timestampFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
  let passkeysAvailable = $state(true);
  let tokenExpiryOpen = $state(false);
  let sshKeyDialogOpen = $state(false);
  let apiTokenDialogOpen = $state(false);
  let tokenRevealOpen = $state(false);
  let deleteDialogOpen = $state(false);
  let pendingDeletion = $state<DeletionTarget | null>(null);

  onMount(() => {
    passkeysAvailable =
      globalThis.isSecureContext &&
      typeof navigator.credentials?.create === "function" &&
      typeof PublicKeyCredential !== "undefined";
  });

  function updateSshPublicKey(publicKey: string): void {
    credentials.sshPublicKey = publicKey;
    if (credentials.sshKeyName.trim()) return;

    const firstLine = publicKey.trim().split(/\r?\n/u)[0] ?? "";
    const comment = firstLine.match(
      /^(?:ssh-(?:ed25519|rsa)|ecdsa-sha2-\S+|sk-\S+)\s+\S+\s+(.+?)\s*$/u,
    )?.[1];
    if (comment) credentials.sshKeyName = comment;
  }

  async function addSshKey() {
    await credentials.addSshKey();
    if (!credentials.error) sshKeyDialogOpen = false;
  }

  async function createApiToken() {
    credentials.createdToken = null;
    await credentials.createApiToken();
    if (!credentials.error && credentials.createdToken) {
      apiTokenDialogOpen = false;
      tokenRevealOpen = true;
    }
  }

  async function copyText(
    value: string,
    label: string,
    failureDescription: string,
  ) {
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
        description: failureDescription,
      });
    }
  }

  async function copyCreatedToken() {
    const token = credentials.createdToken;
    if (!token) return;
    await copyText(
      token,
      "API token",
      "Select the token and copy it manually.",
    );
  }

  function setTokenRevealOpen(open: boolean) {
    tokenRevealOpen = open;
    if (!open) credentials.createdToken = null;
  }

  function requestDeletion(target: DeletionTarget) {
    pendingDeletion = target;
    deleteDialogOpen = true;
  }

  async function confirmDeletion() {
    const target = pendingDeletion;
    if (!target) return;

    if (target.kind === "passkey") {
      await credentials.removePasskey(target.id);
    } else if (target.kind === "ssh-key") {
      await credentials.removeSshKey(target.id);
    } else {
      await credentials.revokeToken(target.id);
    }
    if (!credentials.error) {
      deleteDialogOpen = false;
      pendingDeletion = null;
    }
  }

  function tokenExpiry(expiresAt: string | null): string {
    return expiresAt
      ? `Expires ${new Date(expiresAt).toLocaleDateString()}`
      : "Never expires";
  }

  function formatTimestamp(value: string | null): string {
    return value ? timestampFormatter.format(new Date(value)) : "Never";
  }
</script>

<div
  class={view === "ssh-keys" || view === "api-tokens"
    ? ""
    : "divide-y divide-border overflow-hidden rounded-xl bg-card/20 ring-1 ring-foreground/15"}
>
  {#if view === "account"}
    <AccountAvatarSettings />

    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="username-heading"
    >
      <header class="flex items-start gap-3">
        <UserRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h2 id="username-heading" class="font-semibold">Username</h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Your sign-in name and personal repository namespace.
          </p>
        </div>
      </header>

      <form
        class="grid max-w-2xl gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void profile.updateUsername();
        }}
      >
        <Field.Field>
          <Field.Label for="account-username">Username</Field.Label>
          <Input
            id="account-username"
            autocomplete="username"
            bind:value={profile.username}
            maxlength={39}
            disabled={profile.working}
            required
          />
          <Field.Description>
            Press Enter to save. Repository URLs change with your username, so
            update existing Git remotes afterward.
          </Field.Description>
        </Field.Field>
      </form>
    </section>

    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="repository-defaults-heading"
    >
      <header class="flex items-start gap-3">
        <BookOpen class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h2 id="repository-defaults-heading" class="font-semibold">
            Repository defaults
          </h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Choose the initial visibility for repositories you create.
          </p>
        </div>
      </header>

      <Field.Field class="max-w-2xl">
        <Field.Label for="account-default-repository-visibility">
          Default visibility
        </Field.Label>
        <Select.Root
          type="single"
          value={profile.defaultRepositoryVisibility}
          onValueChange={(value) => {
            if (value === "public" || value === "private") {
              void profile.updateRepositoryVisibility(value);
            }
          }}
        >
          <Select.Trigger
            id="account-default-repository-visibility"
            class="w-full"
            disabled={profile.working}
          >
            {profile.defaultRepositoryVisibility === "private"
              ? "Private"
              : "Public"}
          </Select.Trigger>
          <Select.Content>
            <Select.Item value="private">Private</Select.Item>
            <Select.Item value="public">Public</Select.Item>
          </Select.Content>
        </Select.Root>
        <Field.Description>
          You can choose a different visibility when creating a repository.
        </Field.Description>
      </Field.Field>
    </section>
  {/if}

  {#if view === "authentication"}
    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="password-heading"
    >
      <header class="flex items-start gap-3">
        <LockKeyhole class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h2 id="password-heading" class="font-semibold">Password</h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Use at least 12 characters. Other browser sessions will be signed
            out.
          </p>
        </div>
      </header>

      <form
        class="grid max-w-2xl gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void password.updatePassword();
        }}
      >
        <Field.Field>
          <Field.Label for="password-current">Current password</Field.Label>
          <Input
            id="password-current"
            type="password"
            autocomplete="current-password"
            bind:value={password.currentPassword}
            required
          />
        </Field.Field>
        <div class="grid gap-4 sm:grid-cols-2">
          <Field.Field>
            <Field.Label for="password-new">New password</Field.Label>
            <Input
              id="password-new"
              type="password"
              autocomplete="new-password"
              bind:value={password.newPassword}
              minlength={12}
              required
            />
          </Field.Field>
          <Field.Field>
            <Field.Label for="password-confirm">Confirm password</Field.Label>
            <Input
              id="password-confirm"
              type="password"
              autocomplete="new-password"
              bind:value={password.confirmPassword}
              minlength={12}
              required
            />
          </Field.Field>
        </div>
        <Button class="w-fit" type="submit" disabled={password.working}>
          Update password
        </Button>
      </form>
    </section>

    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="passkeys-heading"
    >
      <header class="flex items-start gap-3">
        <KeyRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h2 id="passkeys-heading" class="font-semibold">Passkeys</h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Use your device or security key instead of a password.
          </p>
        </div>
      </header>

      <div class="grid max-w-2xl gap-5">
        <ul class="grid gap-2">
          {#each credentials.passkeys as passkey (passkey.id)}
            <li
              class="flex items-center justify-between gap-3 rounded-lg border bg-background/30 p-3"
            >
              <span class="min-w-0 truncate font-medium">{passkey.name}</span>
              <Button
                size="sm"
                variant="outline"
                onclick={() =>
                  requestDeletion({
                    kind: "passkey",
                    id: passkey.id,
                    name: passkey.name,
                  })}
              >
                Remove
              </Button>
            </li>
          {:else}
            <li class="text-sm text-muted-foreground">No passkeys added.</li>
          {/each}
        </ul>

        {#if !passkeysAvailable}
          <p
            class="rounded-lg border border-amber-400/35 bg-amber-400/5 p-3 text-xs text-amber-200"
          >
            Passkeys require HTTPS and a supported browser.
          </p>
        {/if}

        <form
          class="grid gap-4 border-t pt-5"
          onsubmit={(event) => {
            event.preventDefault();
            void credentials.addPasskey();
          }}
        >
          <Field.Field>
            <Field.Label for="passkey-name">Passkey name</Field.Label>
            <Input
              id="passkey-name"
              bind:value={credentials.passkeyName}
              disabled={!passkeysAvailable}
              required
            />
          </Field.Field>
          <Button
            class="w-fit"
            type="submit"
            disabled={credentials.working || !passkeysAvailable}
          >
            Add passkey
          </Button>
        </form>
      </div>
    </section>
  {/if}

  {#if view === "ssh-keys"}
    <section class="space-y-6">
      <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        <IntegrationAddCard
          title="New SSH key"
          description="Add a public key for Git authentication and commit signing."
          onclick={() => (sshKeyDialogOpen = true)}
        />

        {#each credentials.sshKeys as key (key.id)}
          <article
            class="flex min-h-72 flex-col rounded-xl border bg-card/40 p-5 shadow-sm transition-colors hover:bg-card/60"
          >
            <header class="flex items-start gap-3">
              <span
                class="grid size-10 shrink-0 place-items-center rounded-lg border bg-muted"
                aria-hidden="true"
              >
                <Terminal class="size-5 text-primary" />
              </span>
              <span class="min-w-0 flex-1">
                <span class="block truncate font-medium">{key.name}</span>
                <span class="mt-0.5 block text-xs text-muted-foreground">
                  Authentication and signing
                </span>
              </span>
            </header>
            <code
              class="mt-4 block truncate text-xs text-muted-foreground"
              title={key.fingerprint}
            >
              {key.fingerprint}
            </code>
            <dl class="mt-4 grid grid-cols-2 gap-3 text-xs">
              <div>
                <dt class="text-muted-foreground">Created</dt>
                <dd class="mt-0.5">
                  <time datetime={key.created_at}>
                    {formatTimestamp(key.created_at)}
                  </time>
                </dd>
              </div>
              <div>
                <dt class="text-muted-foreground">Last used</dt>
                <dd class="mt-0.5">
                  {#if key.last_used_at}
                    <time datetime={key.last_used_at}>
                      {formatTimestamp(key.last_used_at)}
                    </time>
                  {:else}
                    Never
                  {/if}
                </dd>
              </div>
            </dl>
            <div class="mt-auto flex flex-wrap justify-end gap-2 pt-4">
              <Button
                size="sm"
                variant="outline"
                class="gap-2"
                onclick={() =>
                  copyText(
                    key.public_key,
                    "SSH public key",
                    "Copy the key from its original file and try again.",
                  )}
              >
                <Clipboard class="size-3.5" />Copy public key
              </Button>
              <Button
                size="sm"
                variant="outline"
                onclick={() =>
                  requestDeletion({
                    kind: "ssh-key",
                    id: key.id,
                    name: key.name,
                  })}
              >
                Remove
              </Button>
            </div>
          </article>
        {/each}
      </div>

      <Dialog.Root bind:open={sshKeyDialogOpen}>
        <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
          <Dialog.Header>
            <Dialog.Title>Add an SSH key</Dialog.Title>
            <Dialog.Description>
              Add a public key for SSH authentication and signed commit
              verification.
            </Dialog.Description>
          </Dialog.Header>
          <form
            class="grid gap-4"
            onsubmit={(event) => {
              event.preventDefault();
              void addSshKey();
            }}
          >
            <Field.Field>
              <Field.Label for="ssh-key-name">Key name</Field.Label>
              <Input
                id="ssh-key-name"
                bind:value={credentials.sshKeyName}
                placeholder="Work laptop"
                autofocus
                required
              />
            </Field.Field>
            <Field.Field>
              <Field.Label for="ssh-public-key">Public key</Field.Label>
              <Textarea
                id="ssh-public-key"
                class="font-mono text-xs"
                rows={5}
                value={credentials.sshPublicKey}
                oninput={(event) =>
                  updateSshPublicKey(event.currentTarget.value)}
                placeholder="ssh-ed25519 AAAA…"
                required
              />
              <Field.Description>
                Paste the complete OpenSSH public key, including its key type.
              </Field.Description>
            </Field.Field>
            <Dialog.Footer>
              <Dialog.Close>
                {#snippet child({ props })}
                  <Button {...props} type="button" variant="outline">
                    Cancel
                  </Button>
                {/snippet}
              </Dialog.Close>
              <Button type="submit" disabled={credentials.working}>
                {credentials.working ? "Adding…" : "Add SSH key"}
              </Button>
            </Dialog.Footer>
          </form>
        </Dialog.Content>
      </Dialog.Root>
    </section>
  {/if}

  {#if view === "api-tokens"}
    <section class="space-y-6">
      <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
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
                <span
                  class="mt-0.5 block truncate text-xs text-muted-foreground"
                >
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
                      {formatTimestamp(token.created_at)}
                    </time>
                  </dd>
                </span>
                <span>
                  <dt class="text-muted-foreground">Last used</dt>
                  <dd class="mt-0.5">
                    {#if token.last_used_at}
                      <time datetime={token.last_used_at}>
                        {formatTimestamp(token.last_used_at)}
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
                  <Popover.Content
                    class="w-auto overflow-hidden p-0"
                    align="start"
                  >
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
                      Browse repositories, source, organizations, and account
                      data.
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
                    <label
                      class="cursor-pointer font-medium"
                      for="scope-ssh-keys"
                    >
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
                  <Button {...props} type="button" variant="outline">
                    Cancel
                  </Button>
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
            Gitadel stores only a secure hash. This value cannot be recovered
            later.
          </p>
          <Dialog.Footer>
            <Button class="gap-2" variant="outline" onclick={copyCreatedToken}>
              <Clipboard class="size-4" />Copy token
            </Button>
            <Button onclick={() => setTokenRevealOpen(false)}>Done</Button>
          </Dialog.Footer>
        </Dialog.Content>
      </Dialog.Root>
    </section>
  {/if}

  <AlertDialog.Root bind:open={deleteDialogOpen}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title>
          {pendingDeletion?.kind === "api-token" ? "Revoke" : "Remove"}
          {pendingDeletion?.name ?? "credential"}?
        </AlertDialog.Title>
        <AlertDialog.Description>
          {#if pendingDeletion?.kind === "api-token"}
            Any script or integration using this token will lose access
            immediately.
          {:else if pendingDeletion?.kind === "ssh-key"}
            This key will no longer authenticate Git operations over SSH.
          {:else}
            This passkey will no longer be available for sign-in.
          {/if}
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action
          variant="destructive"
          disabled={credentials.working}
          onclick={() => void confirmDeletion()}
        >
          {pendingDeletion?.kind === "api-token" ? "Revoke token" : "Remove"}
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>
</div>
