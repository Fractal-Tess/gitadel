<script lang="ts">
  import { Code2, GitBranch, Settings, Star } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as ButtonGroup from "$lib/components/ui/button-group/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();

  const settingsActive = $derived(state.view === "settings");
</script>

<div
  class="mb-5 flex min-h-8 flex-wrap items-center gap-2 max-sm:min-h-11"
>
  {#if !settingsActive && state.refs?.branches.length}
    <Select.Root
      type="single"
      value={state.revision}
      onValueChange={(value) => {
        if (value && value !== state.revision) state.changeRevision(value);
      }}
    >
      <Select.Trigger
        class="w-auto min-w-32 gap-2 font-medium max-sm:h-11"
        aria-label="Switch branch"
      >
        <GitBranch class="size-3.5 text-muted-foreground" />
        {state.revision}
      </Select.Trigger>
      <Select.Content align="start">
        {#each state.refs.branches as branch (branch.name)}
          <Select.Item value={branch.name}>{branch.name}</Select.Item>
        {/each}
      </Select.Content>
    </Select.Root>
  {/if}

  <div class="ml-auto flex items-center gap-2">
    <Button
      variant={state.repository?.favorited ? "secondary" : "outline"}
      class="gap-2 max-sm:h-11"
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

    {#if state.repository?.can_manage}
      <ButtonGroup.Root>
        <Button
          variant={settingsActive ? "outline" : "secondary"}
          class="gap-2 max-sm:h-11"
          aria-current={settingsActive ? undefined : "page"}
          onclick={() => {
            if (settingsActive) state.navigate("overview");
          }}
        >
          <Code2 class="size-3.5" />Code
        </Button>
        <Button
          variant={settingsActive ? "secondary" : "outline"}
          class="gap-2 max-sm:h-11"
          aria-current={settingsActive ? "page" : undefined}
          onclick={() => {
            if (!settingsActive) state.navigate("settings");
          }}
        >
          <Settings class="size-3.5" />Settings
        </Button>
      </ButtonGroup.Root>
    {/if}
  </div>
</div>
