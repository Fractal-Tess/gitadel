<script lang="ts">
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import Heart from "@lucide/svelte/icons/heart";
  import LockKeyhole from "@lucide/svelte/icons/lock-keyhole";
  import PanelRightClose from "@lucide/svelte/icons/panel-right-close";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import * as Tooltip from "$lib/components/ui/tooltip/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  let { state: pageState }: { state: RepositoryPageState } = $props();
  const shell = useShellState();
  /** Set by a click that favorites, so the heart swells once in response. */
  let popped = $state(false);
</script>

<!-- Matches the height of the tree and content headers so the three columns
     share one unbroken divider under the app header. -->
<section
  class="flex min-h-12 shrink-0 flex-wrap items-center gap-2 border-b px-4 py-2"
>
  <!-- The code view heads its own file tree with the branch picker, so the
       toolbar only carries it for the views that have no tree column. -->
  {#if pageState.view !== "settings" && pageState.view !== "overview"}
    <Select.Root
      type="single"
      value={pageState.revision}
      onValueChange={(value) => {
        if (value && value !== pageState.revision) pageState.changeRevision(value);
      }}
    >
      <Select.Trigger
        class="min-w-0 flex-1 shadow-none"
        aria-label="Switch branch"
      >
        <span class="flex min-w-0 items-center gap-2">
          <GitBranch class="size-3.5 shrink-0 text-muted-foreground" />
          <span class="truncate">{pageState.revision}</span>
        </span>
      </Select.Trigger>
      <Select.Content align="end">
        {#each pageState.browser.refs?.branches ?? [] as branch (branch.name)}
          <Select.Item value={branch.name}>{branch.name}</Select.Item>
        {/each}
      </Select.Content>
    </Select.Root>
  {/if}

  {#if pageState.repository?.mirrored}
    <span
      class="inline-flex items-center gap-1 rounded-full border border-sky-500/30 bg-sky-500/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-sky-600 dark:text-sky-400"
    >
      <RefreshCw class="size-2.5" />Mirror
    </span>
  {/if}
  {#if pageState.repository?.visibility === "private"}
    <span
      class="inline-flex items-center gap-1.5 rounded border px-2 py-1 text-xs font-medium text-muted-foreground"
    >
      <LockKeyhole class="size-3" />Private
    </span>
  {/if}
  {#if pageState.repository?.archived_at}
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
    variant={pageState.repository?.favorited ? "secondary" : "outline"}
    class="ml-auto gap-2 max-sm:h-11"
    aria-pressed={pageState.repository?.favorited ?? false}
    disabled={pageState.settings.favoritePending}
    onclick={() => {
      popped = !pageState.repository?.favorited;
      void pageState.settings.toggleFavorite();
    }}
  >
    <Heart
      class={[
        "size-3.5",
        pageState.repository?.favorited && "fill-current text-amber-400",
        popped && pageState.repository?.favorited && "motion-pop",
      ]}
    />
    <!-- The longest label sizes the button so toggling never shifts the row. -->
    <span class="grid justify-items-center">
      <span class="invisible col-start-1 row-start-1" aria-hidden="true">
        Favorited
      </span>
      <span class="col-start-1 row-start-1">
        {pageState.repository?.favorited ? "Favorited" : "Favorite"}
      </span>
    </span>
  </Button>
  <Tooltip.Root>
    <Tooltip.Trigger>
      {#snippet child({ props })}
        <Button
          {...props}
          variant="ghost"
          size="icon-sm"
          class="hidden xl:inline-flex"
          aria-label="Hide repository details"
          onclick={() => shell.setDetailsOpen(false)}
        >
          <PanelRightClose />
        </Button>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Content side="bottom">Hide details</Tooltip.Content>
  </Tooltip.Root>
</section>
