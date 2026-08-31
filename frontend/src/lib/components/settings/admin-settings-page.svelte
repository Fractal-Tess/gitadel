<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import Activity from "@lucide/svelte/icons/activity";
  import ArchiveRestore from "@lucide/svelte/icons/archive-restore";
  import Database from "@lucide/svelte/icons/database";
  import HardDrive from "@lucide/svelte/icons/hard-drive";
  import Palette from "@lucide/svelte/icons/palette";
  import UserPlus from "@lucide/svelte/icons/user-plus";
  import ContextNav, {
    type ContextNavItem,
  } from "$lib/components/app/context-nav.svelte";

  import BackupSettings from "$lib/components/settings/backup-settings.svelte";
  import LfsSettings from "$lib/components/settings/lfs-settings.svelte";
  import InstanceSettings from "$lib/components/settings/instance-settings.svelte";
  import StorageSettings from "$lib/components/settings/storage-settings.svelte";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import {
    preloadAdminSettingsView,
    type AdminSettingsView,
  } from "$lib/settings/settings-data-cache.js";

  const app = useAppState();
  const requestedView = $derived(page.params.view ?? "appearance");
  const view = $derived<AdminSettingsView>(
    requestedView === "access" ||
      requestedView === "storage" ||
      requestedView === "lfs" ||
      requestedView === "backups" ||
      requestedView === "activity"
      ? requestedView
      : "appearance",
  );
  const title = $derived(
    view === "appearance"
      ? "Appearance"
      : view === "access"
        ? "Access"
        : view === "storage"
          ? "Storage"
          : view === "lfs"
            ? "Git LFS"
            : view === "backups"
              ? "Backups"
              : "Activity",
  );
  const description = $derived(
    view === "appearance"
      ? "Manage instance identity and repository defaults."
      : view === "access"
        ? "Control account access, login methods, and identity providers."
        : view === "storage"
          ? "Define reusable filesystem and S3-compatible destinations."
          : view === "lfs"
            ? "Inspect Git LFS usage and move objects between storage targets."
            : view === "backups"
              ? "Add backup providers, then create and schedule complete instance snapshots."
              : "Review repository, authentication, and administration events.",
  );

  const navigation = $derived.by<ContextNavItem[]>(() => [
    {
      id: "appearance",
      label: "Appearance",
      icon: Palette,
      href: resolve("/-/administration/[view]", { view: "appearance" }),
      active: view === "appearance",
      preload: () =>
        preloadAdminSettingsView(app.authorizationScope, "appearance"),
    },
    {
      id: "access",
      label: "Access",
      icon: UserPlus,
      href: resolve("/-/administration/[view]", { view: "access" }),
      active: view === "access",
      preload: () => preloadAdminSettingsView(app.authorizationScope, "access"),
    },
    {
      id: "storage",
      label: "Storage",
      icon: Database,
      href: resolve("/-/administration/[view]", { view: "storage" }),
      active: view === "storage",
      preload: () =>
        preloadAdminSettingsView(app.authorizationScope, "storage"),
    },
    {
      id: "lfs",
      label: "Git LFS",
      icon: HardDrive,
      href: resolve("/-/administration/[view]", { view: "lfs" }),
      active: view === "lfs",
      preload: () => preloadAdminSettingsView(app.authorizationScope, "lfs"),
    },
    {
      id: "backups",
      label: "Backups",
      icon: ArchiveRestore,
      href: resolve("/-/administration/[view]", { view: "backups" }),
      preload: () =>
        preloadAdminSettingsView(app.authorizationScope, "backups"),
      active: view === "backups",
    },
    {
      id: "activity",
      label: "Activity",
      icon: Activity,
      href: resolve("/-/administration/[view]", { view: "activity" }),
      active: view === "activity",
      preload: () =>
        preloadAdminSettingsView(app.authorizationScope, "activity"),
    },
  ]);
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
  {#if view === "storage"}
    <StorageSettings />
  {:else if view === "lfs"}
    <LfsSettings />
  {:else if view === "backups"}
    <BackupSettings />
  {:else}
    <InstanceSettings view={view === "appearance" ? "general" : view} />
  {/if}
</div>
