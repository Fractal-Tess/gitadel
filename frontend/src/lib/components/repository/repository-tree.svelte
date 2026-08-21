<script lang="ts">
  import { ChevronRight, GitBranch, LoaderCircle } from "lucide-svelte";
  import MaterialFileIcon from "$lib/components/repository/material-file-icon.svelte";

  import { formatSize } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import type { Tree } from "$lib/api.js";

  let { state }: { state: RepositoryPageState } = $props();
</script>

<aside class="min-w-0 border-b xl:border-b-0 xl:border-r">
  {#if state.repositoryTree}
    <header class="flex items-center justify-between border-b px-4 py-3">
      <span class="flex items-center gap-2 text-sm font-semibold">
        <GitBranch class="size-4 text-muted-foreground" />
        {state.repositoryTree.revision}
      </span>
      <code class="text-xs text-muted-foreground">
        {state.repositoryTree.commit_oid.slice(0, 8)}
      </code>
    </header>
    {@render entries(state.repositoryTree, 0)}
  {/if}
</aside>

{#snippet entries(tree: Tree, depth: number)}
  <ul class:border-t={depth > 0} class:divide-y={depth === 0}>
    {#each tree.entries as entry (entry.oid + entry.path)}
      <li>
        <button
          class={state.selectedPath === entry.path
            ? "group flex w-full items-center gap-2 bg-accent px-3 py-2.5 text-left text-foreground"
            : "group flex w-full items-center gap-2 px-3 py-2.5 text-left hover:bg-accent/55"}
          style={`padding-left:${0.75 + depth * 1.1}rem`}
          aria-expanded={entry.kind === "tree"
            ? state.expandedPaths.has(entry.path)
            : undefined}
          onclick={() => state.selectEntry(entry)}
        >
          {#if entry.kind === "tree"}
            <ChevronRight
              class={state.expandedPaths.has(entry.path)
                ? "size-3.5 shrink-0 rotate-90 text-muted-foreground transition-transform"
                : "size-3.5 shrink-0 text-muted-foreground transition-transform"}
            />
            <MaterialFileIcon
              name={entry.name}
              directory
              expanded={state.expandedPaths.has(entry.path)}
              class="size-4 shrink-0"
            />
          {:else}
            <span class="size-3.5 shrink-0"></span>
            <MaterialFileIcon name={entry.name} class="size-4 shrink-0" />
          {/if}
          <span class="min-w-0 flex-1 truncate text-sm">{entry.name}</span>
          {#if entry.kind !== "tree"}
            <span class="shrink-0 text-xs text-muted-foreground">
              {formatSize(entry.size)}
            </span>
          {/if}
        </button>

        {#if entry.kind === "tree" && state.expandedPaths.has(entry.path)}
          {#if state.expandedTrees[entry.path]}
            {@render entries(state.expandedTrees[entry.path], depth + 1)}
          {:else if state.loadingPaths.has(entry.path)}
            <div
              class="flex items-center gap-2 py-2 text-xs text-muted-foreground"
              style={`padding-left:${2.6 + depth * 1.1}rem`}
            >
              <LoaderCircle class="size-3 animate-spin" />Loading directory…
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
