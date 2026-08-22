<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import {
    Compass,
    PanelLeftClose,
    PanelLeftOpen,
    ScrollText,
    Settings2,
    ShieldCheck,
    Star,
  } from "lucide-svelte";

  import * as Sheet from "$lib/components/ui/sheet/index.js";
  import {
    preloadAccountSettings,
    preloadExplore,
  } from "$lib/navigation-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import {
    useShellState,
    type ShellIcon,
  } from "$lib/state/shell-state.svelte.js";

  const app = useAppState();
  const shell = useShellState();
  const viewer = $derived(app.authStatus?.user?.username);

  type RailLink = {
    label: string;
    href: string;
    icon: ShellIcon;
    active: boolean;
    preload?: () => void;
  };

  const browseLinks = $derived.by<RailLink[]>(() => {
    const onExplore = page.url.pathname === "/";
    const tab = page.url.searchParams.get("tab");
    return [
      {
        label: "All repositories",
        href: resolve("/"),
        icon: Compass,
        active: onExplore && tab !== "favorites",
        preload: () => preloadExplore(viewer),
      },
      {
        label: "Favorites",
        href: `${resolve("/")}?tab=favorites`,
        icon: Star,
        active: onExplore && tab === "favorites",
        preload: () => preloadExplore(viewer),
      },
    ];
  });

  const manageLinks = $derived.by<RailLink[]>(() => {
    const links: RailLink[] = [];
    if (app.authStatus?.authenticated) {
      links.push({
        label: "Account settings",
        href: resolve("/settings"),
        icon: Settings2,
        active: page.url.pathname === "/settings",
        preload: () => preloadAccountSettings(viewer),
      });
    }
    if (app.authStatus?.user?.is_admin) {
      links.push({
        label: "Administration",
        href: resolve("/admin"),
        icon: ShieldCheck,
        active: page.url.pathname.startsWith("/admin"),
      });
    }
    links.push({
      label: "Changelog",
      href: resolve("/changelog"),
      icon: ScrollText,
      active: page.url.pathname === "/changelog",
    });
    return links;
  });
</script>

{#snippet body(collapsed: boolean, desktop = false)}
  {@const row = collapsed
    ? "flex h-9 items-center justify-center rounded-md"
    : "flex h-9 items-center gap-3 rounded-md px-3"}
  {@const idle = "text-muted-foreground hover:bg-accent hover:text-foreground"}
  {@const on = "bg-accent font-medium text-foreground"}
  {@const heading =
    "px-3 pb-1 text-[11px] font-medium uppercase tracking-wider text-muted-foreground/70"}

  <div class="flex min-h-0 flex-1 flex-col overflow-y-auto py-4">
    <nav class="space-y-1 px-2" aria-label="Browse">
      {#if !collapsed}<p class={heading}>Browse</p>{/if}
      {#each browseLinks as link (link.label)}
        <a
          class="{row} {link.active ? on : idle} text-sm"
          href={link.href}
          aria-current={link.active ? "page" : undefined}
          aria-label={collapsed ? link.label : undefined}
          title={collapsed ? link.label : undefined}
          onpointerenter={link.preload}
          onfocus={link.preload}
          onclick={() => (shell.railMobileOpen = false)}
        >
          <link.icon class="size-4 shrink-0" />
          {#if !collapsed}<span class="truncate">{link.label}</span>{/if}
        </a>
      {/each}
    </nav>

    {#if shell.navGroup}
      <nav
        class="mt-5 space-y-1 border-t px-2 pt-5"
        aria-label={shell.navGroup.label}
      >
        {#if !collapsed}<p class={heading}>{shell.navGroup.label}</p>{/if}
        {#each shell.navGroup.items as item (item.id)}
          <button
            class="{row} {item.active ? on : idle} w-full text-left text-sm"
            aria-current={item.active ? "page" : undefined}
            aria-label={collapsed ? item.label : undefined}
            title={collapsed ? item.label : undefined}
            onclick={() => {
              item.select();
              shell.railMobileOpen = false;
            }}
          >
            <item.icon class="size-4 shrink-0" />
            {#if !collapsed}<span class="truncate">{item.label}</span>{/if}
          </button>
        {/each}
      </nav>
    {/if}

    <div class="mt-auto border-t px-2 pt-4">
      {#if desktop}
        <button
          type="button"
          class="{row} {idle} mb-1 w-full text-left text-sm"
          aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
          aria-expanded={!collapsed}
          aria-controls="app-navigation"
          title={collapsed ? "Expand navigation" : undefined}
          onclick={() => shell.toggleRail()}
        >
          {#if collapsed}
            <PanelLeftOpen class="size-4 shrink-0" />
          {:else}
            <PanelLeftClose class="size-4 shrink-0" />
            <span class="truncate">Collapse sidebar</span>
          {/if}
        </button>
      {/if}

      <nav class="space-y-1" aria-label="Manage">
        {#each manageLinks as link (link.label)}
          <a
            class="{row} {link.active ? on : idle} text-sm"
            href={link.href}
            aria-current={link.active ? "page" : undefined}
            aria-label={collapsed ? link.label : undefined}
            title={collapsed ? link.label : undefined}
            onpointerenter={link.preload}
            onfocus={link.preload}
            onclick={() => (shell.railMobileOpen = false)}
          >
            <link.icon class="size-4 shrink-0" />
            {#if !collapsed}<span class="truncate">{link.label}</span>{/if}
          </a>
        {/each}
      </nav>
    </div>
  </div>
{/snippet}

<aside
  id="app-navigation"
  class="relative hidden shrink-0 border-r transition-[width] duration-150 md:flex md:flex-col {shell.railOpen
    ? 'w-60'
    : 'w-14'}"
>
  {@render body(!shell.railOpen, true)}

  <button
    type="button"
    class="absolute inset-y-0 -right-2 z-20 hidden w-4 {shell.railOpen
      ? 'cursor-w-resize'
      : 'cursor-e-resize'} after:absolute after:inset-y-0 after:left-1/2 after:w-[2px] after:-translate-x-1/2 after:bg-transparent after:transition-colors hover:after:bg-border md:block"
    aria-label={shell.railOpen ? "Collapse navigation" : "Expand navigation"}
    aria-expanded={shell.railOpen}
    aria-controls="app-navigation"
    tabindex={-1}
    title={shell.railOpen ? "Collapse navigation" : "Expand navigation"}
    onclick={() => shell.toggleRail()}
  ></button>
</aside>

<Sheet.Root bind:open={shell.railMobileOpen}>
  <Sheet.Content side="left" class="w-64 p-0">
    <Sheet.Header class="sr-only">
      <Sheet.Title>Navigation</Sheet.Title>
      <Sheet.Description>Browse repositories and settings.</Sheet.Description>
    </Sheet.Header>
    <div class="flex h-full flex-col">
      {@render body(false)}
    </div>
  </Sheet.Content>
</Sheet.Root>
