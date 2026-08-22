<script lang="ts">
  import { ArrowLeft, Clock3, History } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import { dayHeading, dayKey, formatDate } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
</script>

<section>
  <button
    class="mb-7 inline-flex items-center gap-2 text-sm text-muted-foreground hover:text-foreground"
    onclick={() => state.navigate("overview")}
  >
    <ArrowLeft class="size-4" />Back to repository
  </button>
  <header class="mb-6 flex items-center gap-2">
    <History class="size-4 text-muted-foreground" />
    <h2 class="text-sm font-semibold">Commit history</h2>
    <span class="text-xs text-muted-foreground">
      Page {state.history?.page ?? state.historyPage} on {state.revision}
    </span>
  </header>
  <ol>
    {#each state.history?.commits ?? [] as item, index (item.oid)}
      {#if index === 0 || dayKey(item.committer.timestamp) !== dayKey(state.history?.commits[index - 1].committer.timestamp ?? 0)}
        <li
          class="mb-3 mt-7 text-xs font-medium tracking-[0.12em] text-muted-foreground first:mt-0"
        >
          {dayHeading(item.committer.timestamp)}
        </li>
      {/if}
      <li class="border-l border-border pl-5">
        <button
          class="group grid w-full gap-4 py-4 text-left sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center"
          onclick={() => state.navigate("commit", { oid: item.oid })}
        >
          <div class="min-w-0">
            <p class="truncate text-sm font-medium group-hover:underline">
              {item.title || "Untitled commit"}
            </p>
            <p class="mt-1.5 text-xs text-muted-foreground">
              <span class="font-medium text-foreground/80"
                >{item.author.name}</span
              >
              · {formatDate(item.committer.timestamp)}
            </p>
          </div>
          <code
            class="rounded border bg-card px-2.5 py-1.5 text-xs text-muted-foreground"
          >
            {item.short_oid}
          </code>
        </button>
      </li>
    {:else}
      <li class="py-16 text-center text-sm text-muted-foreground">
        No commits found.
      </li>
    {/each}
  </ol>

  {#if state.history && (state.history.page > 1 || state.history.has_next)}
    <footer class="mt-8 flex justify-between border-t pt-4">
      <Button
        variant="outline"
        size="sm"
        disabled={state.history.page <= 1}
        onclick={() =>
          state.navigate("history", { page: state.history!.page - 1 })}
      >
        Previous
      </Button>
      <Button
        variant="outline"
        size="sm"
        disabled={!state.history.has_next}
        onclick={() =>
          state.navigate("history", { page: state.history!.page + 1 })}
      >
        Next
      </Button>
    </footer>
  {/if}
</section>
