<script lang="ts">
  import {
    Ban,
    CheckCircle2,
    CircleDashed,
    CircleOff,
    LoaderCircle,
    XCircle,
  } from "lucide-svelte";

  import { Badge } from "$lib/components/ui/badge/index.js";
  import type { ActionStatus } from "$lib/api.js";

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
