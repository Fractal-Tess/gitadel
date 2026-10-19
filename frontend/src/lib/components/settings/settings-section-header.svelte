<script lang="ts">
  import type { Snippet } from "svelte";

  import type { ShellIcon } from "$lib/state/shell-state.svelte.js";

  /**
   * The heading above a group of cards. It matches a card's own header, so a
   * page mixing single cards and card grids reads as one set of sections.
   */
  let {
    id,
    icon: Icon,
    title,
    description,
    actions,
  }: {
    id: string;
    icon: ShellIcon;
    title: string;
    description?: string;
    actions?: Snippet;
  } = $props();
</script>

<header class="flex flex-wrap items-start justify-between gap-3">
  <div class="flex min-w-0 items-start gap-3">
    <Icon class="mt-1 size-4 shrink-0 text-muted-foreground" />
    <div class="min-w-0">
      <h2 {id} class="text-base leading-snug font-medium">{title}</h2>
      {#if description}
        <p class="mt-0.5 max-w-2xl text-sm text-muted-foreground">
          {description}
        </p>
      {/if}
    </div>
  </div>
  {#if actions}
    <div class="flex shrink-0 items-center gap-2">{@render actions()}</div>
  {/if}
</header>
