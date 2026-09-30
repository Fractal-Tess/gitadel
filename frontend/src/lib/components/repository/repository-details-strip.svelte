<!--
  What stays of the details panel while it is hidden: a slim column with the
  button that brings it back, and the favorite toggle, so the one action people
  reach for there is still a click away.
-->
<script lang="ts">
  import Heart from "@lucide/svelte/icons/heart";
  import PanelRightOpen from "@lucide/svelte/icons/panel-right-open";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Tooltip from "$lib/components/ui/tooltip/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  let { state: pageState }: { state: RepositoryPageState } = $props();
  const shell = useShellState();
</script>

<aside
  class="motion-fade hidden min-h-0 flex-col items-center border-l bg-card/20 xl:flex"
  aria-label="Repository details (hidden)"
>
  <!-- Matches the 3rem header row so the divider under it stays one line. -->
  <div class="flex h-12 w-full shrink-0 items-center justify-center border-b">
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon-sm"
            aria-label="Show repository details"
            onclick={() => shell.setDetailsOpen(true)}
          >
            <PanelRightOpen />
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content side="left">Show details</Tooltip.Content>
    </Tooltip.Root>
  </div>
  <div class="pt-2">
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon-sm"
            aria-label={pageState.repository?.favorited
              ? "Remove from favorites"
              : "Add to favorites"}
            aria-pressed={pageState.repository?.favorited ?? false}
            disabled={pageState.settings.favoritePending}
            onclick={() => void pageState.settings.toggleFavorite()}
          >
            <Heart
              class={pageState.repository?.favorited
                ? "fill-current text-amber-400"
                : ""}
            />
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content side="left">
        {pageState.repository?.favorited ? "Favorited" : "Favorite"}
      </Tooltip.Content>
    </Tooltip.Root>
  </div>
</aside>
