<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { ArrowLeft, FileCode2, History, Settings, Tag } from "lucide-svelte";

  import RepositoryCommit from "$lib/components/repository/repository-commit.svelte";
  import RepositoryHistory from "$lib/components/repository/repository-history.svelte";
  import RepositoryOverview from "$lib/components/repository/repository-overview.svelte";
  import RepositorySettings from "$lib/components/repository/repository-settings.svelte";
  import RepositorySidebar from "$lib/components/repository/repository-sidebar.svelte";
  import RepositoryTags from "$lib/components/repository/repository-tags.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { recordRepositoryVisit } from "$lib/state/recent-repositories.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  const shell = useShellState();
  const state = $derived(
    new RepositoryPageState(
      page.params.namespace ?? "",
      page.params.name ?? "",
    ),
  );

  $effect(() => {
    const current = state;
    void current.initialize();
    return () => current.destroy();
  });

  // The palette leads with recently opened repositories, so every arrival here
  // counts regardless of whether it came from a link, the palette, or a URL.
  $effect(() => {
    if (state.repository) {
      recordRepositoryVisit(state.repository.namespace, state.repository.name);
    }
  });

  // The rail is the only place repository sections live, so republish them
  // whenever the active view or the viewer's permissions change.
  $effect(() => {
    if (!state.repository) return;
    return shell.publishNavGroup({
      label: "Repository",
      items: [
        {
          id: "overview",
          label: "Code",
          icon: FileCode2,
          active: state.view === "overview",
          select: () => state.navigate("overview"),
        },
        {
          id: "history",
          label: "History",
          icon: History,
          active: state.view === "history" || state.view === "commit",
          select: () => state.navigate("history"),
        },
        {
          id: "tags",
          label: "Tags",
          icon: Tag,
          active: state.view === "tags",
          select: () => state.navigate("tags"),
        },
        ...(state.repository.can_manage
          ? [
              {
                id: "settings",
                label: "Settings",
                icon: Settings,
                active: state.view === "settings",
                select: () => state.navigate("settings"),
              },
            ]
          : []),
      ],
    });
  });
</script>

<svelte:window onpopstate={() => state.restoreLocation()} />

<svelte:head>
  <title>
    {state.repository
      ? `${state.repository.namespace}/${state.repository.name}`
      : "Repository"} · Gitadel
  </title>
  <meta
    name="description"
    content={state.repository?.description ??
      "Browse this Git repository on Gitadel."}
  />
</svelte:head>

<!-- The repository browser is full-bleed so its columns meet the rail and the
     header directly instead of floating in a centered block, which reads as
     off-centre once the rail takes width off one side. -->
{#if state.loading && !state.repository}
  <div class="mx-auto max-w-xl px-5 py-16 text-center">
    <p class="text-sm text-muted-foreground">Opening repository…</p>
  </div>
{:else if state.error && !state.repository}
  <div
    class="mx-auto mt-16 max-w-lg rounded-lg border border-destructive/30 bg-destructive/5 p-6 text-center"
  >
    <p class="font-medium">Repository unavailable</p>
    <p class="mt-2 text-sm text-destructive">{state.error}</p>
    <Button class="mt-5 gap-2" variant="link" href={resolve("/")}>
      <ArrowLeft class="size-4" />Back to repositories
    </Button>
  </div>
{:else if state.repository}
  <!-- On wide screens the page itself never scrolls: it fills the shell and each
       column owns its own scrollbar, so a short file tree stays on screen while
       a long file scrolls. Narrow screens keep one ordinary page scroll. -->
  <div class="flex flex-col xl:h-full xl:min-h-0">
    {#if state.error}
      <div
        class="shrink-0 border-b border-destructive/30 bg-destructive/5 px-5 py-3 text-sm text-destructive"
        role="alert"
      >
        {state.error}
      </div>
    {/if}
    {#if state.notice}
      <div
        class="shrink-0 border-b bg-muted/60 px-5 py-3 text-sm"
        role="status"
        aria-live="polite"
      >
        {state.notice}
      </div>
    {/if}

    <!-- The metadata column is a property of the repository, not of one tab,
         so it lives here and stays put while the view changes. -->
    <div class="grid xl:min-h-0 xl:flex-1 xl:grid-cols-[minmax(0,1fr)_18rem]">
      <!-- Only the overview draws its own edge-to-edge columns and scrollers;
           the other views are ordinary documents that need the page padding
           back and scroll as a single block. -->
      <div
        class={state.view === "overview"
          ? "min-w-0 xl:min-h-0"
          : "min-w-0 px-5 py-6 xl:min-h-0 xl:overflow-y-auto xl:overscroll-contain"}
      >
        {#if state.view === "overview"}
          <RepositoryOverview {state} />
        {:else if state.view === "history"}
          <RepositoryHistory {state} />
        {:else if state.view === "commit"}
          <RepositoryCommit {state} />
        {:else if state.view === "tags"}
          <RepositoryTags {state} />
        {:else if state.view === "settings"}
          <RepositorySettings {state} />
        {/if}
      </div>
      <RepositorySidebar {state} />
    </div>
  </div>
{/if}
