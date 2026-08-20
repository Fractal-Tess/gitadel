<script lang="ts">
  import { GitBranch, Star } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
</script>

<div class="mb-5 flex flex-wrap items-center justify-end gap-2">
  <Button
    variant={state.repository?.favorited ? "secondary" : "outline"}
    size="sm"
    class="gap-2"
    disabled={state.favoritePending}
    onclick={() => void state.toggleFavorite()}
  >
    <Star
      class={state.repository?.favorited
        ? "size-3.5 fill-current text-amber-400"
        : "size-3.5"}
    />
    {state.repository?.favorited ? "Favorited" : "Favorite"}
  </Button>

  {#if state.refs?.branches.length}
    <Select.Root
      type="single"
      value={state.revision}
      onValueChange={(value) => {
        if (value && value !== state.revision) state.changeRevision(value);
      }}
    >
      <Select.Trigger class="h-9 w-auto min-w-32 gap-2 text-xs font-medium">
        <GitBranch class="size-3.5 text-muted-foreground" />
        {state.revision}
      </Select.Trigger>
      <Select.Content align="end">
        {#each state.refs.branches as branch (branch.name)}
          <Select.Item value={branch.name}>{branch.name}</Select.Item>
        {/each}
      </Select.Content>
    </Select.Root>
  {/if}
</div>
