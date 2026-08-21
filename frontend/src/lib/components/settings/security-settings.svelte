<script lang="ts">
  import { getLocalTimeZone, today } from "@internationalized/date";
  import {
    CalendarDays,
    ChevronDown,
    KeyRound,
    KeySquare,
    Terminal,
  } from "lucide-svelte";
  import { onMount } from "svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import { Calendar } from "$lib/components/ui/calendar/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Popover from "$lib/components/ui/popover/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import type { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";

  let { state: account }: { state: AccountSettingsState } = $props();
  let passkeysAvailable = $state(true);
  let tokenExpiryOpen = $state(false);

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

  function maskKeyIdentifier(value: string): string {
    return value.length > 22
      ? `${value.slice(0, 12)}…${value.slice(-8)}`
      : value;
  }

  function tokenExpiry(expiresAt: string | null): string {
    return expiresAt
      ? `Expires ${new Date(expiresAt).toLocaleDateString()}`
      : "Never expires";
  }
</script>

<div class="grid gap-4 lg:grid-cols-3">
  <Card.Root class="ring-foreground/20">
    <Card.Header class="border-b">
      <div class="flex items-start gap-3">
        <KeyRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <Card.Title>Passkeys</Card.Title>
          <Card.Description
            >Use your device or security key instead of a password.</Card.Description
          >
        </div>
      </div>
    </Card.Header>
    <Card.Content class="flex flex-1 flex-col gap-5">
      <ul class="grid gap-2">
        {#each account.passkeys as passkey (passkey.id)}
          <li
            class="flex items-center justify-between gap-3 rounded-lg border bg-background/30 p-3"
          >
            <span class="min-w-0 truncate font-medium">{passkey.name}</span>
            <Button
              size="sm"
              variant="outline"
              onclick={() => void account.removePasskey(passkey.id)}
              >Remove</Button
            >
          </li>
        {:else}
          <li class="text-sm text-muted-foreground">No passkeys yet.</li>
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
        class="mt-auto grid gap-4 border-t pt-4"
        onsubmit={(event) => {
          event.preventDefault();
          void account.addPasskey();
        }}
      >
        <Field.Field>
          <Field.Label for="passkey-name">Name</Field.Label>
          <Input
            id="passkey-name"
            bind:value={account.passkeyName}
            disabled={!passkeysAvailable}
            required
          />
        </Field.Field>
        <Button type="submit" disabled={account.working || !passkeysAvailable}
          >Add passkey</Button
        >
      </form>
    </Card.Content>
  </Card.Root>

  <Card.Root class="ring-foreground/20">
    <Card.Header class="border-b">
      <div class="flex items-start gap-3">
        <Terminal class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <Card.Title>SSH keys</Card.Title>
          <Card.Description
            >Authenticate Git operations over SSH.</Card.Description
          >
        </div>
      </div>
    </Card.Header>
    <Card.Content class="flex flex-1 flex-col gap-5">
      <ul class="grid gap-2">
        {#each account.sshKeys as key (key.id)}
          <li class="rounded-lg border bg-background/30 p-3">
            <div class="flex items-center justify-between gap-3">
              <span class="min-w-0 truncate font-medium">{key.name}</span>
              <Button
                size="sm"
                variant="outline"
                onclick={() => void account.removeSshKey(key.id)}>Remove</Button
              >
            </div>
            <code class="mt-2 block text-xs text-muted-foreground">
              {maskKeyIdentifier(key.public_key)}
            </code>
          </li>
        {:else}
          <li class="text-sm text-muted-foreground">No SSH keys yet.</li>
        {/each}
      </ul>

      <form
        class="mt-auto grid gap-4 border-t pt-4"
        onsubmit={(event) => {
          event.preventDefault();
          void account.addSshKey();
        }}
      >
        <Field.Field>
          <Field.Label for="ssh-key-name">Name</Field.Label>
          <Input id="ssh-key-name" bind:value={account.sshKeyName} required />
        </Field.Field>
        <Field.Field>
          <Field.Label for="ssh-public-key">Public key</Field.Label>
          <Textarea
            id="ssh-public-key"
            rows={3}
            value={account.sshPublicKey}
            oninput={(event) => updateSshPublicKey(event.currentTarget.value)}
            required
          />
        </Field.Field>
        <Button type="submit" disabled={account.working}>Add SSH key</Button>
      </form>
    </Card.Content>
  </Card.Root>

  <Card.Root class="ring-foreground/20">
    <Card.Header class="border-b">
      <div class="flex items-start gap-3">
        <KeySquare class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <Card.Title>API tokens</Card.Title>
          <Card.Description
            >Tokens are shown once. Store them somewhere safe.</Card.Description
          >
        </div>
      </div>
    </Card.Header>
    <Card.Content class="flex flex-1 flex-col gap-5">
      {#if account.createdToken}
        <div class="rounded-lg border bg-muted p-3">
          <p class="text-xs font-medium">New token</p>
          <code class="mt-1 block break-all text-sm"
            >{account.createdToken}</code
          >
        </div>
      {/if}

      <ul class="grid gap-2">
        {#each account.tokens as token (token.id)}
          <li
            class="flex items-center justify-between gap-3 rounded-lg border bg-background/30 p-3"
          >
            <div class="min-w-0">
              <p class="truncate font-medium">{token.name}</p>
              <p class="text-xs text-muted-foreground">
                {token.scopes.join(", ")}
              </p>
              <p class="text-xs text-muted-foreground">
                {tokenExpiry(token.expires_at)}
              </p>
            </div>
            <Button
              size="sm"
              variant="outline"
              onclick={() => void account.revokeToken(token.id)}>Revoke</Button
            >
          </li>
        {:else}
          <li class="text-sm text-muted-foreground">No active tokens.</li>
        {/each}
      </ul>

      <form
        class="mt-auto grid gap-4 border-t pt-4"
        onsubmit={(event) => {
          event.preventDefault();
          void account.createApiToken();
        }}
      >
        <Field.Field>
          <Field.Label for="token-name">Name</Field.Label>
          <Input id="token-name" bind:value={account.tokenName} required />
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
              <Popover.Content class="w-auto overflow-hidden p-0" align="start">
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

        <Field.Set>
          <Field.Legend variant="label">Scopes</Field.Legend>
          <Field.Group class="grid grid-cols-3 gap-2">
            <Field.Label
              class="rounded-lg border bg-background/30 p-2.5 font-normal"
            >
              <Checkbox bind:checked={account.tokenRead} />
              <span>Read</span>
            </Field.Label>
            <Field.Label
              class="rounded-lg border bg-background/30 p-2.5 font-normal"
            >
              <Checkbox bind:checked={account.tokenWrite} />
              <span>Write</span>
            </Field.Label>
            <Field.Label
              class="rounded-lg border bg-background/30 p-2.5 font-normal"
            >
              <Checkbox bind:checked={account.tokenSshKeys} />
              <span>SSH keys</span>
            </Field.Label>
          </Field.Group>
        </Field.Set>

        <Button type="submit" disabled={account.working}>Create token</Button>
      </form>
    </Card.Content>
  </Card.Root>
</div>
