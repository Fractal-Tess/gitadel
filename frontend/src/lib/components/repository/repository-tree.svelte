<script lang="ts">
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import MaterialFileIcon from "$lib/components/repository/material-file-icon.svelte";

  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { formatDate, formatSize } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import type { Tree } from "$lib/api/repositories.js";

  let { state }: { state: RepositoryPageState } = $props();

  const relativeTime = new Intl.RelativeTimeFormat(undefined, {
    numeric: "always",
    style: "long",
  });

  function formatCommitAge(timestamp: number): string {
    const seconds = timestamp - Date.now() / 1000;
    const absoluteSeconds = Math.abs(seconds);
    const units: [Intl.RelativeTimeFormatUnit, number][] = [
      ["year", 365 * 24 * 60 * 60],
      ["month", 30 * 24 * 60 * 60],
      ["week", 7 * 24 * 60 * 60],
      ["day", 24 * 60 * 60],
      ["hour", 60 * 60],
      ["minute", 60],
      ["second", 1],
    ];
    const [unit, secondsPerUnit] =
      units.find(([, threshold]) => absoluteSeconds >= threshold) ??
      (["second", 1] as const);
    return relativeTime.format(Math.round(seconds / secondsPerUnit), unit);
  }
</script>

<!-- The right-hand divider is drawn by the pane resizer on wide layouts, so
     this column only owns the border it stacks with on narrow screens. -->
<aside
  class="flex min-w-0 flex-col border-b xl:h-full xl:min-h-0 xl:border-b-0"
>
  {#if state.browser.repositoryTree}
    {@const tree = state.browser.repositoryTree}
    <button
      type="button"
      class="group flex h-12 w-full shrink-0 items-center gap-3 border-b px-4 text-left font-medium hover:bg-accent/45 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
      aria-label={`View commit history for ${tree.revision}, updated ${formatCommitAge(tree.commit_timestamp)}`}
      onclick={() => state.navigate("history")}
    >
      <span class="flex min-w-0 flex-1 items-center gap-2 text-sm">
        <GitBranch class="size-4 shrink-0 text-muted-foreground" />
        <span class="truncate">{tree.revision}</span>
      </span>
      <span class="flex shrink-0 items-center gap-1.5 text-xs font-normal text-muted-foreground">
        <code>{tree.commit_oid.slice(0, 8)}</code>
        <span aria-hidden="true">·</span>
        <time
          datetime={new Date(tree.commit_timestamp * 1000).toISOString()}
          title={formatDate(tree.commit_timestamp)}
        >
          {formatCommitAge(tree.commit_timestamp)}
        </time>
      </span>
    </button>
    <!-- The history link stays pinned so it keeps forming the divider that runs
         under the app header while the entries scroll beneath it. -->
    <div class="min-h-0 flex-1 xl:overflow-y-auto xl:overscroll-contain">
      {@render entries(state.browser.repositoryTree, 0)}
    </div>
  {/if}
</aside>

{#snippet entries(tree: Tree, depth: number)}
  <ul class:border-t={depth > 0} class:divide-y={depth === 0}>
    {#each tree.entries as entry (entry.oid + entry.path)}
      <li>
        <button
          class={state.browser.selectedPath === entry.path
            ? "group flex w-full items-center gap-2 bg-accent px-3 py-2.5 text-left text-foreground"
            : "group flex w-full items-center gap-2 px-3 py-2.5 text-left hover:bg-accent/55"}
          style={`padding-left:${0.75 + depth * 1.1}rem`}
          aria-expanded={entry.kind === "tree"
            ? state.browser.expandedPaths.has(entry.path)
            : undefined}
          onclick={() => state.selectEntry(entry)}
        >
          {#if entry.kind === "tree"}
            <ChevronRight
              class={state.browser.expandedPaths.has(entry.path)
                ? "size-3.5 shrink-0 rotate-90 text-muted-foreground transition-transform"
                : "size-3.5 shrink-0 text-muted-foreground transition-transform"}
            />
            <MaterialFileIcon
              name={entry.path}
              directory
              expanded={state.browser.expandedPaths.has(entry.path)}
              class="size-4 shrink-0"
            />
          {:else}
            <span class="size-3.5 shrink-0"></span>
            <MaterialFileIcon name={entry.path} class="size-4 shrink-0" />
          {/if}
          <span class="min-w-0 flex-1 truncate text-sm">{entry.name}</span>
          {#if entry.kind !== "tree"}
            <span class="shrink-0 text-xs text-muted-foreground">
              {formatSize(entry.size)}
            </span>
          {/if}
        </button>

        {#if entry.kind === "tree" && state.browser.expandedPaths.has(entry.path)}
          {#if state.browser.expandedTrees[entry.path]}
            {@render entries(state.browser.expandedTrees[entry.path], depth + 1)}
          {:else if state.browser.loadingPaths.has(entry.path)}
            <div
              class="flex items-center gap-2 py-2 text-xs text-muted-foreground"
              style={`padding-left:${2.6 + depth * 1.1}rem`}
            >
              <Spinner class="size-3 animate-spin" />Loading directory…
            </div>
          {/if}
        {/if}
      </li>
    {:else}
      <li class="p-8 text-center text-sm text-muted-foreground">
        This directory is empty.
      </li>
    {/each}
  </ul>
{/snippet}
