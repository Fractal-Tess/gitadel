<script lang="ts">
  import { KeyRound, KeySquare, Terminal } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import type { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";

  let { state }: { state: AccountSettingsState } = $props();
  const inputClass =
    "w-full rounded-md border bg-background px-3 py-2 text-sm outline-none focus:border-ring focus:ring-2 focus:ring-ring/20";
</script>

<div class="grid gap-6 lg:grid-cols-2">
  <section class="rounded-md border bg-card/25">
    <header class="flex items-center gap-3 border-b px-5 py-4">
      <KeyRound class="size-4 text-muted-foreground" />
      <div>
        <h2 class="text-sm font-semibold">Passkeys</h2>
        <p class="mt-0.5 text-xs text-muted-foreground">
          Use your device or security key instead of a password.
        </p>
      </div>
    </header>
    <div class="p-5">
      <ul class="grid gap-2">
        {#each state.passkeys as passkey (passkey.id)}
          <li class="flex items-center justify-between rounded-md border p-3">
            <span class="text-sm">{passkey.name}</span>
            <Button
              size="sm"
              variant="ghost"
              onclick={() => void state.removePasskey(passkey.id)}>Remove</Button
            >
          </li>
        {:else}
          <li class="text-sm text-muted-foreground">No passkeys yet.</li>
        {/each}
      </ul>
      <form
        class="mt-5 flex gap-2"
        onsubmit={(event) => {
          event.preventDefault();
          void state.addPasskey();
        }}
      >
        <input class={inputClass} bind:value={state.passkeyName} required />
        <Button type="submit" disabled={state.working}>Add passkey</Button>
      </form>
    </div>
  </section>

  <section class="rounded-md border bg-card/25">
    <header class="flex items-center gap-3 border-b px-5 py-4">
      <Terminal class="size-4 text-muted-foreground" />
      <div>
        <h2 class="text-sm font-semibold">SSH keys</h2>
        <p class="mt-0.5 text-xs text-muted-foreground">
          Authenticate Git operations over SSH.
        </p>
      </div>
    </header>
    <div class="p-5">
      <ul class="grid gap-2">
        {#each state.sshKeys as key (key.id)}
          <li class="rounded-md border p-3">
            <div class="flex justify-between gap-3">
              <span class="text-sm font-medium">{key.name}</span>
              <Button
                size="sm"
                variant="ghost"
                onclick={() => void state.removeSshKey(key.id)}>Remove</Button
              >
            </div>
            <code class="mt-1 block break-all text-xs text-muted-foreground">
              {key.fingerprint}
            </code>
          </li>
        {:else}
          <li class="text-sm text-muted-foreground">No SSH keys yet.</li>
        {/each}
      </ul>
      <form
        class="mt-5 grid gap-3"
        onsubmit={(event) => {
          event.preventDefault();
          void state.addSshKey();
        }}
      >
        <label class="grid gap-1.5 text-sm font-medium">
          Name
          <input class={inputClass} bind:value={state.sshKeyName} required />
        </label>
        <label class="grid gap-1.5 text-sm font-medium">
          Public key
          <textarea
            class={inputClass}
            rows="3"
            bind:value={state.sshPublicKey}
            required
          ></textarea>
        </label>
        <Button type="submit" disabled={state.working}>Add SSH key</Button>
      </form>
    </div>
  </section>

  <section class="rounded-md border bg-card/25 lg:col-span-2">
    <header class="flex items-center gap-3 border-b px-5 py-4">
      <KeySquare class="size-4 text-muted-foreground" />
      <div>
        <h2 class="text-sm font-semibold">API tokens</h2>
        <p class="mt-0.5 text-xs text-muted-foreground">
          Tokens are shown once. Store them somewhere safe.
        </p>
      </div>
    </header>
    <div class="p-5">
      {#if state.createdToken}
        <div class="mb-4 rounded-md border bg-muted p-3">
          <p class="text-xs font-medium">New token</p>
          <code class="mt-1 block break-all text-sm">{state.createdToken}</code>
        </div>
      {/if}
      <ul class="grid gap-2">
        {#each state.tokens as token (token.id)}
          <li class="flex flex-wrap items-center justify-between gap-2 rounded-md border p-3">
            <div>
              <p class="text-sm font-medium">{token.name}</p>
              <p class="text-xs text-muted-foreground">{token.scopes.join(", ")}</p>
            </div>
            <Button
              size="sm"
              variant="ghost"
              onclick={() => void state.revokeToken(token.id)}>Revoke</Button
            >
          </li>
        {:else}
          <li class="text-sm text-muted-foreground">No active tokens.</li>
        {/each}
      </ul>
      <form
        class="mt-5 grid gap-4 md:grid-cols-2"
        onsubmit={(event) => {
          event.preventDefault();
          void state.createApiToken();
        }}
      >
        <label class="grid gap-1.5 text-sm font-medium">
          Name
          <input class={inputClass} bind:value={state.tokenName} required />
        </label>
        <label class="grid gap-1.5 text-sm font-medium">
          Expires in days
          <input
            class={inputClass}
            type="number"
            min="1"
            max="3650"
            bind:value={state.tokenExpiryDays}
            placeholder="Never"
          />
        </label>
        <fieldset class="flex flex-wrap gap-4 text-sm">
          <legend class="mb-2 font-medium">Scopes</legend>
          <label><input type="checkbox" bind:checked={state.tokenRead} /> Read</label>
          <label><input type="checkbox" bind:checked={state.tokenWrite} /> Write</label>
          <label><input type="checkbox" bind:checked={state.tokenSshKeys} /> SSH keys</label>
        </fieldset>
        <Button type="submit" disabled={state.working}>Create token</Button>
      </form>
    </div>
  </section>
</div>
