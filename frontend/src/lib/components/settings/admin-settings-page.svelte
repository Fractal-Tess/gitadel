<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import ContextNav, {
    type ContextNavItem,
  } from "$lib/components/app/context-nav.svelte";
  import InstanceSettings from "$lib/components/settings/instance-settings.svelte";
  import ActionsSettings from "$lib/components/settings/actions-settings.svelte";
  import BackupSettings from "$lib/components/settings/backup-settings.svelte";
  import LfsSettings from "$lib/components/settings/lfs-settings.svelte";
  import RegistrySettings from "$lib/components/settings/registry-settings.svelte";
  import IntegritySettings from "$lib/components/settings/integrity-settings.svelte";
  import StorageSettings from "$lib/components/settings/storage-settings.svelte";
  import { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";
  import { systemRunnerScope } from "$lib/settings/account/actions-settings-state.svelte.js";
  import {
    adminSettingsSections,
    type AdminSettingsView,
  } from "$lib/settings/navigation.js";
  import { preloadAdminSettingsView } from "$lib/settings/settings-data-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const accountState = new AccountSettingsState(app);
  const actionsState = accountState.actions;
  const requestedView = $derived(page.params.view ?? "appearance");
  const view = $derived<AdminSettingsView>(
    requestedView === "access" ||
      requestedView === "runners" ||
      requestedView === "storage" ||
      requestedView === "lfs" ||
      requestedView === "registry" ||
      requestedView === "backups" ||
      requestedView === "maintenance" ||
      requestedView === "activity"
      ? requestedView
      : "appearance",
  );
  const title = $derived(
    view === "appearance"
      ? "Appearance"
      : view === "access"
        ? "Access"
        : view === "runners"
          ? "Runners"
          : view === "storage"
            ? "Storage"
            : view === "lfs"
              ? "Git LFS"
              : view === "registry"
                ? "Container registry"
                : view === "backups"
                  ? "Backups"
                  : view === "maintenance"
                    ? "Maintenance"
                    : "Activity",
  );
  const description = $derived(
    view === "appearance"
      ? "Manage instance identity and repository defaults."
      : view === "access"
        ? "Control account access, login methods, and identity providers."
        : view === "runners"
          ? "Provide runners that can execute workflows for every repository."
          : view === "storage"
            ? "Define reusable filesystem and S3-compatible destinations."
            : view === "lfs"
              ? "Inspect Git LFS usage and move objects between storage targets."
              : view === "registry"
                ? "Inspect image usage and move container registry data between storage targets."
                : view === "backups"
                  ? "Add backup providers, then create and schedule complete instance snapshots."
                  : view === "maintenance"
                    ? "Configure recurring repository integrity checks and review the latest result."
                    : "Review repository, authentication, and administration events.",
  );

  const navigation = $derived.by<ContextNavItem[]>(() =>
    adminSettingsSections.map((section) => ({
      id: section.id,
      label: section.label,
      icon: section.icon,
      href: resolve("/-/administration/[view]", { view: section.id }),
      active: view === section.id,
      preload: () =>
        preloadAdminSettingsView(app.authorizationScope, section.id),
    })),
  );

  $effect(() => {
    accountState.syncScope();
  });
</script>

<svelte:head>
  <title
    >{title} · Administration · {app.instance?.site_name ?? "Gitadel"}</title
  >
</svelte:head>

<ContextNav label="Administration" items={navigation} />

<div class="mx-auto max-w-5xl px-5 py-8 lg:px-8">
  <header class="mb-6">
    <p
      class="text-xs font-medium uppercase tracking-wider text-muted-foreground"
    >
      Administration
    </p>
    <h1 class="mt-1 text-xl font-semibold tracking-tight">{title}</h1>
    <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
      {description}
    </p>
  </header>
  {#if view === "runners"}
    <ActionsSettings state={actionsState} scope={systemRunnerScope} />
  {:else if view === "storage"}
    <StorageSettings />
  {:else if view === "lfs"}
    <LfsSettings />
  {:else if view === "registry"}
    <RegistrySettings />
  {:else if view === "backups"}
    <BackupSettings />
  {:else if view === "maintenance"}
    <IntegritySettings />
  {:else}
    <InstanceSettings view={view === "appearance" ? "general" : view} />
  {/if}
</div>
