<script lang="ts">
  import type { Snippet } from "svelte";

  let { children, style }: { children: Snippet; style?: string } = $props();
</script>

<span
  class="pointer-events-none absolute z-20 rounded-md border bg-popover px-2 py-1 font-mono text-[11px] font-medium whitespace-nowrap tabular-nums text-popover-foreground shadow-lg"
  {style}
  aria-hidden="true">{@render children()}</span
>
