<script lang="ts">
  import AccountApiTokenSettings from "$lib/components/settings/account-api-token-settings.svelte";
  import AccountAuthenticationSettings from "$lib/components/settings/account-authentication-settings.svelte";
  import AccountProfileSettings from "$lib/components/settings/account-profile-settings.svelte";
  import AccountSshKeySettings from "$lib/components/settings/account-ssh-key-settings.svelte";
  import * as Card from "$lib/components/ui/card/index.js";
  import type { CredentialsSettingsState } from "$lib/settings/account/credentials-settings-state.svelte.js";
  import type {
    PasswordSettingsState,
    ProfileSettingsState,
  } from "$lib/settings/account/profile-settings-state.svelte.js";

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
</script>

<Card.Root
  class={view === "ssh-keys" || view === "api-tokens"
    ? "contents"
    : "gap-0 py-0 [--card-spacing:--spacing(5)] divide-y divide-border md:[--card-spacing:--spacing(6)]"}
>
  {#if view === "account"}
    <AccountProfileSettings {profile} />
  {:else if view === "authentication"}
    <AccountAuthenticationSettings {password} {credentials} />
  {:else if view === "ssh-keys"}
    <AccountSshKeySettings {credentials} />
  {:else}
    <AccountApiTokenSettings {credentials} />
  {/if}
</Card.Root>
