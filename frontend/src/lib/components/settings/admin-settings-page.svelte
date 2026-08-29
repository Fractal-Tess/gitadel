<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { Activity, ArchiveRestore, Palette, UserPlus } from "lucide-svelte";
  import ContextNav, {
    type ContextNavItem,
  } from "$lib/components/app/context-nav.svelte";

  import BackupSettings from "$lib/components/settings/backup-settings.svelte";
  import InstanceSettings from "$lib/components/settings/instance-settings.svelte";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  type AdminView = "appearance" | "access" | "backups" | "activity";

  const app = useAppState();
  const requestedView = $derived(page.params.view ?? "appearance");
  const view = $derived<AdminView>(
    requestedView === "access" ||
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
        : view === "backups"
          ? "Backups"
          : "Activity",
  );

  const navigation = $derived.by<ContextNavItem[]>(() => [
    {
      id: "appearance",
      label: "Appearance",
      icon: Palette,
      href: resolve("/-/administration/[view]", { view: "appearance" }),
      active: view === "appearance",
    },
    {
      id: "access",
      label: "Access",
      icon: UserPlus,
      href: resolve("/-/administration/[view]", { view: "access" }),
      active: view === "access",
    },
    {
      id: "backups",
      label: "Backups",
      icon: ArchiveRestore,
      href: resolve("/-/administration/[view]", { view: "backups" }),
      active: view === "backups",
    },
    {
      id: "activity",
      label: "Activity",
      icon: Activity,
      href: resolve("/-/administration/[view]", { view: "activity" }),
      active: view === "activity",
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
  {#if view === "backups"}
    <BackupSettings />
  {:else}
    <InstanceSettings view={view === "appearance" ? "general" : view} />
  {/if}
</div>
