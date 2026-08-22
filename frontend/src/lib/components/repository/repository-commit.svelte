<script lang="ts">
  import { ArrowLeft, GitBranch, GitCommitHorizontal } from "lucide-svelte";

  import PierreDiff from "$lib/components/repository/pierre-diff.svelte";
  import { formatDate } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
</script>

{#if state.commit}
  <div class="grid gap-7 lg:grid-cols-[minmax(0,1fr)_15rem]">
    <div class="min-w-0">
      <button
        class="mb-7 inline-flex items-center gap-2 text-sm text-muted-foreground hover:text-foreground"
        onclick={() => state.navigate("history")}
      >
        <ArrowLeft class="size-4" />Back to commits
      </button>
      <header class="border-b pb-5">
        <h2 class="text-2xl font-semibold tracking-tight">{state.commit.title || "Untitled commit"}</h2>
        <div class="mt-3 flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
          <span class="font-medium text-foreground">{state.commit.author.name}</span>
          <span>·</span>
          <span>{formatDate(state.commit.committer.timestamp)}</span>
          <span>·</span>
          <span class="inline-flex items-center gap-1.5"><GitBranch class="size-3.5" />{state.revision}</span>
        </div>
        {#if state.commit.message !== state.commit.title}
          <pre class="mt-5 whitespace-pre-wrap border-t pt-4 font-sans text-sm leading-6 text-foreground/80">{state.commit.message}</pre>
        {/if}
      </header>

      <section class="mt-6">
        <header class="mb-3 flex items-center gap-2 text-sm font-semibold">
          <GitCommitHorizontal class="size-4 text-muted-foreground" />Changes
        </header>
        {#if state.diff?.patch}
          {#key state.commit.oid}
            <PierreDiff patch={state.diff.patch} cacheKey={state.commit.oid} />
          {/key}
        {:else}
          <div class="rounded-md border bg-card/25 p-10 text-center text-sm text-muted-foreground">No textual changes.</div>
        {/if}
        {#if state.diff?.truncated}
          <p class="mt-4 rounded-md border border-amber-500/20 bg-amber-500/10 px-4 py-3 text-xs text-amber-800 dark:text-amber-200">This large diff was truncated; some files or changes are not shown.</p>
        {/if}
      </section>
    </div>

    <aside class="space-y-6 border-l pl-5">
      <div>
        <p class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">Commit</p>
        <code class="mt-3 block break-all text-xs">{state.commit.short_oid}</code>
      </div>
      <div>
        <p class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">Author</p>
        <div class="mt-3 rounded-md border bg-card/25 p-3">
          <p class="text-sm font-medium">{state.commit.author.name}</p>
          <p class="mt-1 truncate text-xs text-muted-foreground">{state.commit.author.email}</p>
        </div>
      </div>
      <div>
        <p class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">Parents</p>
        <p class="mt-3 text-sm">{state.commit.parents.length}</p>
      </div>
    </aside>
  </div>
{/if}
