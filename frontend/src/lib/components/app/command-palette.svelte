<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { Command as CommandPrimitive } from "bits-ui";
  import Archive from "@lucide/svelte/icons/archive";
  import Building2 from "@lucide/svelte/icons/building-2";
  import Compass from "@lucide/svelte/icons/compass";
  import FilePlus2 from "@lucide/svelte/icons/file-plus-2";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Heart from "@lucide/svelte/icons/heart";
  import LockKeyhole from "@lucide/svelte/icons/lock-keyhole";
  import Plus from "@lucide/svelte/icons/plus";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import ScrollText from "@lucide/svelte/icons/scroll-text";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";

  import RepositoryIcon from "$lib/components/repository/repository-icon.svelte";
  import * as Command from "$lib/components/ui/command/index.js";
  import * as Kbd from "$lib/components/ui/kbd/index.js";
  import type { Repository } from "$lib/api/repositories.js";
  import {
    loadRepositoryIndex,
    peekRepositoryIndex,
  } from "$lib/navigation-cache.js";
  import {
    cancelRepositoryPreload,
    scheduleRepositoryPreload,
  } from "$lib/repository/repository-preload.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import {
    accountSettingsSections,
    adminSettingsSections,
  } from "$lib/settings/navigation.js";
  import { repositorySettingsSections } from "$lib/repository/settings-sections.js";
  import { recentRepositoryPaths } from "$lib/state/recent-repositories.js";
  import {
    useShellState,
    type ShellIcon,
  } from "$lib/state/shell-state.svelte.js";

  const resultLimit = 24;

  const app = useAppState();
  const shell = useShellState();
  const viewer = $derived(app.authStatus?.user?.username);
  const scope = $derived(app.authorizationScope);
  let repositories = $state.raw<Repository[]>([]);
  let recentPaths = $state.raw<string[]>([]);
  let query = $state("");
  let selected = $state("");
  let loading = $state(false);

  type IndexedRepository = {
    repository: Repository;
    path: string;
    normalizedName: string;
    normalizedPath: string;
    normalizedDescription: string;
    updatedAt: number;
    normalizedTopics: string[];
  };

  const indexedRepositories = $derived(
    repositories.map((repository) => {
      const path = `${repository.namespace}/${repository.name}`;
      return {
        repository,
        path,
        normalizedName: repository.name.toLowerCase(),
        normalizedPath: path.toLowerCase(),
        normalizedDescription: (repository.description ?? "").toLowerCase(),
        normalizedTopics: repository.topics.map((topic) => topic.toLowerCase()),
        updatedAt: Date.parse(repository.updated_at),
      };
    }),
  );

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
    const cached = peekRepositoryIndex(scope);
    repositories = cached ?? [];
    loading = !cached;

    let cancelled = false;
    void loadRepositoryIndex(scope)
      .then((loaded) => {
        if (!cancelled) repositories = loaded;
      })
      .catch(() => undefined)
      .finally(() => {
        if (!cancelled) loading = false;
      });

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
  type SearchQuery = {
    needle: string;
    terms: string[];
    topics: string[];
  };

  function parseSearchQuery(value: string): SearchQuery {
    const terms: string[] = [];
    const topics: string[] = [];
    for (const token of value.trim().toLowerCase().split(/\s+/)) {
      if (token.startsWith("topic:") && token.length > "topic:".length) {
        topics.push(token.slice("topic:".length));
      } else if (token) {
        terms.push(token);
      }
    }
    return { needle: terms.join(" "), terms, topics };
  }

  const parsedQuery = $derived(parseSearchQuery(query));

  function score(repository: IndexedRepository, terms: string[]): number {
    if (terms.length === 0) return 1;
    const fields = [
      [repository.normalizedName, 100],
      [repository.normalizedPath, 80],
      [repository.normalizedDescription, 30],
    ] as const;
    let total = 0;
    for (const term of terms) {
      const best = fields.reduce((current, [field, weight]) => {
        if (field.startsWith(term)) return Math.max(current, weight);
        if (field.includes(term)) return Math.max(current, weight - 20);
        if (isSubsequence(field, term)) return Math.max(current, weight - 45);
        return current;
      }, 0);
      if (!best) return 0;
      total += best;
    }
    if (repository.normalizedPath.includes(terms.join(" "))) total += 20;
    return total;
  }

  const results = $derived.by(() => {
    const { needle, topics } = parsedQuery;
    const topicMatches = indexedRepositories.filter((repository) =>
      topics.every((topic) => repository.normalizedTopics.includes(topic)),
    );

    if (!needle && topics.length === 0) {
      const byPath = new Map(
        indexedRepositories.map((repository) => [
          repository.path,
          repository.repository,
        ]),
      );
      const recent = recentPaths
        .map((path) => byPath.get(path))
        .filter((repository) => repository !== undefined);
      const recentIds = new Set(recent.map((repository) => repository.id));
      const matches = topicMatches
        .filter((entry) => !recentIds.has(entry.repository.id))
        .sort(
          (left, right) =>
            Number(right.repository.favorited) -
              Number(left.repository.favorited) ||
            right.updatedAt - left.updatedAt,
        )
        .map((entry) => entry.repository);
      return {
        recent,
        matches: matches.slice(0, resultLimit),
        matched: topicMatches.length,
      };
    }

    const recency = new Map(
      recentPaths.map((path, index) => [path, recentPaths.length - index]),
    );
    const ranked = topicMatches
      .map((repository) => {
        const base = score(repository, parsedQuery.terms);
        return {
          repository,
          rank:
            base === 0
              ? 0
              : base +
                (repository.repository.favorited ? 4 : 0) +
                (recency.get(repository.path) ?? 0),
        };
      })
      .filter((entry) => entry.rank > 0)
      .sort(
        (left, right) =>
          right.rank - left.rank ||
          right.repository.updatedAt - left.repository.updatedAt,
      );

    return {
      recent: [],
      matches: ranked
        .slice(0, resultLimit)
        .map((entry) => entry.repository.repository),
      matched: ranked.length,
    };
  });

  function actionScore(action: PaletteAction): number {
    if (parsedQuery.topics.length > 0 || parsedQuery.terms.length === 0)
      return 1;
    const label = action.label.toLowerCase();
    const aliases = action.keywords.toLowerCase();
    let total = 0;
    for (const term of parsedQuery.terms) {
      if (label.includes(term)) {
        total += label.startsWith(term) ? 100 : 80;
      } else if (aliases.includes(term)) {
        total += 35;
      } else {
        return 0;
      }
    }
    if (label.includes(parsedQuery.needle)) total += 25;
    return total;
  }

  function matches(action: PaletteAction): boolean {
    return actionScore(action) > 0;
  }

  function sortActions(actions: PaletteAction[]): PaletteAction[] {
    return actions
      .filter(matches)
      .sort((left, right) => actionScore(right) - actionScore(left));
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
        icon: Heart,
        keywords: "starred saved",
        run: () => void goto(`${resolve("/")}?tab=favorites`),
      },
    ];
    if (app.authStatus?.authenticated) {
      actions.push({
        id: "organizations",
        label: "Organizations",
        icon: Building2,
        keywords: "organization teams namespaces",
        run: () => void goto(resolve("/-/organizations")),
      });
      actions.push(
        ...accountSettingsSections.map(
          (section) =>
            ({
              id: `account-settings-${section.id}`,
              label: `Account settings · ${section.label}`,
              icon: section.icon,
              keywords: `account settings ${section.label} profile security`,
              run: () =>
                void goto(resolve("/-/account/[view]", { view: section.id })),
            }) satisfies PaletteAction,
        ),
      );
      if (app.authStatus.user?.is_admin) {
        actions.push(
          ...adminSettingsSections.map(
            (section) =>
              ({
                id: `admin-settings-${section.id}`,
                label: `Administration · ${section.label}`,
                icon: section.icon,
                keywords: `administration admin settings ${section.label} instance ${section.id === "storage" ? "migration migrations migrate move storage target" : section.id === "registry" ? "container registry images blobs manifests tags usage migration migrate storage" : ""}`,
                run: () =>
                  void goto(
                    resolve("/-/administration/[view]", { view: section.id }),
                  ),
              }) satisfies PaletteAction,
          ),
        );
      }
      const current = shell.activeRepository;
      if (current?.canManage) {
        actions.push(
          ...repositorySettingsSections
            .filter((section) => section.id !== "mirror" || current.mirrored)
            .map(
              (section) =>
                ({
                  id: `repository-settings-${section.id}`,
                  label: `${current.namespace}/${current.name} · Settings · ${section.label}`,
                  icon: section.icon,
                  keywords: `repository repo settings ${section.label}`,
                  run: () =>
                    void goto(
                      `${resolve("/[namespace]/[name]", {
                        namespace: current.namespace,
                        name: current.name,
                      })}?view=settings${
                        section.id === "general" ? "" : `&tab=${section.id}`
                      }`,
                    ),
                }) satisfies PaletteAction,
            ),
        );
      }
    }
    actions.push({
      id: "changelog",
      label: "Changelog",
      icon: ScrollText,
      keywords: "releases version updates",
      run: () => void goto(resolve("/changelog")),
    });
    return sortActions(actions);
  });

  const namespaceActions = $derived.by(() => {
    const namespace = page.params.namespace;
    if (!app.authStatus?.authenticated || !namespace || page.params.name)
      return [];
    const personal = namespace === viewer;
    const organization = app.organizations.find(
      (candidate) => candidate.slug === namespace,
    );
    const canManage = personal || organization?.role === "owner";
    const canViewMembers = personal || Boolean(organization);
    if (!canManage && !canViewMembers) return [];
    const views = [
      ...(canViewMembers
        ? [{ id: "members", label: "Members", icon: Building2 }]
        : []),
      ...(canManage
        ? [
            { id: "runners", label: "Runners", icon: ShieldCheck },
            { id: "integrations", label: "Integrations", icon: Settings2 },
            {
              id: "mirror-credentials",
              label: "Mirror credentials",
              icon: RefreshCw,
            },
            { id: "settings", label: "Settings", icon: Settings2 },
          ]
        : []),
    ];
    return sortActions(
      views.map(
        (view) =>
          ({
            id: `namespace-${view.id}`,
            label: `${namespace} · ${view.label}`,
            icon: view.icon,
            keywords: `organization namespace ${namespace} ${view.label}`,
            run: () =>
              void goto(`${resolve("/[namespace]", { namespace })}/${view.id}`),
          }) satisfies PaletteAction,
      ),
    );
  });

  const createActions = $derived.by(() => {
    if (!app.authStatus?.authenticated) return [];
    const actions: PaletteAction[] = [
      {
        id: "new-repository",
        label: "New repository",
        icon: Plus,
        keywords: "new create add repository repositories project",
        run: () => shell.openCreate("repository"),
      },
      {
        id: "new-organization",
        label: "New organization",
        icon: Building2,
        keywords: "new create add organization organizations team namespace",
        run: () => shell.openCreate("organization"),
      },
      {
        id: "import-repositories",
        label: "Import repositories",
        icon: Plus,
        keywords:
          "migration migrations migrate import imports transfer repositories github gitlab gitea forgejo",
        run: () => void goto(resolve("/imports/new")),
      },
    ];
    const current = shell.activeRepository;
    if (current?.canWrite && !current.mirrored) {
      actions.push(
        {
          id: "new-file",
          label: `New file in ${current.namespace}/${current.name}`,
          icon: FilePlus2,
          keywords: "new file files create edit commit code",
          run: () => shell.openFileCreate("file"),
        },
        {
          id: "new-directory",
          label: `New directory in ${current.namespace}/${current.name}`,
          icon: FolderPlus,
          keywords:
            "new directory directories folder folders create gitkeep commit code",
          run: () => shell.openFileCreate("directory"),
        },
      );
    }
    return sortActions(actions);
  });
  const repositoryRows = $derived(
    results.recent.length + results.matches.length,
  );
  const actionRowsCount = $derived(
    navigationActions.length + namespaceActions.length + createActions.length,
  );
  const hasResults = $derived(repositoryRows + actionRowsCount > 0);
  const countLabel = $derived.by(() => {
    const total = repositories.length;
    if (loading && total === 0) return "Loading…";
    if (query.trim()) return `${results.matched} of ${total}`;
    return `${total} ${total === 1 ? "repository" : "repositories"}`;
  });

  // Preloading the highlighted row means arrow keys warm the page up before
  // Enter is ever pressed.
  const rowIndex = $derived.by(() => {
    const index = new Map<string, Repository>();
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
    if (!repository) return;
    scheduleRepositoryPreload(
      repository.namespace,
      repository.name,
      scope,
      repository.default_branch ?? undefined,
    );
    return () =>
      cancelRepositoryPreload(repository.namespace, repository.name, scope);
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

  function openRepository(repository: Repository): void {
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

{#snippet repositoryRow(repository: Repository, group: string)}
  <Command.Item
    class={rowClass}
    value={`${group}:${repository.id}`}
    onSelect={() => openRepository(repository)}
    onmouseenter={() =>
      scheduleRepositoryPreload(
        repository.namespace,
        repository.name,
        scope,
        repository.default_branch ?? undefined,
      )}
    onmouseleave={() =>
      cancelRepositoryPreload(repository.namespace, repository.name, scope)}
  >
    <RepositoryIcon
      namespace={repository.namespace}
      name={repository.name}
      iconUpdatedAt={repository.icon_updated_at}
      class="size-6"
    />
    <span class="min-w-0 flex-1 truncate font-mono text-[0.8rem]">
      <span class="text-muted-foreground">{repository.namespace}/</span><span
        class="font-medium">{repository.name}</span
      >
    </span>
    {#if repository.mirrored}
      <span
        class="inline-flex shrink-0 items-center gap-1 rounded-full border border-sky-500/30 bg-sky-500/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-sky-600 dark:text-sky-400"
      >
        <RefreshCw class="size-2.5" />Mirror
      </span>
    {/if}
    {#if repository.favorited}
      <Heart class="size-3 shrink-0 fill-amber-400 text-amber-400" />
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
      class="font-mono text-base leading-none text-foreground/80"
    >
      /
    </span>
    <CommandPrimitive.Input
      bind:value={query}
      class="h-12 min-w-0 flex-1 bg-transparent font-mono text-sm text-foreground outline-none placeholder:font-sans placeholder:text-muted-foreground"
      placeholder="Search repositories or try topic:name"
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
            ? "Try a namespace, repository name, description, or topic:name."
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

      {#if repositoryRows && actionRowsCount}
        <Command.Separator />
      {/if}

      {#if navigationActions.length || namespaceActions.length}
        <Command.Group class={groupClass} heading="Go to">
          {@render actionRows(navigationActions)}
          {@render actionRows(namespaceActions)}
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
