<script lang="ts">
  import { onMount } from "svelte";
  import { resolve } from "$app/paths";
  import { page } from "$app/state";
  import { AppWindow, Building2, KeyRound, ShieldCheck } from "lucide-svelte";

  import BrandMark from "$lib/components/brand-mark.svelte";
  import OauthApplicationSettings from "$lib/components/settings/oauth-application-settings.svelte";
  import OrganizationSettings from "$lib/components/settings/organization-settings.svelte";
  import SecuritySettings from "$lib/components/settings/security-settings.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { preloadExplore } from "$lib/navigation-cache.js";
  import { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const state = new AccountSettingsState(app);

  function tabClass(active: boolean) {
    return `-mb-px h-auto rounded-none border-x-0 border-t-0 border-b-2 px-1 pb-3 pt-1 hover:bg-transparent ${
      active
        ? "border-foreground text-foreground"
        : "border-transparent text-muted-foreground hover:text-foreground"
    }`;
  }

  onMount(() => {
    if (page.url.searchParams.get("view") === "applications") {
      state.view = "applications";
    }
    void state.initialize();
  });
</script>

<svelte:head>
  <title>Account settings · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<div class="min-h-screen bg-background">
  <header class="border-b bg-background/95">
    <div
      class="mx-auto flex min-h-16 max-w-5xl flex-wrap items-center justify-between gap-4 px-5 py-3"
    >
      <a
        class="flex items-center gap-2 text-sm font-bold tracking-[-0.035em]"
        href={resolve("/")}
        onpointerenter={() => preloadExplore(app.authStatus?.user?.username)}
        onpointerdown={() => preloadExplore(app.authStatus?.user?.username)}
        onfocus={() => preloadExplore(app.authStatus?.user?.username)}
      >
        <BrandMark />
        {app.instance?.site_name ?? "GITADEL"}
      </a>
      <div class="flex items-center gap-3">
        <span class="text-sm text-muted-foreground"
          >{app.authStatus?.user?.username}</span
        >
        <Button
          variant="outline"
          size="sm"
          onclick={() => void state.logout()}
          disabled={state.working}>Sign out</Button
        >
      </div>
    </div>
  </header>

  <main class="mx-auto max-w-5xl px-5 py-10">
    <div
      class="mb-7 flex flex-col items-start justify-between gap-4 sm:flex-row"
    >
      <div>
        <h1 class="text-2xl font-semibold tracking-tight">Account settings</h1>
        <p class="mt-1.5 text-sm text-muted-foreground">
          Manage your identity, authentication methods, and access credentials.
        </p>
      </div>
      {#if app.authStatus?.user?.is_admin}
        <Button
          class="shrink-0 gap-2"
          size="sm"
          variant="outline"
          href={resolve("/admin")}
        >
          <ShieldCheck class="size-4" />Administration
        </Button>
      {/if}
    </div>

    <nav
      class="mb-6 flex items-end gap-4 border-b sm:gap-5"
      aria-label="Settings sections"
    >
      <Button
        class={`${tabClass(state.view === "security")} gap-2`}
        variant="ghost"
        onclick={() => (state.view = "security")}
      >
        <KeyRound class="hidden size-4 sm:block" />Security
      </Button>
      <Button
        class={`${tabClass(state.view === "applications")} gap-2`}
        variant="ghost"
        onclick={() => (state.view = "applications")}
      >
        <AppWindow class="hidden size-4 sm:block" />Applications
      </Button>
      <Button
        class={`${tabClass(state.view === "organizations")} gap-2`}
        variant="ghost"
        onclick={() => (state.view = "organizations")}
      >
        <Building2 class="hidden size-4 sm:block" />Organizations
      </Button>
    </nav>

    {#if state.loading}
      <div class="py-16 text-center text-sm text-muted-foreground">
        Loading account settings…
      </div>
    {:else if state.view === "security"}
      <SecuritySettings {state} />
    {:else if state.view === "applications"}
      <OauthApplicationSettings {state} />
    {:else}
      <OrganizationSettings {state} />
    {/if}
  </main>
</div>
