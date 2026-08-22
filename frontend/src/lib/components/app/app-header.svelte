<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { PanelLeft, Plus, Search } from "lucide-svelte";

  import BrandMark from "$lib/components/brand-mark.svelte";
  import UserMenu from "$lib/components/app/user-menu.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Kbd from "$lib/components/ui/kbd/index.js";
  import {
    preloadExplore,
    preloadRepositoryIndex,
  } from "$lib/navigation-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  const app = useAppState();
  const shell = useShellState();
  const siteName = $derived(app.instance?.site_name ?? "GITADEL");

  // The palette answers to both ⌘/Ctrl+K and `/`, but only the modifier needs
  // spelling out: the prompt glyph in the field teaches the slash by itself.
  const searchHint = /mac/i.test(globalThis.navigator?.platform ?? "")
    ? "⌘ K"
    : "Ctrl K";

  $effect(() => {
    const viewer = app.authStatus?.user?.username;
    if (typeof window.requestIdleCallback === "function") {
      const idle = window.requestIdleCallback(
        () => preloadRepositoryIndex(viewer),
        { timeout: 2_000 },
      );
      return () => window.cancelIdleCallback(idle);
    }
    const timer = window.setTimeout(() => preloadRepositoryIndex(viewer), 1_000);
    return () => window.clearTimeout(timer);
  });

  // Derived from the URL rather than published by each page, so the trail can
  // never fall out of sync with where the user actually is.
  const crumbs = $derived.by(() => {
    const path = page.url.pathname;
    if (path === "/") return ["Explore"];
    if (path === "/settings") return ["Settings"];
    if (path.startsWith("/admin")) return ["Administration"];
    if (path === "/changelog") return ["Changelog"];
    const { namespace, name } = page.params;
    if (namespace && name) return [namespace, name];
    return [];
  });
</script>

<header
  class="flex h-16 shrink-0 items-center gap-3 border-b bg-background px-4 sm:px-5"
>
  <Button
    variant="ghost"
    size="icon"
    class="shrink-0 text-muted-foreground hover:text-foreground md:hidden"
    aria-label="Open navigation"
    onclick={() => (shell.railMobileOpen = true)}
  >
    <PanelLeft class="size-4" />
  </Button>

  <nav
    class="flex min-w-0 flex-1 items-center gap-2 text-sm"
    aria-label="Breadcrumb"
  >
    <a
      class="flex shrink-0 items-center gap-2 font-bold tracking-[-0.035em]"
      href={resolve("/")}
      onpointerenter={() => preloadExplore(app.authStatus?.user?.username)}
      onfocus={() => preloadExplore(app.authStatus?.user?.username)}
      aria-label={`${siteName} home`}
    >
      <BrandMark />
      <span class="hidden max-w-40 truncate sm:inline">{siteName}</span>
    </a>
    {#each crumbs as crumb, index (crumb)}
      <span class="shrink-0 text-muted-foreground">/</span>
      <span
        class={index === crumbs.length - 1
          ? "min-w-0 truncate font-medium"
          : "hidden min-w-0 truncate text-muted-foreground sm:inline"}
      >
        {crumb}
      </span>
    {/each}
  </nav>

  <!--
    A repository is an address, so the field is typeset as one: a monospace
    prompt where a magnifier would sit, and the same glyph that opens it.
  -->
  <button
    type="button"
    class="group hidden h-9 w-56 shrink-0 items-center gap-2.5 rounded-lg border border-input/40 bg-input/20 px-2.5 text-left outline-none hover:border-input hover:bg-input/35 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 md:flex xl:w-72"
    aria-keyshortcuts="Control+K Meta+K /"
    onpointerenter={() => preloadRepositoryIndex(app.authStatus?.user?.username)}
    onfocus={() => preloadRepositoryIndex(app.authStatus?.user?.username)}
    onclick={() => (shell.paletteOpen = true)}
  >
    <span
      aria-hidden="true"
      class="font-mono text-sm leading-none text-activity-3/70 group-hover:text-activity-3"
    >
      /
    </span>
    <span
      class="flex-1 truncate text-sm text-muted-foreground group-hover:text-foreground"
    >
      Search repositories
    </span>
    <Kbd.Root
      class="shrink-0 border border-border/60 bg-transparent px-1.5 font-mono text-[10px] tracking-wide"
    >
      {searchHint}
    </Kbd.Root>
  </button>
  <Button
    variant="ghost"
    size="icon"
    class="shrink-0 text-muted-foreground hover:text-foreground md:hidden"
    aria-label="Search repositories"
    onpointerenter={() => preloadRepositoryIndex(app.authStatus?.user?.username)}
    onfocus={() => preloadRepositoryIndex(app.authStatus?.user?.username)}
    onclick={() => (shell.paletteOpen = true)}
  >
    <Search class="size-4" />
  </Button>

  {#if app.authStatus?.authenticated}
    <Button class="shrink-0 gap-2" onclick={() => (shell.createOpen = true)}>
      <Plus class="size-4" />
      <span class="hidden sm:inline">New repository</span>
    </Button>
  {/if}

  <UserMenu />
</header>
