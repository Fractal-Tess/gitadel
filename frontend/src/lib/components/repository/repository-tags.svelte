<script lang="ts">
  import { Tag } from "lucide-svelte";

  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
</script>

<section class="mx-auto max-w-5xl overflow-hidden rounded-md border bg-card/25">
  <header class="flex items-center gap-2 border-b px-5 py-3 text-sm font-semibold">
    <Tag class="size-4 text-muted-foreground" />Tags
  </header>
  <ul class="divide-y">
    {#each state.refs?.tags ?? [] as item (item.name)}
      <li class="flex items-center justify-between gap-4 px-5 py-4">
        <button
          class="font-mono text-sm font-medium hover:underline"
          onclick={() => state.changeRevision(item.name)}>{item.name}</button
        >
        <code class="text-xs text-muted-foreground">{item.oid.slice(0, 12)}</code>
      </li>
    {:else}
      <li class="p-10 text-center text-sm text-muted-foreground">
        No tags in this repository.
      </li>
    {/each}
  </ul>
</section>
