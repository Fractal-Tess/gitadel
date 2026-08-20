<script lang="ts">
  import { onMount } from "svelte";
  import { resolve } from "$app/paths";
  import { Building2, KeyRound, ShieldCheck } from "lucide-svelte";

  import OrganizationSettings from "$lib/components/settings/organization-settings.svelte";
  import SecuritySettings from "$lib/components/settings/security-settings.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const state = new AccountSettingsState(app);

  onMount(() => {
    void state.initialize();
  });
</script>

<svelte:head>
  <title>Account settings · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<div class="min-h-screen bg-background">
  <header class="border-b bg-background/95">
    <div class="mx-auto flex min-h-16 max-w-6xl flex-wrap items-center justify-between gap-4 px-5 py-3">
      <a class="text-sm font-bold tracking-[-0.035em]" href={resolve("/")}>
        {app.instance?.site_name ?? "GITADEL"}
      </a>
      <div class="flex items-center gap-3">
        <span class="text-sm text-muted-foreground">{app.authStatus?.user?.username}</span>
        <Button
          variant="outline"
          size="sm"
          onclick={() => void state.logout()}
          disabled={state.working}>Sign out</Button
        >
      </div>
    </div>
  </header>

  <main class="mx-auto max-w-6xl px-5 py-10">
    <div class="mb-8">
      <p class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
        Personal
      </p>
      <h1 class="mt-2 text-2xl font-semibold">Account settings</h1>
    </div>

    <nav class="mb-6 flex flex-wrap gap-2" aria-label="Settings sections">
      <Button
        class="gap-2"
        variant={state.view === "security" ? "default" : "outline"}
        onclick={() => (state.view = "security")}
      >
        <KeyRound class="size-4" />Security
      </Button>
      <Button
        class="gap-2"
        variant={state.view === "organizations" ? "default" : "outline"}
        onclick={() => (state.view = "organizations")}
      >
        <Building2 class="size-4" />Organizations
      </Button>
      {#if app.authStatus?.user?.is_admin}
        <Button class="gap-2" variant="outline" href={resolve("/admin")}>
          <ShieldCheck class="size-4" />Administration
        </Button>
      {/if}
    </nav>

    {#if state.error}
      <p class="mb-5 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
        {state.error}
      </p>
    {/if}
    {#if state.notice}
      <p class="mb-5 rounded-md border bg-muted p-3 text-sm">{state.notice}</p>
    {/if}

    {#if state.loading}
      <div class="py-16 text-center text-sm text-muted-foreground">
        Loading account settings…
      </div>
    {:else if state.view === "security"}
      <SecuritySettings {state} />
    {:else}
      <OrganizationSettings {state} />
    {/if}
  </main>
</div>
