<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import AppWindow from "@lucide/svelte/icons/app-window";
  import KeySquare from "@lucide/svelte/icons/key-square";
  import LockKeyhole from "@lucide/svelte/icons/lock-keyhole";
  import Terminal from "@lucide/svelte/icons/terminal";
  import UserRound from "@lucide/svelte/icons/user-round";
  import ContextNav, {
    type ContextNavItem,
  } from "$lib/components/app/context-nav.svelte";

  import OauthApplicationSettings from "$lib/components/settings/oauth-application-settings.svelte";
  import SecuritySettings from "$lib/components/settings/security-settings.svelte";
  import { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  type AccountRouteView =
    | "profile"
    | "authentication"
    | "ssh-keys"
    | "api-tokens"
    | "oauth-applications";

  const app = useAppState();
  const state = new AccountSettingsState(app);
  const requestedView = $derived(page.params.view ?? "profile");
  const view = $derived<AccountRouteView>(
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
    void state.initialize(
      view === "profile"
        ? "account"
        : view === "oauth-applications"
          ? "applications"
          : view,
    );
  });

  const navigation = $derived.by<ContextNavItem[]>(() => [
    {
      id: "profile",
      label: "Profile",
      icon: UserRound,
      href: resolve("/-/account/[view]", { view: "profile" }),
      active: view === "profile",
    },
    {
      id: "authentication",
      label: "Authentication",
      icon: LockKeyhole,
      href: resolve("/-/account/[view]", { view: "authentication" }),
      active: view === "authentication",
    },
    {
      id: "ssh-keys",
      label: "SSH keys",
      icon: Terminal,
      href: resolve("/-/account/[view]", { view: "ssh-keys" }),
      active: view === "ssh-keys",
    },
    {
      id: "api-tokens",
      label: "API tokens",
      icon: KeySquare,
      href: resolve("/-/account/[view]", { view: "api-tokens" }),
      active: view === "api-tokens",
    },
    {
      id: "oauth-applications",
      label: "OAuth applications",
      icon: AppWindow,
      href: resolve("/-/account/[view]", { view: "oauth-applications" }),
      active: view === "oauth-applications",
    },
  ]);
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
