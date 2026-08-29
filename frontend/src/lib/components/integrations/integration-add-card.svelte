<script lang="ts">
  import Plus from "@lucide/svelte/icons/plus";

  let {
    title = "Add integration",
    description,
    compact = false,
    onclick,
  }: {
    title?: string;
    description: string;
    compact?: boolean;
    onclick: () => void;
  } = $props();
</script>

<button
  type="button"
  class={[
    "group flex w-full flex-col items-center justify-center rounded-xl border border-dashed bg-card/15 p-5 text-center transition-colors hover:border-foreground/25 hover:bg-card/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
    compact ? "min-h-52" : "min-h-72",
  ]}
  {onclick}
>
  <span
    class="grid size-12 place-items-center rounded-full border bg-background transition-transform group-hover:scale-105"
  >
    <Plus class="size-5" />
  </span>
  <span class="mt-4 font-medium">{title}</span>
  <span class="mt-1.5 max-w-52 text-sm leading-5 text-muted-foreground">
    {description}
  </span>
</button>
