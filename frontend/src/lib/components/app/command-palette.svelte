<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { Command as CommandPrimitive } from "bits-ui";
  import {
    Archive,
    Compass,
    LockKeyhole,
    Plus,
    ScrollText,
    Settings2,
    ShieldCheck,
    Star,
  } from "lucide-svelte";

  import * as Command from "$lib/components/ui/command/index.js";
  import * as Kbd from "$lib/components/ui/kbd/index.js";
  import type { RepositoryOverviewItem } from "$lib/api.js";
  import { peekExplore, refreshExplore } from "$lib/navigation-cache.js";
  import { preloadRepository } from "$lib/repository/repository-preload.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import { recentRepositoryPaths } from "$lib/state/recent-repositories.js";
  import {
    useShellState,
    type ShellIcon,
  } from "$lib/state/shell-state.svelte.js";

  // Matches the explore page so the first page paints from its warm cache. The
  // endpoint pages rather than returning everything, so the palette walks pages
  // instead of searching only the first screenful and pretending that is all.
  const palettePageSize = 20;
  const paletteMaxPages = 10;
  const paletteIndexLifetime = 60_000;
  const resultLimit = 24;

  const app = useAppState();
  const shell = useShellState();
  const viewer = $derived(app.authStatus?.user?.username);

  let repositories = $state.raw<RepositoryOverviewItem[]>([]);
  let recentPaths = $state.raw<string[]>([]);
  let query = $state("");
  let selected = $state("");
  let loading = $state(false);
  let truncated = $state(false);
  let indexedViewer = "";
  let indexExpiresAt = 0;

  const relative = new Intl.RelativeTimeFormat(undefined, {
    numeric: "auto",
    style: "narrow",
  });

  type PaletteAction = {
    id: string;
    label: string;
    icon: ShellIcon;
    keywords: string;
    run: () => void;
  };

  $effect(() => {
    if (!shell.paletteOpen) return;

    query = "";
    recentPaths = recentRepositoryPaths();
    const viewerKey = viewer ?? "";
    if (indexedViewer === viewerKey && indexExpiresAt > Date.now()) {
      loading = false;
      return;
    }
    indexedViewer = "";
    indexExpiresAt = 0;
    truncated = false;

    const cached = peekExplore(1, palettePageSize, viewer);
    repositories = cached?.repositories ?? [];
    loading = !cached;

    let cancelled = false;
    void (async () => {
      const collected: RepositoryOverviewItem[] = [];
      let pageNumber = 1;
      try {
        while (pageNumber <= paletteMaxPages) {
          const overview = await refreshExplore(
            pageNumber,
            palettePageSize,
            viewer,
          );
          if (cancelled) return;
          collected.push(...overview.repositories);
          repositories = [...collected];
          loading = false;
          if (!overview.has_next) {
            indexedViewer = viewerKey;
            indexExpiresAt = Date.now() + paletteIndexLifetime;
            return;
          }
          pageNumber += 1;
        }
        truncated = true;
        indexedViewer = viewerKey;
        indexExpiresAt = Date.now() + paletteIndexLifetime;
      } catch {
        // Whatever pages did land are still worth searching.
      } finally {
        if (!cancelled) loading = false;
      }
    })();

    return () => {
      cancelled = true;
    };
  });

  function isSubsequence(haystack: string, needle: string): boolean {
    let cursor = 0;
    for (const character of haystack) {
      if (character === needle[cursor]) cursor += 1;
      if (cursor === needle.length) return true;
    }
    return false;
  }

  /**
   * Ranks a repository against the query. Repository names outrank namespaces,
   * which outrank descriptions, so typing `web` finds `acme/web` before it
   * finds `web-team/billing` or anything that merely mentions the web.
   */
  function score(
    repository: RepositoryOverviewItem,
    path: string,
    needle: string,
  ): number {
    const name = repository.name.toLowerCase();
    if (name.startsWith(needle)) return 100;
    if (path.startsWith(needle)) return 90;
    if (name.includes(needle)) return 70;
    if (path.includes(needle)) return 55;
    if (isSubsequence(path, needle)) return 35;
    if ((repository.description ?? "").toLowerCase().includes(needle))
      return 20;
    return 0;
  }

  const results = $derived.by(() => {
    const needle = query.trim().toLowerCase();

    if (!needle) {
      const byPath = new Map(
        repositories.map((repository) => [
          `${repository.namespace}/${repository.name}`,
          repository,
        ]),
      );
      const recent = recentPaths
        .map((path) => byPath.get(path))
        .filter((repository) => repository !== undefined);
      const recentIds = new Set(recent.map((repository) => repository.id));
      const matches = repositories
        .filter((repository) => !recentIds.has(repository.id))
        .sort(
          (left, right) =>
            Number(right.favorited) - Number(left.favorited) ||
            Date.parse(right.updated_at) - Date.parse(left.updated_at),
        );
      return {
        recent,
        matches: matches.slice(0, resultLimit),
        matched: repositories.length,
      };
    }

    const recency = new Map(
      recentPaths.map((path, index) => [path, recentPaths.length - index]),
    );
    const ranked = repositories
      .map((repository) => {
        const path = `${repository.namespace}/${repository.name}`;
        const base = score(repository, path.toLowerCase(), needle);
        return {
          repository,
          rank:
            base === 0
              ? 0
              : base +
                (repository.favorited ? 4 : 0) +
                (recency.get(path) ?? 0),
        };
      })
      .filter((entry) => entry.rank > 0)
      .sort(
        (left, right) =>
          right.rank - left.rank ||
          Date.parse(right.repository.updated_at) -
            Date.parse(left.repository.updated_at),
      );

    return {
      recent: [],
      matches: ranked.slice(0, resultLimit).map((entry) => entry.repository),
      matched: ranked.length,
    };
  });

  function matches(action: PaletteAction): boolean {
    const needle = query.trim().toLowerCase();
    if (!needle) return true;
    return `${action.label} ${action.keywords}`.toLowerCase().includes(needle);
  }

  const navigationActions = $derived.by(() => {
    const actions: PaletteAction[] = [
      {
        id: "explore",
        label: "Explore",
        icon: Compass,
        keywords: "all repositories browse home",
        run: () => void goto(resolve("/")),
      },
      {
        id: "favorites",
        label: "Favorites",
        icon: Star,
        keywords: "starred saved",
        run: () => void goto(`${resolve("/")}?tab=favorites`),
      },
    ];
    if (app.authStatus?.authenticated) {
      actions.push({
        id: "settings",
        label: "Account settings",
        icon: Settings2,
        keywords: "profile security passkeys tokens ssh keys avatar",
        run: () => void goto(resolve("/settings")),
      });
    }
    if (app.authStatus?.user?.is_admin) {
      actions.push({
        id: "admin",
        label: "Administration",
        icon: ShieldCheck,
        keywords: "instance users audit log",
        run: () => void goto(resolve("/admin")),
      });
    }
    actions.push({
      id: "changelog",
      label: "Changelog",
      icon: ScrollText,
      keywords: "releases version updates",
      run: () => void goto(resolve("/changelog")),
    });
    return actions.filter(matches);
  });

  const createActions = $derived.by(() => {
    if (!app.authStatus?.authenticated) return [];
    return [
      {
        id: "create",
        label: "New repository",
        icon: Plus,
        keywords: "create add initialise initialize",
        run: () => (shell.createOpen = true),
      } satisfies PaletteAction,
    ].filter(matches);
  });

  const repositoryRows = $derived(
    results.recent.length + results.matches.length,
  );
  const hasResults = $derived(
    repositoryRows + navigationActions.length + createActions.length > 0,
  );
  const countLabel = $derived.by(() => {
    const total = `${repositories.length}${truncated ? "+" : ""}`;
    if (loading && repositories.length === 0) return "Loading…";
    if (query.trim()) return `${results.matched} of ${total}`;
    return `${total} ${repositories.length === 1 ? "repository" : "repositories"}`;
  });

  // Preloading the highlighted row means arrow keys warm the page up before
  // Enter is ever pressed.
  const rowIndex = $derived.by(() => {
    const index = new Map<string, RepositoryOverviewItem>();
    for (const repository of results.recent) {
      index.set(`recent:${repository.id}`, repository);
    }
    for (const repository of results.matches) {
      index.set(`match:${repository.id}`, repository);
    }
    return index;
  });

  $effect(() => {
    const repository = rowIndex.get(selected);
    if (repository) {
      preloadRepository(
        repository.namespace,
        repository.name,
        repository.default_branch,
      );
    }
  });

  function updatedLabel(value: string): string {
    const minutes = Math.round((Date.now() - Date.parse(value)) / 60_000);
    if (minutes < 60) return relative.format(-minutes, "minute");
    const hours = Math.round(minutes / 60);
    if (hours < 24) return relative.format(-hours, "hour");
    const days = Math.round(hours / 24);
    if (days < 30) return relative.format(-days, "day");
    const months = Math.round(days / 30);
    if (months < 12) return relative.format(-months, "month");
    return relative.format(-Math.round(months / 12), "year");
  }

  function run(action: () => void): void {
    shell.paletteOpen = false;
    action();
  }

  function openRepository(repository: RepositoryOverviewItem): void {
    run(
      () =>
        void goto(
          resolve("/[namespace]/[name]", {
            namespace: repository.namespace,
            name: repository.name,
          }),
        ),
    );
  }

  function isTyping(target: EventTarget | null): boolean {
    if (!(target instanceof HTMLElement)) return false;
    return (
      target.isContentEditable ||
      target.tagName === "INPUT" ||
      target.tagName === "TEXTAREA" ||
      target.tagName === "SELECT"
    );
  }

  function handleShortcut(event: KeyboardEvent): void {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      shell.paletteOpen = !shell.paletteOpen;
      return;
    }
    // `/` is the path separator the palette searches over, so it is also the
    // key that opens it — as long as the viewer is not mid-sentence somewhere.
    if (
      event.key === "/" &&
      !event.metaKey &&
      !event.ctrlKey &&
      !event.altKey &&
      !shell.paletteOpen &&
      !isTyping(event.target)
    ) {
      event.preventDefault();
      shell.paletteOpen = true;
    }
  }

  const groupClass =
    "px-1 pb-1 [&_[data-command-group-heading]]:px-2.5 [&_[data-command-group-heading]]:pt-2.5 [&_[data-command-group-heading]]:pb-1.5 [&_[data-command-group-heading]]:text-[11px] [&_[data-command-group-heading]]:font-medium [&_[data-command-group-heading]]:tracking-wider [&_[data-command-group-heading]]:uppercase [&_[data-command-group-heading]]:text-muted-foreground/70";
  const rowClass = "gap-2.5 px-2.5 py-2 [&_.cn-command-item-indicator]:hidden";
</script>

<svelte:window onkeydown={handleShortcut} />

{#snippet repositoryRow(repository: RepositoryOverviewItem, group: string)}
  <Command.Item
    class={rowClass}
    value={`${group}:${repository.id}`}
    onSelect={() => openRepository(repository)}
    onmouseenter={() =>
      preloadRepository(
        repository.namespace,
        repository.name,
        repository.default_branch,
      )}
  >
    <span class="min-w-0 flex-1 truncate font-mono text-[0.8rem]">
      <span class="text-muted-foreground">{repository.namespace}/</span><span
        class="font-medium">{repository.name}</span
      >
    </span>
    {#if repository.favorited}
      <Star class="size-3 shrink-0 fill-amber-400 text-amber-400" />
    {/if}
    {#if repository.visibility === "private"}
      <LockKeyhole class="size-3 shrink-0 text-muted-foreground" />
    {/if}
    {#if repository.archived_at}
      <Archive class="size-3 shrink-0 text-muted-foreground" />
    {/if}
    <span
      class="shrink-0 text-[11px] tabular-nums text-muted-foreground"
      title={`Updated ${new Date(repository.updated_at).toLocaleString()}`}
    >
      {updatedLabel(repository.updated_at)}
    </span>
  </Command.Item>
{/snippet}

{#snippet actionRows(actions: PaletteAction[])}
  {#each actions as action (action.id)}
    <Command.Item
      class={rowClass}
      value={`action:${action.id}`}
      onSelect={() => run(action.run)}
    >
      <action.icon class="shrink-0 text-muted-foreground" />
      <span class="truncate">{action.label}</span>
    </Command.Item>
  {/each}
{/snippet}

<Command.Dialog
  bind:open={shell.paletteOpen}
  bind:value={selected}
  title="Search Gitadel"
  description="Jump to a repository or run a command."
  shouldFilter={false}
  loop
  class="top-[10vh] sm:max-w-xl"
>
  <!--
    The prompt is a monospace `/`: the separator in every `namespace/name` the
    field searches, and the key that opens it.
  -->
  <div class="-mx-1 -mt-1 flex items-center gap-2.5 border-b px-3.5">
    <span
      aria-hidden="true"
      class="font-mono text-base leading-none text-activity-3"
    >
      /
    </span>
    <CommandPrimitive.Input
      bind:value={query}
      class="h-12 min-w-0 flex-1 bg-transparent font-mono text-sm text-foreground outline-none placeholder:font-sans placeholder:text-muted-foreground"
      placeholder="Jump to a repository or run a command"
    />
    <Kbd.Root
      class="shrink-0 border border-border/60 bg-transparent px-1.5 font-mono text-[10px]"
    >
      Esc
    </Kbd.Root>
  </div>

  <Command.List class="max-h-[min(26rem,54vh)] scroll-py-2">
    {#if loading && repositories.length === 0}
      <div class="space-y-1 p-1 pt-2" aria-hidden="true">
        {#each Array(5) as _, index (index)}
          <div class="h-9 animate-pulse rounded-lg bg-muted/40"></div>
        {/each}
      </div>
    {:else if !hasResults}
      <div class="px-3.5 py-9 text-center">
        <p class="text-sm">
          {query.trim() ? "No matches" : "No repositories yet"}
        </p>
        <p class="mx-auto mt-1.5 max-w-xs text-xs text-muted-foreground">
          {query.trim()
            ? "Try a namespace, a repository name, or part of a description."
            : "Create a repository and it will show up here."}
        </p>
      </div>
    {:else}
      {#if results.recent.length}
        <Command.Group class={groupClass} heading="Recent">
          {#each results.recent as repository (repository.id)}
            {@render repositoryRow(repository, "recent")}
          {/each}
        </Command.Group>
      {/if}

      {#if results.matches.length}
        <Command.Group class={groupClass} heading="Repositories">
          {#each results.matches as repository (repository.id)}
            {@render repositoryRow(repository, "match")}
          {/each}
        </Command.Group>
      {/if}

      {#if repositoryRows && navigationActions.length + createActions.length}
        <Command.Separator />
      {/if}

      {#if navigationActions.length}
        <Command.Group class={groupClass} heading="Go to">
          {@render actionRows(navigationActions)}
        </Command.Group>
      {/if}

      {#if createActions.length}
        <Command.Group class={groupClass} heading="Actions">
          {@render actionRows(createActions)}
        </Command.Group>
      {/if}
    {/if}
  </Command.List>

  <div
    class="-mx-1 -mb-1 flex items-center justify-between gap-3 border-t px-3.5 py-2 text-[11px] text-muted-foreground"
  >
    <span class="flex items-center gap-3">
      <span class="flex items-center gap-1.5">
        <Kbd.Root class="bg-transparent">↑</Kbd.Root>
        <Kbd.Root class="bg-transparent">↓</Kbd.Root>
        Move
      </span>
      <span class="flex items-center gap-1.5">
        <Kbd.Root class="bg-transparent">↵</Kbd.Root>
        Open
      </span>
    </span>
    <span class="tabular-nums">{countLabel}</span>
  </div>
</Command.Dialog>
