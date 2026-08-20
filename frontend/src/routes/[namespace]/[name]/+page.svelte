<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { ArrowLeft } from "lucide-svelte";

  import RepositoryCommit from "$lib/components/repository/repository-commit.svelte";
  import RepositoryHeader from "$lib/components/repository/repository-header.svelte";
  import RepositoryHistory from "$lib/components/repository/repository-history.svelte";
  import RepositoryOverview from "$lib/components/repository/repository-overview.svelte";
  import RepositoryTags from "$lib/components/repository/repository-tags.svelte";
  import RepositoryToolbar from "$lib/components/repository/repository-toolbar.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  const state = new RepositoryPageState(
    page.params.namespace ?? "",
    page.params.name ?? "",
  );

  $effect(() => {
    void state.initialize();
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

<div class="min-h-screen bg-background">
  <RepositoryHeader {state} />

  <main class="mx-auto max-w-[96rem] px-5 py-6">
    {#if state.loading && !state.repository}
      <div class="mx-auto max-w-xl py-16 text-center">
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
      <RepositoryToolbar {state} />

      {#if state.error}
        <div
          class="mb-5 rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive"
        >
          {state.error}
        </div>
      {/if}

      {#if state.view === "overview"}
        <RepositoryOverview {state} />
      {:else if state.view === "history"}
        <RepositoryHistory {state} />
      {:else if state.view === "commit"}
        <RepositoryCommit {state} />
      {:else if state.view === "tags"}
        <RepositoryTags {state} />
      {/if}
    {/if}
  </main>
</div>
