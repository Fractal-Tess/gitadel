<script lang="ts">
  import { GitBranch } from "lucide-svelte";

  import RepositoryContent from "$lib/components/repository/repository-content.svelte";
  import RepositorySidebar from "$lib/components/repository/repository-sidebar.svelte";
  import RepositoryTree from "$lib/components/repository/repository-tree.svelte";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
</script>

{#if state.emptyRepository}
  <section
    class="grid min-h-72 place-items-center rounded-lg border bg-card p-8 text-center shadow-sm"
  >
    <div>
      <GitBranch class="mx-auto size-9 text-muted-foreground" strokeWidth={1.4} />
      <h2 class="mt-4 font-semibold">This repository is empty</h2>
      <p class="mt-2 text-sm text-muted-foreground">
        Push the first commit over SSH to begin the archive.
      </p>
    </div>
  </section>
{:else}
  <div
    class="grid overflow-hidden rounded-md border bg-card/20 xl:grid-cols-[18rem_minmax(0,1fr)_18rem]"
  >
    <RepositoryTree {state} />
    <RepositoryContent {state} />
    <RepositorySidebar {state} />
  </div>
{/if}
