<script lang="ts">
  import { resolve } from "$app/paths";
  import { SiGithub } from "@icons-pack/svelte-simple-icons";
  import {
    History,
    LockKeyhole,
    Menu,
    Search,
    Settings2,
    Tag,
  } from "lucide-svelte";

  import BrandMark from "$lib/components/brand-mark.svelte";
  import {
    preloadAccountSettings,
    preloadExplore,
  } from "$lib/navigation-cache.js";
  import type {
    RepositoryPageState,
    RepositoryView,
  } from "$lib/repository/repository-page-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
  const app = useAppState();

  const sections: Array<{
    id: RepositoryView;
    label: string;
    icon: typeof Menu;
  }> = [
    { id: "overview", label: "Overview", icon: Menu },
    { id: "history", label: "History", icon: History },
    { id: "tags", label: "Tags", icon: Tag },
  ];
</script>

<header class="sticky top-0 z-30 border-b bg-background/95 backdrop-blur">
  <div class="flex h-[64px] items-center gap-4 px-4 sm:px-5">
    <div class="flex min-w-0 flex-1 items-center gap-2 text-sm">
      <a
        class="flex shrink-0 items-center gap-2 font-bold tracking-[-0.035em]"
        href={resolve("/")}
        onpointerenter={() => preloadExplore(app.authStatus?.user?.username)}
        onpointerdown={() => preloadExplore(app.authStatus?.user?.username)}
        onfocus={() => preloadExplore(app.authStatus?.user?.username)}
        aria-label={`${app.instance?.site_name ?? "Gitadel"} home`}
      >
        <BrandMark />
        <span class="max-w-24 truncate sm:max-w-none">
          {app.instance?.site_name ?? "GITADEL"}
        </span>
      </a>
      <span class="text-muted-foreground">/</span>
      <a
        class="hidden truncate text-muted-foreground hover:text-foreground sm:inline"
        href={resolve("/")}
        onpointerenter={() => preloadExplore(app.authStatus?.user?.username)}
        onfocus={() => preloadExplore(app.authStatus?.user?.username)}
        >{state.namespace}</a
      >
      <span class="hidden text-muted-foreground sm:inline">/</span>
      <strong class="max-w-40 truncate sm:max-w-56">{state.name}</strong>
      {#if state.repository?.visibility === "private"}
        <LockKeyhole class="ml-1 size-3.5 text-muted-foreground" />
      {/if}
    </div>

    <form
      class="relative hidden w-full max-w-md lg:block"
      action={resolve("/")}
      method="get"
    >
      <label class="sr-only" for="repository-search">Search repositories</label>
      <Search
        class="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
      />
      <input
        id="repository-search"
        name="q"
        class="h-10 w-full rounded-md border bg-input/45 pl-10 pr-3 text-sm outline-none placeholder:text-muted-foreground/70 focus:border-ring focus:ring-2 focus:ring-ring/15"
        placeholder="Search repositories..."
      />
    </form>

    {#if state.repository}
      <nav
        class="ml-auto hidden h-full items-center overflow-x-auto sm:flex"
        aria-label="Repository sections"
      >
        {#each sections as item (item.id)}
          <button
            class={state.view === item.id ||
            (state.view === "commit" && item.id === "history")
              ? "inline-flex h-full items-center gap-2 whitespace-nowrap border-b-2 border-foreground px-3 text-sm font-medium"
              : "inline-flex h-full items-center gap-2 whitespace-nowrap border-b-2 border-transparent px-3 text-sm font-medium text-muted-foreground"}
            onclick={() => state.navigate(item.id)}
          >
            <item.icon class="size-4" />{item.label}
          </button>
        {/each}
      </nav>
    {:else}
      <span class="ml-auto"></span>
    {/if}

    <a
      class="inline-flex size-9 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
      href="https://github.com/Fractal-Tess/gitadel"
      target="_blank"
      rel="noreferrer"
      aria-label="Gitadel on GitHub"
      title="Gitadel on GitHub"
    >
      <SiGithub size={16} />
    </a>
    <a
      class="inline-flex size-9 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
      href={resolve("/settings")}
      onpointerenter={() =>
        preloadAccountSettings(app.authStatus?.user?.username)}
      onpointerdown={() =>
        preloadAccountSettings(app.authStatus?.user?.username)}
      onfocus={() => preloadAccountSettings(app.authStatus?.user?.username)}
      aria-label="Settings"
      title="Settings"
    >
      <Settings2 class="size-4" />
    </a>
  </div>
  {#if state.repository}
    <nav
      class="grid h-11 grid-cols-3 border-t sm:hidden"
      aria-label="Repository sections"
    >
      {#each sections as item (item.id)}
        <button
          class={state.view === item.id ||
          (state.view === "commit" && item.id === "history")
            ? "inline-flex items-center justify-center gap-2 border-b-2 border-foreground text-xs font-medium"
            : "inline-flex items-center justify-center gap-2 border-b-2 border-transparent text-xs font-medium text-muted-foreground"}
          onclick={() => state.navigate(item.id)}
        >
          <item.icon class="size-3.5" />{item.label}
        </button>
      {/each}
    </nav>
  {/if}
</header>
