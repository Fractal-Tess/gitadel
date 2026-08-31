<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";

  import RepositoryCommit from "$lib/components/repository/repository-commit.svelte";
  import RepositoryActions from "$lib/components/repository/repository-actions.svelte";
  import RepositoryHistory from "$lib/components/repository/repository-history.svelte";
  import RepositoryIssues from "$lib/components/repository/repository-issues.svelte";
  import RepositoryOverview from "$lib/components/repository/repository-overview.svelte";
  import RepositoryReleases from "$lib/components/repository/repository-releases.svelte";
  import RepositorySettings from "$lib/components/repository/repository-settings.svelte";
  import RepositorySidebar from "$lib/components/repository/repository-sidebar.svelte";
  import RepositoryTags from "$lib/components/repository/repository-tags.svelte";
  import RepositoryIntegrationConfigure from "$lib/components/repository/repository-integration-configure.svelte";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { recordRepositoryVisit } from "$lib/state/recent-repositories.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  const app = useAppState();
  const shell = useShellState();
  const state = $derived.by(() => {
    app.authorizationScope;
    return new RepositoryPageState(
      page.params.namespace ?? "",
      page.params.name ?? "",
      app,
    );
  });
  const inSettings = $derived(
    state.view === "settings" || state.view === "integrations",
  );

  $effect(() => {
    const current = state;
    void current.initialize();
    return () => current.destroy();
  });

  $effect(() => {
    const repository = state.repository;
    shell.setActiveRepository(
      repository
        ? {
            namespace: repository.namespace,
            name: repository.name,
            canManage: repository.can_manage,
            mirrored: repository.mirrored,
          }
        : null,
    );
    return () => shell.setActiveRepository(null);
  });

  // The palette leads with recently opened repositories, so every arrival here
  // counts regardless of whether it came from a link, the palette, or a URL.
  $effect(() => {
    if (state.repository) {
      recordRepositoryVisit(state.repository.namespace, state.repository.name);
    }
  });
</script>

<svelte:window onpopstate={() => state.restoreLocation()} />
<svelte:document onvisibilitychange={() => state.handleVisibilityChange()} />

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
  <Alert.Root
    class="mx-auto mt-16 max-w-lg text-center"
    variant="destructive"
  >
    <Alert.Title>Repository unavailable</Alert.Title>
    <Alert.Description>{state.error}</Alert.Description>
    <Button class="mt-5" variant="link" href={resolve("/")}>
      <ArrowLeft data-icon="inline-start" />Back to repositories
    </Button>
  </Alert.Root>
{:else if state.repository}
  <!-- On wide screens the page itself never scrolls: it fills the shell and each
       column owns its own scrollbar, so a short file tree stays on screen while
       a long file scrolls. Narrow screens keep one ordinary page scroll. -->
  <div class="flex flex-col xl:h-full xl:min-h-0">
    {#if state.error}
      <Alert.Root
        class="shrink-0 rounded-none border-x-0 border-t-0 px-5 py-3"
        variant="destructive"
      >
        <Alert.Title>Repository update failed</Alert.Title>
        <Alert.Description>{state.error}</Alert.Description>
      </Alert.Root>
    {/if}

    {#if inSettings}
      <!-- Settings is a form, not a browsing surface: it drops the metadata
           column and reads in the same narrow measure as account settings. -->
      <div
        class="min-w-0 xl:min-h-0 xl:flex-1 xl:overflow-y-auto xl:overscroll-contain"
      >
        <div class="mx-auto max-w-5xl px-5 py-8 lg:px-8">
          {#if state.view === "integrations" && state.integrationProvider}
            <RepositoryIntegrationConfigure {state} />
          {:else}
            <RepositorySettings {state} />
          {/if}
        </div>
      </div>
    {:else}
      <!-- The metadata column is a property of the repository, not of one view,
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
          {:else if state.view === "actions"}
            <RepositoryActions {state} />
          {:else if state.view === "tags"}
            <RepositoryTags {state} />
          {:else if state.view === "releases"}
            <RepositoryReleases {state} />
          {:else if state.view === "issues"}
            <RepositoryIssues {state} />
          {/if}
        </div>
        <RepositorySidebar {state} />
      </div>
    {/if}
  </div>
{/if}
