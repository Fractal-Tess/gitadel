<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { AppWindow, Building2, KeyRound, ShieldCheck } from "lucide-svelte";

  import InstanceSettings from "$lib/components/settings/instance-settings.svelte";
  import OauthApplicationSettings from "$lib/components/settings/oauth-application-settings.svelte";
  import OrganizationSettings from "$lib/components/settings/organization-settings.svelte";
  import SecuritySettings from "$lib/components/settings/security-settings.svelte";
  import { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  const app = useAppState();
  const shell = useShellState();
  const state = new AccountSettingsState(app);

  onMount(() => {
    const requestedView = page.url.searchParams.get("view");
    if (requestedView === "applications") {
      state.view = "applications";
    } else if (requestedView === "organizations") {
      state.view = "organizations";
    } else if (
      requestedView === "administration" &&
      app.authStatus?.user?.is_admin
    ) {
      state.view = "administration";
    }
    void state.initialize();
  });

  $effect(() =>
    shell.publishNavGroup({
      label: "Account",
      items: [
        {
          id: "security",
          label: "Security",
          icon: KeyRound,
          active: state.view === "security",
          select: () => (state.view = "security"),
        },
        {
          id: "applications",
          label: "Applications",
          icon: AppWindow,
          active: state.view === "applications",
          select: () => (state.view = "applications"),
        },
        {
          id: "organizations",
          label: "Organizations",
          icon: Building2,
          active: state.view === "organizations",
          select: () => (state.view = "organizations"),
        },
        ...(app.authStatus?.user?.is_admin
          ? [
              {
                id: "administration",
                label: "Administration",
                icon: ShieldCheck,
                active: state.view === "administration",
                select: () => (state.view = "administration"),
              },
            ]
          : []),
      ],
    }),
  );
</script>

<svelte:head>
  <title>Account settings · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<div class="mx-auto max-w-5xl px-5 py-8 lg:px-8">
  <div class="mb-7">
    <h1 class="text-lg font-semibold tracking-tight">Account settings</h1>
    <p class="mt-1.5 text-sm text-muted-foreground">
      Manage your identity, authentication methods, and access credentials.
    </p>
  </div>

  {#if state.loading}
    <div class="py-16 text-center text-sm text-muted-foreground">
      Loading account settings…
    </div>
  {:else if state.view === "security"}
    <SecuritySettings {state} />
  {:else if state.view === "applications"}
    <OauthApplicationSettings {state} />
  {:else if state.view === "organizations"}
    <OrganizationSettings {state} />
  {:else}
    <InstanceSettings />
  {/if}
</div>
