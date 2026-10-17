<script lang="ts">
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import CircleCheck from "@lucide/svelte/icons/circle-check";
  import CircleDashed from "@lucide/svelte/icons/circle-dashed";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";

  import { deploymentStatusView } from "$lib/api/integrations.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import type { DokployConfigureState } from "$lib/repository/state/integrations-state.svelte.js";

  let { configure }: { configure: DokployConfigureState } = $props();
  // Several deployments can be listed on one page.
  const uid = $props.id();
  const headingId = `${uid}-deployment-heading`;

  const latest = $derived(configure.deployment?.deployment ?? null);
  const view = $derived(
    configure.awaitingDeployment
      ? { label: "Starting", tone: "running" as const }
      : deploymentStatusView(latest),
  );
  const startedAt = $derived.by(() => {
    if (!latest?.created_at) return null;
    const date = new Date(latest.created_at);
    return Number.isNaN(date.getTime()) ? null : date;
  });
</script>

<section
  class="rounded-xl border bg-card p-5 shadow-sm"
  aria-labelledby={headingId}
>
  <div class="flex flex-wrap items-start justify-between gap-3">
    <div class="min-w-0">
      <h2 id={headingId} class="font-semibold">
        Latest deployment
      </h2>
      <p class="mt-1 text-sm text-muted-foreground">
        {#if configure.awaitingDeployment}
          Waiting for Dokploy to start the deployment…
        {:else if !configure.deployment && configure.deploymentLoading}
          Asking Dokploy…
        {:else if !latest}
          This resource has not been deployed yet.
        {:else}
          {latest.title ?? "Deployment"}{#if startedAt}
            · started {startedAt.toLocaleString()}{/if}
        {/if}
      </p>
    </div>
    <div class="flex items-center gap-2">
      {#if configure.deployment || configure.awaitingDeployment}
        <span
          class={[
            "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium",
            view.tone === "done" && "border-emerald-500/30 text-emerald-500",
            view.tone === "error" && "border-destructive/40 text-destructive",
            view.tone === "running" && "border-primary/40 text-primary",
            view.tone === "idle" && "text-muted-foreground",
          ]}
          role="status"
        >
          {#if view.tone === "running"}
            <Spinner class="size-3 animate-spin" />
          {:else if view.tone === "done"}
            <CircleCheck class="size-3" aria-hidden="true" />
          {:else if view.tone === "error"}
            <CircleAlert class="size-3" aria-hidden="true" />
          {:else}
            <CircleDashed class="size-3" aria-hidden="true" />
          {/if}
          {view.label}
        </span>
      {/if}
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Refresh deployment status"
        disabled={configure.deploymentLoading}
        onclick={() => (configure.deploymentWatch += 1)}
      >
        <RefreshCw
          class={configure.deploymentLoading ? "size-3.5 animate-spin" : "size-3.5"}
        />
      </Button>
    </div>
  </div>

  {#if latest?.error}
    <p
      class="mt-3 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 font-mono text-xs leading-5 whitespace-pre-wrap text-destructive"
    >
      {latest.error}
    </p>
  {/if}
  {#if latest?.log}
    <details class="mt-3" open={view.tone === "error"}>
      <summary class="cursor-pointer text-xs font-medium text-muted-foreground">
        End of the deployment log
      </summary>
      <pre
        class="mt-2 max-h-80 overflow-auto rounded-md border bg-muted/30 px-3 py-2 font-mono text-[0.7rem] leading-5 whitespace-pre-wrap">{latest.log.trimEnd()}</pre>
    </details>
  {/if}
  {#if configure.deploymentError}
    <p class="mt-3 text-xs text-destructive">{configure.deploymentError}</p>
  {:else if configure.deploymentPollExpired}
    <p class="mt-3 text-xs text-muted-foreground">
      Still building after ten minutes. Gitadel stopped checking; refresh or
      open the logs in Dokploy.
    </p>
  {/if}

  {#if configure.deployment?.logs_url}
    <Button
      href={configure.deployment.logs_url}
      target="_blank"
      rel="noreferrer"
      variant="outline"
      size="sm"
      class="mt-4 gap-1.5"
    >
      View logs in Dokploy <ExternalLink class="size-3.5" />
    </Button>
  {/if}
</section>
