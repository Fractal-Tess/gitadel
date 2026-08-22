<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { onMount } from "svelte";
  import { toast } from "svelte-sonner";
  import {
    Braces,
    Check,
    GitBranch,
    LockKeyhole,
    Star,
    X,
  } from "lucide-svelte";

  import RepositoryActivityChart from "$lib/components/repository/repository-activity-chart.svelte";
  import { Button } from "$lib/components/ui/button/index.js";

  import {
    ApiFailure,
    requestEmpty,
    type RepositoryOverviewItem,
  } from "$lib/api.js";
  import {
    invalidateExplore,
    peekExplore,
    refreshExplore,
  } from "$lib/navigation-cache.js";
  import { languageColor } from "$lib/repository/language-colors.js";
  import { preloadRepository } from "$lib/repository/repository-preload.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const viewer = app.authStatus?.user?.username;
  const repositoryPageSize = 20;
  const cachedExplore = peekExplore(1, repositoryPageSize, viewer);
  type CloneTarget = "ssh" | "http";
  let repositories = $state.raw<RepositoryOverviewItem[]>(
    cachedExplore?.repositories ?? [],
  );
  let nextPage = $state((cachedExplore?.page ?? 1) + 1);
  let hasNextPage = $state(cachedExplore?.has_next ?? true);
  let loadingMore = $state(false);
  let loadMoreError = $state<string | null>(null);
  let loadMoreQueued = false;
  let refreshingFirstPage = true;
  let activeLoadMore: Promise<boolean> | null = null;
  let loading = $state(!cachedExplore);
  let error = $state<string | null>(null);
  let favoriteError = $state<string | null>(null);
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

  function updatedAt(value: string): string {
    return new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
      year: "numeric",
    }).format(new Date(value));
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
          viewer,
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
      const overview = await refreshExplore(1, repositoryPageSize, viewer);
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
  }

  async function toggleFavorite(
    repository: RepositoryOverviewItem,
  ): Promise<void> {
    if (!app.authStatus?.authenticated) {
      window.location.assign(`/login?returnTo=${encodeURIComponent("/")}`);
      return;
    }
    const favorited = !repository.favorited;
    favoriteError = null;
    favoritePending = [...favoritePending, repository.id];
    try {
      await requestEmpty(
        `/api/v1/repositories/${encodeURIComponent(repository.namespace)}/${encodeURIComponent(repository.name)}/favorite`,
        { method: favorited ? "PUT" : "DELETE" },
      );
      invalidateExplore(viewer);
      repositories = repositories.map((item) =>
        item.id === repository.id ? { ...item, favorited } : item,
      );
    } catch (caught) {
      favoriteError = message(caught);
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
        const overview = await refreshExplore(1, repositoryPageSize, viewer);
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
  <title>{app.instance?.site_name ?? "Gitadel"} · Project archive</title>
  <meta
    name="description"
    content={app.instance?.site_description ??
      "A small Git server for projects worth keeping."}
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
          <Star class="size-5 fill-amber-400 text-amber-400" />Favorites
        {:else}
          <GitBranch class="size-5 text-muted-foreground" />Repositories
        {/if}
      </h1>
      <p class="mt-1.5 text-sm text-muted-foreground">
        {#if filter === "favorites"}
          Repositories you have starred.
        {:else if app.authStatus?.authenticated}
          Public repositories and projects shared with you.
        {:else}
          Public projects available on this server.
        {/if}
      </p>
    </div>
    <p class="text-xs tabular-nums text-muted-foreground">
      {visibleRepositories.length}
      {hasNextPage ? "loaded" : "total"}
    </p>
  </div>

  {#if search.trim()}
    <div class="mb-4 flex items-center gap-2 text-xs">
      <span class="text-muted-foreground">Filtered by</span>
      <a
        class="inline-flex items-center gap-1.5 rounded-md bg-secondary px-2 py-1 font-mono text-secondary-foreground hover:bg-secondary/80"
        href={filter === "favorites"
          ? `${resolve("/")}?tab=favorites`
          : resolve("/")}
      >
        {search}
        <X class="size-3" />
      </a>
    </div>
  {/if}

  {#if favoriteError}
    <p
      class="mb-3 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
    >
      {favoriteError}
    </p>
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
      <div
        class="rounded-md border border-destructive/30 bg-destructive/5 p-5 text-sm text-destructive"
      >
        {error}
      </div>
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
                  preloadRepository(
                    repository.namespace,
                    repository.name,
                    repository.default_branch,
                  )}
                onfocus={() =>
                  preloadRepository(
                    repository.namespace,
                    repository.name,
                    repository.default_branch,
                  )}
              >
                <div class="min-w-0 py-3">
                  <div class="flex items-center gap-2">
                    <h2 class="truncate text-sm font-semibold">
                      <span class="text-muted-foreground"
                        >{repository.namespace}/</span
                      >{repository.name}
                    </h2>
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
                            style:background={languageColor(language.language)}
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
                            style:background={languageColor(language.language)}
                          ></span>
                          {language.language}
                        </span>
                      {/each}
                    {/if}
                  </div>
                </div>
                <div class="pb-3 sm:col-start-2 sm:row-start-1 sm:py-3">
                  <RepositoryActivityChart activity={repository.activity} />
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
                  <Star
                    class={repository.favorited
                      ? "size-4 fill-amber-400 text-amber-400"
                      : "size-4"}
                  />
                </Button>
              </div>
            </li>
          {:else}
            <li class="grid place-items-center px-6 py-16 text-center">
              <GitBranch
                class="size-7 text-muted-foreground/60"
                strokeWidth={1.4}
              />
              <p class="mt-3 text-sm font-medium">
                {search ? "No matching repositories" : "No repositories yet"}
              </p>
              <p class="mt-1 text-xs text-muted-foreground">
                {search
                  ? "Try a different search."
                  : app.authStatus?.authenticated
                    ? "Use New repository above, then push your first commit over SSH."
                    : "Sign in to create a repository."}
              </p>
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
      <div class="py-4 text-center">
        <button
          class="text-xs text-destructive hover:underline"
          onclick={() => void loadMoreRepositories()}
        >
          {loadMoreError} Retry
        </button>
      </div>
    {/if}
  </div>
</section>
