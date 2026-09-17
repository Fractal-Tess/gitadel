<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import ContextNav, {
    type ContextNavItem,
  } from "$lib/components/app/context-nav.svelte";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";

  import OauthApplicationSettings from "$lib/components/settings/oauth-application-settings.svelte";
  import SecuritySettings from "$lib/components/settings/security-settings.svelte";
  import { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";
  import {
    accountSettingsSections,
    type AccountSettingsView,
  } from "$lib/settings/navigation.js";
  import { preloadAccountSettingsView } from "$lib/settings/settings-data-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const state = new AccountSettingsState(app);
  const requestedView = $derived(page.params.view ?? "profile");
  const view = $derived<AccountSettingsView>(
    requestedView === "authentication" ||
      requestedView === "ssh-keys" ||
      requestedView === "api-tokens" ||
      requestedView === "oauth-applications"
      ? requestedView
      : "profile",
  );
  const title = $derived(
    view === "profile"
      ? "Profile"
      : view === "authentication"
        ? "Authentication"
        : view === "ssh-keys"
          ? "SSH keys"
          : view === "api-tokens"
            ? "API tokens"
            : "OAuth applications",
  );
  const description = $derived(
    view === "profile"
      ? "Manage your identity and repository defaults."
      : view === "authentication"
        ? "Change your password and manage passkeys."
        : view === "ssh-keys"
          ? "Choose which SSH keys can access your repositories."
          : view === "api-tokens"
            ? "Create scoped tokens for scripts and API clients."
            : "Register applications that access Gitadel on your behalf.",
  );

  $effect(() => {
    state.syncScope();
    void state.initialize(view);
  });

  const navigation = $derived.by<ContextNavItem[]>(() =>
    accountSettingsSections.map((section) => ({
      id: section.id,
      label: section.label,
      icon: section.icon,
      href: resolve("/-/account/[view]", { view: section.id }),
      active: view === section.id,
      ...(section.id !== "profile" && {
        preload: () =>
          preloadAccountSettingsView(app.authorizationScope, section.id),
      }),
    })),
  );
</script>

<svelte:head>
  <title>{title} · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<ContextNav label="Account settings" items={navigation} />

<div class="mx-auto max-w-5xl px-5 py-8 lg:px-8">
  <header class="mb-6">
    <p
      class="text-xs font-medium uppercase tracking-wider text-muted-foreground"
    >
      Account settings
    </p>
    <h1 class="mt-1 text-xl font-semibold tracking-tight">{title}</h1>
    <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
      {description}
    </p>
  </header>

  {#if state.loading}
    <p class="py-16 text-center text-sm text-muted-foreground">
      Loading account settings…
    </p>
  {:else if state.error}
    <Alert.Root variant="destructive">
      <Alert.Title>Account settings unavailable</Alert.Title>
      <Alert.Description>{state.error}</Alert.Description>
      <Button
        class="mt-3"
        size="sm"
        variant="outline"
        onclick={() => void state.initialize(view)}
      >
        Retry
      </Button>
    </Alert.Root>
  {:else if view === "oauth-applications"}
    <OauthApplicationSettings state={state.oauth} showHeader={false} />
  {:else}
    <SecuritySettings
      profile={state.profile}
      password={state.password}
      credentials={state.credentials}
      view={view === "profile" ? "account" : view}
    />
  {/if}
</div>
