<script lang="ts">
  import { GitBranch, LockKeyhole, Star } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
</script>

<!-- Matches the height of the tree and content headers so the three columns
     share one unbroken divider under the app header. -->
<section
  class="flex min-h-12 shrink-0 flex-wrap items-center gap-2 border-b px-4 py-2"
>
  {#if state.view !== "overview" && state.view !== "settings"}
    <Select.Root
      type="single"
      value={state.revision}
      onValueChange={(value) => {
        if (value && value !== state.revision) state.changeRevision(value);
      }}
    >
      <Select.Trigger
        class="min-w-0 flex-1 shadow-none"
        aria-label="Switch branch"
      >
        <span class="flex min-w-0 items-center gap-2">
          <GitBranch class="size-3.5 shrink-0 text-muted-foreground" />
          <span class="truncate">{state.revision}</span>
        </span>
      </Select.Trigger>
      <Select.Content align="end">
        {#each state.refs?.branches ?? [] as branch (branch.name)}
          <Select.Item value={branch.name}>{branch.name}</Select.Item>
        {/each}
      </Select.Content>
    </Select.Root>
  {/if}

  {#if state.repository?.visibility === "private"}
    <span
      class="inline-flex items-center gap-1.5 rounded border px-2 py-1 text-xs font-medium text-muted-foreground"
    >
      <LockKeyhole class="size-3" />Private
    </span>
  {/if}
  {#if state.repository?.archived_at}
    <span
      class="rounded border px-2 py-1 text-xs font-medium text-muted-foreground"
    >
      Archived
    </span>
  {/if}

  <!-- A small button keeps this row inside the 3rem bar the tree and content
       headers use, so the divider under all three columns stays one line. -->
  <Button
    size="sm"
    variant={state.repository?.favorited ? "secondary" : "outline"}
    class="ml-auto gap-2 max-sm:h-11"
    aria-pressed={state.repository?.favorited ?? false}
    disabled={state.favoritePending}
    onclick={() => void state.toggleFavorite()}
  >
    <Star
      class={state.repository?.favorited
        ? "size-3.5 fill-current text-amber-400"
        : "size-3.5"}
    />
    <!-- The longest label sizes the button so toggling never shifts the row. -->
    <span class="grid justify-items-center">
      <span class="invisible col-start-1 row-start-1" aria-hidden="true">
        Favorited
      </span>
      <span class="col-start-1 row-start-1">
        {state.repository?.favorited ? "Favorited" : "Favorite"}
      </span>
    </span>
  </Button>
</section>
