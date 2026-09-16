<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { onMount, untrack } from "svelte";
  import { toast } from "svelte-sonner";
  import Braces from "@lucide/svelte/icons/braces";
  import Building2 from "@lucide/svelte/icons/building-2";
  import Check from "@lucide/svelte/icons/check";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import Heart from "@lucide/svelte/icons/heart";
  import LockKeyhole from "@lucide/svelte/icons/lock-keyhole";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import X from "@lucide/svelte/icons/x";
  import RepositoryActivityChart from "$lib/components/repository/repository-activity-chart.svelte";
  import RepositoryIcon from "$lib/components/repository/repository-icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as Empty from "$lib/components/ui/empty/index.js";

  import { ApiFailure, requestEmpty } from "$lib/api/transport.js";
  import type { RepositoryOverviewItem } from "$lib/api/repositories.js";
  import {
    invalidateExplore,
    peekExplore,
    refreshExplore,
  } from "$lib/navigation-cache.js";
  import { languageColor } from "$lib/repository/language-colors.js";
  import {
    cancelRepositoryPreload,
    scheduleRepositoryPreload,
  } from "$lib/repository/repository-preload.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  let {
    namespace: namespaceProp = null,
    manageHref = null,
  }: {
    namespace?: string | null;
    manageHref?: string | null;
  } = $props();
  const namespace = untrack(() => namespaceProp);
  const app = useAppState();
  const viewer = app.authStatus?.user?.username;
  const scope = $derived(app.authorizationScope);
  const repositoryPageSize = 20;
  const initialExplore = untrack(() =>
    peekExplore(1, repositoryPageSize, app.authorizationScope, namespace),
  );
  const cachedExplore = $derived(
    peekExplore(1, repositoryPageSize, scope, namespace),
  );
  const listHref = namespace
    ? resolve("/[namespace]", { namespace })
    : resolve("/");
  const organization = $derived(
    app.organizations.find((candidate) => candidate.slug === namespace) ?? null,
  );
  const namespaceTitle = $derived(
    organization?.display_name ?? namespace ?? "",
  );
  type CloneTarget = "ssh" | "http";
  let repositories = $state.raw<RepositoryOverviewItem[]>(
    initialExplore?.repositories ?? [],
  );
  let nextPage = $state((initialExplore?.page ?? 1) + 1);
  let hasNextPage = $state(initialExplore?.has_next ?? true);
  let loadingMore = $state(false);
  let loadMoreError = $state<string | null>(null);
  let loadMoreQueued = false;
  let refreshingFirstPage = true;
  let activeLoadMore: Promise<boolean> | null = null;
  let loading = $state(!initialExplore);
  let error = $state<string | null>(null);
  let favoritePending = $state.raw<string[]>([]);
  let copied = $state<string | null>(null);

  // Both live in the URL so the rail can link to them and so a filtered view
  // stays shareable.
  const search = $derived(page.url.searchParams.get("q") ?? "");
  const filter = $derived(
    page.url.searchParams.get("tab") === "favorites" ? "favorites" : "all",
  );

  let visibleRepositories = $derived.by(() => {
    const query = search.trim().toLowerCase();
    let filtered =
      filter === "favorites"
        ? repositories.filter((repository) => repository.favorited)
        : repositories;
    if (query) {
      filtered = filtered.filter((repository) =>
        `${repository.namespace}/${repository.name} ${repository.description ?? ""}`
          .toLowerCase()
          .includes(query),
      );
    }
    return filtered;
  });

  function message(caught: unknown): string {
    if (caught instanceof ApiFailure || caught instanceof Error)
      return caught.message;
    return "Could not load repositories.";
  }

  const dateFormatter = new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });

  function updatedAt(value: string): string {
    return dateFormatter.format(new Date(value));
  }

  function lineCount(value: number): string {
    return value.toLocaleString();
  }

  function loadMoreRepositories() {
    if (loading || refreshingFirstPage || !hasNextPage) {
      return Promise.resolve(false);
    }
    if (activeLoadMore) return activeLoadMore;

    loadingMore = true;
    loadMoreError = null;
    activeLoadMore = (async () => {
      try {
        const overview = await refreshExplore(
          nextPage,
          repositoryPageSize,
          scope,
          namespace,
        );
        const loadedIds = new Set(
          repositories.map((repository) => repository.id),
        );
        repositories = [
          ...repositories,
          ...overview.repositories.filter(
            (repository) => !loadedIds.has(repository.id),
          ),
        ];
        nextPage = overview.page + 1;
        hasNextPage = overview.has_next;
        return true;
      } catch (caught) {
        loadMoreError = message(caught);
        return false;
      } finally {
        loadingMore = false;
        activeLoadMore = null;
      }
    })();
    return activeLoadMore;
  }

  async function loadAllRepositories() {
    while (hasNextPage) {
      if (!(await loadMoreRepositories())) break;
    }
  }

  async function refreshVisibleRepositories() {
    if (refreshingFirstPage) return;
    refreshingFirstPage = true;
    loadMoreError = null;
    try {
      const overview = await refreshExplore(
        1,
        repositoryPageSize,
        scope,
        namespace,
      );
      repositories = overview.repositories;
      nextPage = overview.page + 1;
      hasNextPage = overview.has_next;
      error = null;
    } catch (caught) {
      loadMoreError = message(caught);
    } finally {
      refreshingFirstPage = false;
      if (search.trim() || filter === "favorites") {
        void loadAllRepositories();
      }
    }
  }

  function observeLoadMore(element: HTMLDivElement) {
    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((entry) => entry.isIntersecting)) return;
        if (loading || refreshingFirstPage) {
          loadMoreQueued = true;
          return;
        }
        void loadMoreRepositories();
      },
      {
        // The app shell owns the only scroll container on the page.
        root: element.closest("[data-scroll-region]"),
        rootMargin: "320px 0px",
      },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }

  function cloneUrl(
    repository: RepositoryOverviewItem,
    target: CloneTarget,
  ): string {
    if (target === "ssh") return repository.ssh_clone_url;
    return typeof window === "undefined"
      ? ""
      : `${window.location.origin}/${repository.namespace}/${repository.name}.git`;
  }

  async function copyCloneUrl(
    repository: RepositoryOverviewItem,
    target: CloneTarget,
  ): Promise<void> {
    const url = cloneUrl(repository, target);
    try {
      // A self-hosted instance is often reached over plain HTTP, where
      // navigator.clipboard is undefined, so fall back to a selection copy.
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(url);
      } else {
        const carrier = document.createElement("textarea");
        carrier.value = url;
        carrier.setAttribute("readonly", "");
        carrier.style.position = "fixed";
        carrier.style.opacity = "0";
        document.body.append(carrier);
        carrier.select();
        document.execCommand("copy");
        carrier.remove();
      }

      toast.success(`${target.toUpperCase()} clone URL copied`, {
        description: url,
      });

      const token = `${repository.id}:${target}`;
      copied = token;
      window.setTimeout(() => {
        if (copied === token) copied = null;
      }, 1600);
    } catch {
      toast.error("The clone URL could not be copied.");
    }
  }

  async function toggleFavorite(
    repository: RepositoryOverviewItem,
  ): Promise<void> {
    if (!app.authStatus?.authenticated) {
      window.location.assign(`/login?returnTo=${encodeURIComponent("/")}`);
      return;
    }
    const favorited = !repository.favorited;
    favoritePending = [...favoritePending, repository.id];
    try {
      await requestEmpty(
        `/api/v1/repositories/${encodeURIComponent(repository.namespace)}/${encodeURIComponent(repository.name)}/favorite`,
        { method: favorited ? "PUT" : "DELETE" },
      );
      invalidateExplore(scope);
      repositories = repositories.map((item) =>
        item.id === repository.id ? { ...item, favorited } : item,
      );
    } catch (caught) {
      toast.error(message(caught));
    } finally {
      favoritePending = favoritePending.filter((id) => id !== repository.id);
    }
  }

  // Filtering happens client-side, so a narrowed view needs every page loaded
  // before it can claim to be complete.
  $effect(() => {
    if (loading) return;
    if (!search.trim() && filter !== "favorites") return;
    void loadAllRepositories();
  });

  onMount(() => {
    void (async () => {
      try {
        const overview = await refreshExplore(
          1,
          repositoryPageSize,
          scope,
          namespace,
        );
        repositories = overview.repositories;
        nextPage = overview.page + 1;
        hasNextPage = overview.has_next;
      } catch (caught) {
        if (cachedExplore) loadMoreError = message(caught);
        else error = message(caught);
      } finally {
        loading = false;
        refreshingFirstPage = false;
        const shouldLoadMore = loadMoreQueued;
        loadMoreQueued = false;
        if (search.trim() || filter === "favorites") {
          void loadAllRepositories();
        } else if (shouldLoadMore) {
          void loadMoreRepositories();
        }
      }
    })();
  });
</script>

<svelte:window onfocus={() => void refreshVisibleRepositories()} />

<svelte:head>
  <title>
    {namespace
      ? `${namespaceTitle} · ${app.instance?.site_name ?? "Gitadel"}`
      : `${app.instance?.site_name ?? "Gitadel"} · Project archive`}
  </title>
  <meta
    name="description"
    content={namespace
      ? `Repositories in the ${namespaceTitle} namespace.`
      : (app.instance?.site_description ??
        "A small Git server for projects worth keeping.")}
  />
</svelte:head>

<section
  id="repositories"
  class="mx-auto max-w-5xl px-5 py-8 lg:px-8"
  aria-labelledby="repositories-heading"
>
  <div class="mb-5 flex flex-wrap items-end justify-between gap-4">
    <div>
      <h1
        id="repositories-heading"
        class="flex items-center gap-3 text-lg font-semibold tracking-tight"
      >
        {#if filter === "favorites"}
          <Heart class="size-5 fill-amber-400 text-amber-400" />Favorites
        {:else if organization}
          <Building2 class="size-5 text-muted-foreground" />{namespaceTitle}
        {:else if namespace}
          <GitBranch class="size-5 text-muted-foreground" />{namespaceTitle}
        {:else}
          <GitBranch class="size-5 text-muted-foreground" />Repositories
        {/if}
      </h1>
      <p class="mt-1.5 text-sm text-muted-foreground">
        {#if filter === "favorites"}
          Repositories you have starred.
        {:else if organization}
          Repositories owned by {namespaceTitle}.
        {:else if namespace}
          Repositories in the <span class="font-mono">{namespace}</span> namespace.
        {:else if app.authStatus?.authenticated}
          Public repositories and projects shared with you.
        {:else}
          Public projects available on this server.
        {/if}
      </p>
    </div>
    <div class="flex items-center gap-3">
      <p class="text-xs tabular-nums text-muted-foreground">
        {visibleRepositories.length}
        {hasNextPage ? "loaded" : "total"}
      </p>
      {#if manageHref}
        <Button variant="outline" size="sm" href={manageHref}>
          <Settings2 data-icon="inline-start" />Manage
        </Button>
      {/if}
    </div>
  </div>

  {#if search.trim()}
    <div class="mb-4 flex items-center gap-2 text-xs">
      <span class="text-muted-foreground">Filtered by</span>
      <a
        class="inline-flex items-center gap-1.5 rounded-md bg-secondary px-2 py-1 font-mono text-secondary-foreground hover:bg-secondary/80"
        href={filter === "favorites" ? `${listHref}?tab=favorites` : listHref}
      >
        {search}
        <X class="size-3" />
      </a>
    </div>
  {/if}

  <div>
    {#if loading}
      <div
        class="overflow-hidden rounded-md border bg-card"
        aria-label="Loading repositories"
      >
        {#each Array(6) as _, index (index)}
          <div
            class="h-28 animate-pulse border-b bg-muted/20 last:border-b-0"
          ></div>
        {/each}
      </div>
    {:else if error}
      <Alert.Root variant="destructive">
        <Alert.Title>Repositories unavailable</Alert.Title>
        <Alert.Description>{error}</Alert.Description>
      </Alert.Root>
    {:else}
      <div class="overflow-hidden rounded-md border bg-card/35">
        <ul class="divide-y">
          {#each visibleRepositories as repository (repository.id)}
            <li class="relative">
              <a
                class="group grid min-h-28 grid-cols-[minmax(0,1fr)] items-center gap-x-3 px-4 pr-20 hover:bg-accent/55 sm:grid-cols-[minmax(0,1fr)_11rem] lg:grid-cols-[minmax(0,1fr)_12rem]"
                href={resolve("/[namespace]/[name]", {
                  namespace: repository.namespace,
                  name: repository.name,
                })}
                onpointerenter={() =>
                  scheduleRepositoryPreload(
                    repository.namespace,
                    repository.name,
                    scope,
                    repository.default_branch,
                  )}
                onpointerleave={() =>
                  cancelRepositoryPreload(
                    repository.namespace,
                    repository.name,
                    scope,
                  )}
                onfocus={() =>
                  scheduleRepositoryPreload(
                    repository.namespace,
                    repository.name,
                    scope,
                    repository.default_branch,
                  )}
                onblur={() =>
                  cancelRepositoryPreload(
                    repository.namespace,
                    repository.name,
                    scope,
                  )}
              >
                <div class="flex min-w-0 items-start gap-3 py-3">
                  <RepositoryIcon
                    namespace={repository.namespace}
                    name={repository.name}
                    iconUpdatedAt={repository.icon_updated_at}
                    class="mt-0.5 size-9"
                  />
                  <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-2">
                      <h2 class="min-w-0 truncate text-sm font-semibold">
                        <span class="text-muted-foreground"
                          >{repository.namespace}/</span
                        >{repository.name}
                      </h2>
                      {#if repository.mirrored}
                        <span
                          class="inline-flex items-center gap-1 rounded-full border border-sky-500/30 bg-sky-500/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-sky-600 dark:text-sky-400"
                        >
                          <RefreshCw class="size-2.5" />Mirror
                        </span>
                      {/if}
                      {#if repository.visibility === "private"}
                        <span
                          class="inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground"
                          ><LockKeyhole class="size-2.5" /> Private</span
                        >
                      {/if}
                      {#if repository.archived_at}
                        <span
                          class="rounded border px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground"
                          >Archived</span
                        >
                      {/if}
                    </div>
                    <p class="mt-1 truncate text-sm text-muted-foreground">
                      {repository.description ?? "No description provided."}
                    </p>
                    <div
                      class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground"
                    >
                      <span class="inline-flex items-center gap-1.5">
                        <GitBranch class="size-3.5" />
                        {repository.branch_count} branch{repository.branch_count ===
                        1
                          ? ""
                          : "es"}
                      </span>
                      <span class="inline-flex items-center gap-1.5">
                        <Braces class="size-3.5" />
                        {lineCount(repository.total_lines)} lines
                      </span>
                      {#if repository.languages.length}
                        <span
                          class="flex h-1.5 w-16 overflow-hidden rounded-full bg-muted"
                          aria-hidden="true"
                        >
                          {#each repository.languages as language (language.language)}
                            <span
                              style:width={`${repository.total_lines ? (language.lines / repository.total_lines) * 100 : 0}%`}
                              style:background={languageColor(
                                language.language,
                              )}
                            ></span>
                          {/each}
                        </span>
                        {#each repository.languages as language (language.language)}
                          <span
                            class="inline-flex items-center gap-1"
                            title={`${lineCount(language.lines)} lines`}
                          >
                            <span
                              class="size-1.5 rounded-full"
                              style:background={languageColor(
                                language.language,
                              )}
                            ></span>
                            {language.language}
                          </span>
                        {/each}
                      {/if}
                    </div>
                  </div>
                </div>
                <div class="pb-3 sm:col-start-2 sm:row-start-1 sm:py-3">
                  <RepositoryActivityChart
                    activity={repository.activity}
                    commitCount={repository.commit_count}
                  />
                  <p
                    class="mt-0.5 text-right text-[10px] text-muted-foreground"
                  >
                    Updated {updatedAt(repository.updated_at)}
                  </p>
                </div>
              </a>
              <!-- A flush rail rather than floating buttons: it fills
                             the row edge to edge, so the three actions read as
                             part of the card instead of hovering over it. -->
              <div class="absolute inset-y-0 right-0 z-10 flex w-16 flex-col">
                {#each ["ssh", "http"] as const as target, index (target)}
                  <Button
                    variant="ghost"
                    size="xs"
                    class="h-auto w-full flex-1 rounded-none border-l-border/60 font-mono text-[11px] tracking-widest text-muted-foreground uppercase hover:text-foreground {index >
                    0
                      ? 'border-t-border/60'
                      : ''}"
                    onclick={() => void copyCloneUrl(repository, target)}
                    aria-label={`Copy ${target.toUpperCase()} clone URL for ${repository.namespace}/${repository.name}`}
                    title={cloneUrl(repository, target)}
                  >
                    {#if copied === `${repository.id}:${target}`}
                      <Check class="size-3.5 text-emerald-500" />
                    {:else}
                      {target}
                    {/if}
                  </Button>
                {/each}
                <Button
                  variant="ghost"
                  size="xs"
                  class="h-auto w-full flex-1 rounded-none border-t-border/60 border-l-border/60 text-muted-foreground hover:text-amber-500"
                  onclick={() => void toggleFavorite(repository)}
                  disabled={favoritePending.includes(repository.id)}
                  aria-label={repository.favorited
                    ? `Unfavorite ${repository.namespace}/${repository.name}`
                    : `Favorite ${repository.namespace}/${repository.name}`}
                  title={repository.favorited
                    ? "Remove from favorites"
                    : "Add to favorites"}
                >
                  <Heart
                    class={repository.favorited
                      ? "size-4 fill-amber-400 text-amber-400"
                      : "size-4"}
                  />
                </Button>
              </div>
            </li>
          {:else}
            <li>
              <Empty.Root class="border-0 py-16">
                <Empty.Header>
                  <Empty.Media variant="icon">
                    <GitBranch />
                  </Empty.Media>
                  <Empty.Title>
                    {search
                      ? "No matching repositories"
                      : "No repositories yet"}
                  </Empty.Title>
                  <Empty.Description>
                    {search
                      ? "Try a different search."
                      : namespace
                        ? "No repositories are visible in this namespace."
                        : app.authStatus?.authenticated
                          ? "Use New above, then push your first commit over SSH."
                          : "Sign in to create a repository."}
                  </Empty.Description>
                </Empty.Header>
              </Empty.Root>
            </li>
          {/each}
        </ul>
      </div>
    {/if}

    <div class="h-px" aria-hidden="true" {@attach observeLoadMore}></div>
    {#if loadingMore}
      <p class="py-5 text-center text-xs text-muted-foreground">
        Loading more repositories…
      </p>
    {:else if loadMoreError}
      <Alert.Root variant="destructive">
        <Alert.Title>More repositories unavailable</Alert.Title>
        <Alert.Description>{loadMoreError}</Alert.Description>
        <Button
          type="button"
          size="sm"
          variant="link"
          onclick={() => void loadMoreRepositories()}>Retry</Button
        >
      </Alert.Root>
    {/if}
  </div>
</section>
