<script lang="ts">
  import { getLocalTimeZone, today } from "@internationalized/date";
  import {
    CalendarDays,
    ChevronDown,
    Clipboard,
    KeyRound,
    KeySquare,
    LockKeyhole,
    Terminal,
    UserRound,
  } from "lucide-svelte";
  import { onMount } from "svelte";
  import { toast } from "svelte-sonner";

  import AccountAvatarSettings from "$lib/components/settings/account-avatar-settings.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Calendar } from "$lib/components/ui/calendar/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Popover from "$lib/components/ui/popover/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import type { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";

  type DeletionTarget = {
    kind: "passkey" | "ssh-key" | "api-token";
    id: string;
    name: string;
  };

  let { state: account }: { state: AccountSettingsState } = $props();
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
    account.sshPublicKey = publicKey;
    if (account.sshKeyName.trim()) return;

    const firstLine = publicKey.trim().split(/\r?\n/u)[0] ?? "";
    const comment = firstLine.match(
      /^(?:ssh-(?:ed25519|rsa)|ecdsa-sha2-\S+|sk-\S+)\s+\S+\s+(.+?)\s*$/u,
    )?.[1];
    if (comment) account.sshKeyName = comment;
  }

  async function addSshKey() {
    await account.addSshKey();
    if (!account.error) sshKeyDialogOpen = false;
  }

  async function createApiToken() {
    account.createdToken = null;
    await account.createApiToken();
    if (!account.error && account.createdToken) {
      apiTokenDialogOpen = false;
      tokenRevealOpen = true;
    }
  }

  async function copyCreatedToken() {
    const token = account.createdToken;
    if (!token) return;

    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(token);
      } else {
        const carrier = document.createElement("textarea");
        carrier.value = token;
        carrier.setAttribute("readonly", "");
        carrier.style.position = "fixed";
        carrier.style.opacity = "0";
        document.body.append(carrier);
        carrier.select();
        document.execCommand("copy");
        carrier.remove();
      }
      toast.success("API token copied");
    } catch {
      toast.error("Could not copy the API token", {
        description: "Select the token and copy it manually.",
      });
    }
  }

  function setTokenRevealOpen(open: boolean) {
    tokenRevealOpen = open;
    if (!open) account.createdToken = null;
  }

  function requestDeletion(target: DeletionTarget) {
    pendingDeletion = target;
    deleteDialogOpen = true;
  }

  async function confirmDeletion() {
    const target = pendingDeletion;
    if (!target) return;

    if (target.kind === "passkey") {
      await account.removePasskey(target.id);
    } else if (target.kind === "ssh-key") {
      await account.removeSshKey(target.id);
    } else {
      await account.revokeToken(target.id);
    }
    if (!account.error) {
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

<div
  class="divide-y divide-border overflow-hidden rounded-xl bg-card/20 ring-1 ring-foreground/15"
>
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
        void account.updateUsername();
      }}
    >
      <Field.Field>
        <Field.Label for="account-username">Username</Field.Label>
        <Input
          id="account-username"
          autocomplete="username"
          bind:value={account.username}
          maxlength={39}
          required
        />
        <Field.Description>
          Repository URLs change with your username. Update existing Git remotes
          afterward.
        </Field.Description>
      </Field.Field>
      <Field.Field>
        <Field.Label for="username-current-password">
          Current password
        </Field.Label>
        <Input
          id="username-current-password"
          type="password"
          autocomplete="current-password"
          bind:value={account.usernamePassword}
          required
        />
      </Field.Field>
      <Button class="w-fit" type="submit" disabled={account.working}>
        Update username
      </Button>
    </form>
  </section>

  <section
    class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
    aria-labelledby="password-heading"
  >
    <header class="flex items-start gap-3">
      <LockKeyhole class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div>
        <h2 id="password-heading" class="font-semibold">Password</h2>
        <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
          Use at least 12 characters. Other browser sessions will be signed out.
        </p>
      </div>
    </header>

    <form
      class="grid max-w-2xl gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void account.updatePassword();
      }}
    >
      <Field.Field>
        <Field.Label for="password-current">Current password</Field.Label>
        <Input
          id="password-current"
          type="password"
          autocomplete="current-password"
          bind:value={account.currentPassword}
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
            bind:value={account.newPassword}
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
            bind:value={account.confirmPassword}
            minlength={12}
            required
          />
        </Field.Field>
      </div>
      <Button class="w-fit" type="submit" disabled={account.working}>
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
        {#each account.passkeys as passkey (passkey.id)}
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
          void account.addPasskey();
        }}
      >
        <Field.Field>
          <Field.Label for="passkey-name">Passkey name</Field.Label>
          <Input
            id="passkey-name"
            bind:value={account.passkeyName}
            disabled={!passkeysAvailable}
            required
          />
        </Field.Field>
        <Button
          class="w-fit"
          type="submit"
          disabled={account.working || !passkeysAvailable}
        >
          Add passkey
        </Button>
      </form>
    </div>
  </section>

  <section
    class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
    aria-labelledby="ssh-keys-heading"
  >
    <header class="flex items-start gap-3">
      <Terminal class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div>
        <h2 id="ssh-keys-heading" class="font-semibold">SSH keys</h2>
        <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
          Authenticate Git operations over SSH.
        </p>
      </div>
    </header>

    <div class="grid max-w-2xl gap-4">
      <div class="flex items-center justify-between gap-4">
        <p class="text-sm text-muted-foreground">
          {account.sshKeys.length}
          {account.sshKeys.length === 1 ? "key" : "keys"} added
        </p>
        <Button
          size="sm"
          variant="outline"
          onclick={() => (sshKeyDialogOpen = true)}
        >
          Add key
        </Button>
      </div>

      {#if account.sshKeys.length > 0}
        <ul
          class="max-h-72 divide-y overflow-y-auto overscroll-contain rounded-lg border bg-background/20"
        >
          {#each account.sshKeys as key (key.id)}
            <li class="flex items-center justify-between gap-4 p-3">
              <div class="min-w-0">
                <p class="truncate font-medium">{key.name}</p>
                <code class="mt-1 block truncate text-xs text-muted-foreground">
                  {key.fingerprint}
                </code>
              </div>
              <Button
                class="shrink-0"
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
            </li>
          {/each}
        </ul>
      {:else}
        <p
          class="rounded-lg border border-dashed px-4 py-6 text-center text-sm text-muted-foreground"
        >
          No SSH keys added.
        </p>
      {/if}
    </div>

    <Dialog.Root bind:open={sshKeyDialogOpen}>
      <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
        <Dialog.Header>
          <Dialog.Title>Add an SSH key</Dialog.Title>
          <Dialog.Description>
            Add a public key to authenticate Git operations over SSH.
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
              bind:value={account.sshKeyName}
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
              value={account.sshPublicKey}
              oninput={(event) => updateSshPublicKey(event.currentTarget.value)}
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
            <Button type="submit" disabled={account.working}>
              {account.working ? "Adding…" : "Add SSH key"}
            </Button>
          </Dialog.Footer>
        </form>
      </Dialog.Content>
    </Dialog.Root>
  </section>

  <section
    class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
    aria-labelledby="api-tokens-heading"
  >
    <header class="flex items-start gap-3">
      <KeySquare class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div>
        <h2 id="api-tokens-heading" class="font-semibold">API tokens</h2>
        <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
          Create scoped credentials for scripts and integrations.
        </p>
      </div>
    </header>

    <div class="grid max-w-2xl gap-4">
      <div class="flex items-center justify-between gap-4">
        <p class="text-sm text-muted-foreground">
          {account.tokens.length}
          active {account.tokens.length === 1 ? "token" : "tokens"}
        </p>
        <Button
          size="sm"
          variant="outline"
          onclick={() => (apiTokenDialogOpen = true)}
        >
          Create token
        </Button>
      </div>

      {#if account.tokens.length > 0}
        <ul
          class="max-h-72 divide-y overflow-y-auto overscroll-contain rounded-lg border bg-background/20"
        >
          {#each account.tokens as token (token.id)}
            <li class="flex items-center justify-between gap-4 p-3">
              <div class="min-w-0">
                <p class="truncate font-medium">{token.name}</p>
                <p class="mt-1 truncate text-xs text-muted-foreground">
                  {token.scopes.join(", ")} · {tokenExpiry(token.expires_at)}
                </p>
              </div>
              <Button
                class="shrink-0"
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
            </li>
          {/each}
        </ul>
      {:else}
        <p
          class="rounded-lg border border-dashed px-4 py-6 text-center text-sm text-muted-foreground"
        >
          No active API tokens.
        </p>
      {/if}
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
              bind:value={account.tokenName}
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
                        {account.tokenExpiresOn
                          ? account.tokenExpiresOn
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
                    bind:value={account.tokenExpiresOn}
                    minValue={today(getLocalTimeZone()).add({ days: 1 })}
                    maxValue={today(getLocalTimeZone()).add({ days: 3650 })}
                    captionLayout="dropdown"
                    initialFocus
                    onValueChange={() => (tokenExpiryOpen = false)}
                  />
                </Popover.Content>
              </Popover.Root>
              {#if account.tokenExpiresOn}
                <Button
                  type="button"
                  variant="outline"
                  aria-label="Clear expiration date"
                  onclick={() => (account.tokenExpiresOn = undefined)}
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
                class={account.tokenRead
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
                  bind:checked={account.tokenRead}
                />
              </div>
              <div
                class={account.tokenWrite
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
                  bind:checked={account.tokenWrite}
                />
              </div>
              <div
                class={account.tokenSshKeys
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
                  bind:checked={account.tokenSshKeys}
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
            <Button type="submit" disabled={account.working}>
              {account.working ? "Creating…" : "Create token"}
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
            {account.createdToken}
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
          disabled={account.working}
          onclick={() => void confirmDeletion()}
        >
          {pendingDeletion?.kind === "api-token" ? "Revoke token" : "Remove"}
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>
</div>
