<script lang="ts">
  import KeyRound from "@lucide/svelte/icons/key-round";
  import LockKeyhole from "@lucide/svelte/icons/lock-keyhole";
  import { onMount } from "svelte";

  import CredentialRemovalDialog, {
    type CredentialRemoval,
  } from "$lib/components/settings/credential-removal-dialog.svelte";
  import TwoFactorSettings from "$lib/components/settings/two-factor-settings.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import type { CredentialsSettingsState } from "$lib/settings/account/credentials-settings-state.svelte.js";
  import type { PasswordSettingsState } from "$lib/settings/account/profile-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  let {
    password,
    credentials,
  }: {
    password: PasswordSettingsState;
    credentials: CredentialsSettingsState;
  } = $props();
  const app = useAppState();
  let browserSupportsPasskeys = $state(true);
  const passkeysEnabled = $derived(
    app.authStatus?.authentication.passkey_enabled ?? true,
  );
  const passkeysAvailable = $derived(
    browserSupportsPasskeys && passkeysEnabled,
  );
  let deleteDialogOpen = $state(false);
  let pendingDeletion = $state<CredentialRemoval | null>(null);

  onMount(() => {
    browserSupportsPasskeys =
      globalThis.isSecureContext &&
      typeof navigator.credentials?.create === "function" &&
      typeof PublicKeyCredential !== "undefined";
  });

  function requestDeletion(target: CredentialRemoval) {
    pendingDeletion = target;
    deleteDialogOpen = true;
  }

  async function confirmDeletion() {
    const target = pendingDeletion;
    if (!target) return;
    if (await credentials.removePasskey(target.id)) {
      deleteDialogOpen = false;
      pendingDeletion = null;
    }
  }
</script>

<section
  class="grid gap-5 py-(--card-spacing) md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10"
  aria-labelledby="password-heading"
>
  <Card.Header class="flex flex-row items-start gap-3">
    <LockKeyhole class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
    <div>
      <Card.Title id="password-heading" role="heading" aria-level={2}>
        Password
      </Card.Title>
      <Card.Description class="mt-1 max-w-xs leading-5">
        Use at least 12 characters. Other browser sessions will be signed out.
      </Card.Description>
    </div>
  </Card.Header>

  <Card.Content>
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
  </Card.Content>
</section>

<section
  class="grid gap-5 py-(--card-spacing) md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10"
  aria-labelledby="passkeys-heading"
>
  <Card.Header class="flex flex-row items-start gap-3">
    <KeyRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
    <div>
      <Card.Title id="passkeys-heading" role="heading" aria-level={2}>
        Passkeys
      </Card.Title>
      <Card.Description class="mt-1 max-w-xs leading-5">
        Use your device or security key instead of a password.
      </Card.Description>
    </div>
  </Card.Header>

  <Card.Content class="grid max-w-2xl gap-5">
    <ul class="motion-list grid gap-2">
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

    {#if !passkeysEnabled}
      <p
        class="rounded-lg border bg-muted/30 p-3 text-xs text-muted-foreground"
      >
        Passkey registration is disabled by the administrator.
      </p>
    {:else if !browserSupportsPasskeys}
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
  </Card.Content>
</section>

<TwoFactorSettings />

<CredentialRemovalDialog
  bind:open={deleteDialogOpen}
  target={pendingDeletion}
  working={credentials.working}
  onconfirm={() => void confirmDeletion()}
/>
