<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import ContextNav, {
    type ContextNavItem,
  } from "$lib/components/app/context-nav.svelte";
  import InstanceSettings from "$lib/components/settings/instance-settings.svelte";
  import AdminUserSettings from "$lib/components/settings/admin-user-settings.svelte";
  import ActionsSettings from "$lib/components/settings/actions-settings.svelte";
  import BackupSettings from "$lib/components/settings/backup-settings.svelte";
  import EmailSettings from "$lib/components/settings/email-settings.svelte";
  import StorageDomainSettings from "$lib/components/settings/storage-domain-settings.svelte";
  import IntegritySettings from "$lib/components/settings/integrity-settings.svelte";
  import StorageOverview from "$lib/components/settings/storage-overview.svelte";
  import StorageSettings from "$lib/components/settings/storage-settings.svelte";
  import { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";
  import { systemRunnerScope } from "$lib/settings/account/actions-settings-state.svelte.js";
  import {
    adminSettingsGroups,
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
    requestedView === "users" ||
      requestedView === "access" ||
      requestedView === "email" ||
      requestedView === "runners" ||
      requestedView === "storage" ||
      requestedView === "targets" ||
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
      : view === "users"
        ? "Users"
        : view === "access"
        ? "Access"
        : view === "email"
          ? "Email"
          : view === "runners"
          ? "Runners"
          : view === "storage"
            ? "Storage overview"
            : view === "targets"
            ? "Storage targets"
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
      : view === "users"
        ? "Find accounts, disable or re-enable access, and remove accounts that are no longer needed."
        : view === "access"
        ? "Control account access, login methods, and identity providers."
        : view === "email"
          ? "Check outgoing mail used for password resets and notifications."
          : view === "runners"
          ? "Provide runners that can execute workflows for every repository."
          : view === "storage"
            ? "See how full each disk is and what Gitadel keeps where."
            : view === "targets"
            ? "Add, check, and remove the destinations Git LFS and the container registry can use."
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

  function viewItem(id: AdminSettingsView) {
    const section = adminSettingsSections.find((entry) => entry.id === id)!;
    return {
      id,
      // The Storage tab already names the area; its first page is the summary.
      label: id === "storage" ? "Overview" : section.label,
      icon: section.icon,
      href: resolve("/-/administration/[view]", { view: id }),
      active: view === id,
      preload: () => preloadAdminSettingsView(app.authorizationScope, id),
    };
  }

  const navigation = $derived.by<ContextNavItem[]>(() =>
    adminSettingsGroups.map((group) => {
      const items = group.views.map(viewItem);
      return {
        ...items[0],
        id: group.id,
        label: group.label,
        icon: group.icon,
        active: items.some((item) => item.active),
        items: items.length > 1 ? items : undefined,
      };
    }),
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
  {#if view === "users"}
    <AdminUserSettings />
  {:else if view === "email"}
    <EmailSettings />
  {:else if view === "runners"}
    <ActionsSettings state={actionsState} scope={systemRunnerScope} />
  {:else if view === "storage"}
    <StorageOverview />
  {:else if view === "targets"}
    <StorageSettings />
  {:else if view === "lfs"}
    <StorageDomainSettings
      domain="lfs"
      noun="Git LFS"
      objectsLabel="LFS objects"
      migrationNote="Browsing, Git operations, and LFS downloads remain available. LFS uploads continue during copying and wait while the final changes are moved."
    />
  {:else if view === "registry"}
    <StorageDomainSettings
      domain="registry"
      noun="container registry"
      objectsLabel="Blobs and manifests"
      migrationNote="Pulls remain available. Registry pushes wait while the final changes are moved."
    />
  {:else if view === "backups"}
    <BackupSettings />
  {:else if view === "maintenance"}
    <IntegritySettings />
  {:else}
    <InstanceSettings view={view === "appearance" ? "general" : view} />
  {/if}
</div>
