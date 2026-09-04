<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import PanelLeft from "@lucide/svelte/icons/panel-left";
  import Plus from "@lucide/svelte/icons/plus";
  import Search from "@lucide/svelte/icons/search";

  import BrandMark from "$lib/components/brand-mark.svelte";
  import UserMenu from "$lib/components/app/user-menu.svelte";
  import ThemeSwitcher from "$lib/components/app/theme-switcher.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Kbd from "$lib/components/ui/kbd/index.js";
  import { useSidebar } from "$lib/components/ui/sidebar/index.js";
  import {
    preloadExplore,
    preloadRepositoryIndex,
  } from "$lib/navigation-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  const app = useAppState();
  const shell = useShellState();
  const sidebar = useSidebar();
  const siteName = $derived(app.instance?.site_name ?? "GITADEL");

  // The palette answers to both ⌘/Ctrl+K and `/`, but only the modifier needs
  // spelling out: the prompt glyph in the field teaches the slash by itself.
  const searchHint = /mac/i.test(globalThis.navigator?.platform ?? "")
    ? "⌘ K"
    : "Ctrl K";

  $effect(() => {
    const scope = app.authorizationScope;
    if (typeof window.requestIdleCallback === "function") {
      const idle = window.requestIdleCallback(
        () => preloadRepositoryIndex(scope),
        { timeout: 2_000 },
      );
      return () => window.cancelIdleCallback(idle);
    }
    const timer = window.setTimeout(() => preloadRepositoryIndex(scope), 1_000);
    return () => window.clearTimeout(timer);
  });

  type Crumb = { label: string; href?: string };

  const viewLabels: Record<string, string> = {
    profile: "Profile",
    authentication: "Authentication",
    "ssh-keys": "SSH keys",
    "api-tokens": "API tokens",
    "oauth-applications": "OAuth applications",
    appearance: "Appearance",
    access: "Access",
    storage: "Storage",
    backups: "Backups",
    activity: "Activity",
    members: "Members",
    runners: "Runners",
    integrations: "Integrations",
    "mirror-credentials": "Mirror identities",
    overview: "Code",
    history: "History",
    actions: "Actions",
    issues: "Issues",
    releases: "Releases",
    tags: "Tags",
    settings: "Settings",
    general: "General",
    location: "Location",
    mirror: "Mirror",
    webhooks: "Webhooks",
    danger: "Danger zone",
  };

  const crumbs = $derived.by<Crumb[]>(() => {
    const path = page.url.pathname;
    if (path === "/") return [{ label: "Explore" }];
    if (path === "/changelog") return [{ label: "Changelog" }];
    if (path === "/settings") return [{ label: "Account settings" }];
    if (path === "/-/organizations") return [{ label: "Organizations" }];
    if (path.startsWith("/-/account/")) {
      return [
        {
          label: "Account",
          href: resolve("/-/account/[view]", { view: "profile" }),
        },
        { label: viewLabels[page.params.view ?? ""] ?? "Profile" },
      ];
    }
    if (path.startsWith("/-/administration/")) {
      return [
        {
          label: "Administration",
          href: resolve("/-/administration/[view]", { view: "appearance" }),
        },
        { label: viewLabels[page.params.view ?? ""] ?? "Appearance" },
      ];
    }
    if (path === "/imports/new") {
      return [{ label: "Import repositories" }];
    }
    if (path.startsWith("/imports/")) {
      return [{ label: "Imports" }, { label: "Repository import" }];
    }
    const { namespace, name } = page.params;
    const namespaceView = path.split("/").at(-1) ?? "";
    if (
      namespace &&
      (namespaceView === "members" ||
        namespaceView === "runners" ||
        namespaceView === "integrations" ||
        namespaceView === "mirror-credentials" ||
        namespaceView === "settings")
    ) {
      return [
        {
          label: namespace,
          href: resolve("/[namespace]", { namespace }),
        },
        { label: viewLabels[namespaceView] ?? namespaceView },
      ];
    }
    if (namespace && name) {
      const repositoryHref = resolve("/[namespace]/[name]", {
        namespace,
        name,
      });
      const view = page.url.searchParams.get("view") ?? "overview";
      const trail: Crumb[] = [
        {
          label: namespace,
          href: resolve("/[namespace]", { namespace }),
        },
        {
          label: name,
          href: view === "overview" ? undefined : repositoryHref,
        },
      ];
      if (view !== "overview") {
        trail.push({ label: viewLabels[view] ?? view });
      }
      if (view === "settings") {
        const tab = page.url.searchParams.get("tab") ?? "general";
        trail.push({ label: viewLabels[tab] ?? tab });
      }
      return trail;
    }
    if (namespace) return [{ label: namespace }];
    return [];
  });
</script>

<header
  class="flex h-16 shrink-0 items-center gap-3 border-b bg-background px-4 sm:px-5"
>
  {#if !shell.railHidden}
    <Button
      variant="ghost"
      size="icon"
      class="shrink-0 text-muted-foreground hover:text-foreground md:hidden"
      aria-label="Open navigation"
      onclick={() => sidebar.setOpenMobile(true)}
    >
      <PanelLeft class="size-4" />
    </Button>
  {/if}

  <nav
    class="flex min-w-0 flex-1 items-center gap-2 text-sm"
    aria-label="Breadcrumb"
  >
    <a
      class="flex shrink-0 items-center gap-2 font-bold tracking-[-0.035em]"
      href={resolve("/")}
      onpointerenter={() => preloadExplore(app.authorizationScope)}
      onfocus={() => preloadExplore(app.authorizationScope)}
      aria-label={`${siteName} home`}
    >
      <BrandMark />
      <span class="hidden max-w-40 truncate sm:inline">{siteName}</span>
    </a>
    {#each crumbs as crumb, index (crumb.label)}
      <span
        class={index === crumbs.length - 1
          ? "shrink-0 text-muted-foreground"
          : "hidden shrink-0 text-muted-foreground sm:inline"}>/</span
      >
      {#if crumb.href}
        <a
          class={index === crumbs.length - 1
            ? "min-w-0 truncate font-medium"
            : "hidden min-w-0 truncate text-muted-foreground hover:text-foreground sm:inline"}
          href={crumb.href}
        >
          {crumb.label}
        </a>
      {:else}
        <span
          class={index === crumbs.length - 1
            ? "min-w-0 truncate font-medium"
            : "hidden min-w-0 truncate text-muted-foreground sm:inline"}
        >
          {crumb.label}
        </span>
      {/if}
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
    onpointerenter={() => preloadRepositoryIndex(app.authorizationScope)}
    onfocus={() => preloadRepositoryIndex(app.authorizationScope)}
    onclick={() => (shell.paletteOpen = true)}
  >
    <span
      aria-hidden="true"
      class="font-mono text-sm leading-none text-foreground/80 group-hover:text-foreground"
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
    onpointerenter={() => preloadRepositoryIndex(app.authorizationScope)}
    onfocus={() => preloadRepositoryIndex(app.authorizationScope)}
    onclick={() => (shell.paletteOpen = true)}
  >
    <Search class="size-4" />
  </Button>

  {#if app.authStatus?.authenticated}
    <Button
      class="shrink-0"
      aria-label="Create or import repositories and organizations"
      title="Create or import repositories and organizations"
      onclick={() => (shell.createOpen = true)}
    >
      <Plus data-icon="inline-start" />
      <span class="hidden sm:inline">New</span>
    </Button>
    <ThemeSwitcher />
  {/if}

  <UserMenu />
</header>
