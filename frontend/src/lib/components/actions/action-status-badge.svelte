<script lang="ts">
  import Ban from "@lucide/svelte/icons/ban";
  import CheckCircle2 from "@lucide/svelte/icons/check-circle-2";
  import CircleDashed from "@lucide/svelte/icons/circle-dashed";
  import CircleOff from "@lucide/svelte/icons/circle-off";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import XCircle from "@lucide/svelte/icons/x-circle";

  import { Badge } from "$lib/components/ui/badge/index.js";
  import type { ActionStatus } from "$lib/api/actions.js";

  let { status }: { status: ActionStatus } = $props();

  const label = $derived(
    status === "queued"
      ? "Queued"
      : status === "running"
        ? "Running"
        : status === "success"
          ? "Success"
          : status === "failure"
            ? "Failure"
            : status === "cancelled"
              ? "Cancelled"
              : "Skipped",
  );
</script>

<Badge
  variant="outline"
  class={[
    "gap-1.5",
    status === "success" && "border-emerald-500/40 text-emerald-600",
    status === "failure" && "border-destructive/40 text-destructive",
    status === "running" && "border-blue-500/40 text-blue-600",
    (status === "cancelled" || status === "skipped") &&
      "text-muted-foreground",
  ]}
>
  {#if status === "queued"}
    <CircleDashed class="size-3" />
  {:else if status === "running"}
    <LoaderCircle class="size-3 animate-spin" />
  {:else if status === "success"}
    <CheckCircle2 class="size-3" />
  {:else if status === "failure"}
    <XCircle class="size-3" />
  {:else if status === "cancelled"}
    <Ban class="size-3" />
  {:else}
    <CircleOff class="size-3" />
  {/if}
  {label}
</Badge>
