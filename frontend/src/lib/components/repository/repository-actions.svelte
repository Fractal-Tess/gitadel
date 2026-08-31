<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import Download from "@lucide/svelte/icons/download";
  import GitCommit from "@lucide/svelte/icons/git-commit";
  import PackageOpen from "@lucide/svelte/icons/package-open";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Workflow from "@lucide/svelte/icons/workflow";
  import { tick } from "svelte";

  import ActionStatusBadge from "$lib/components/actions/action-status-badge.svelte";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Empty from "$lib/components/ui/empty/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();
  const sizeFormatter = new Intl.NumberFormat(undefined, {
    maximumFractionDigits: 1,
  });
  const dateFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
  const sizeUnits = ["B", "KiB", "MiB", "GiB"] as const;

  let logElement: HTMLPreElement | null = $state(null);
  let followLog = $state(true);
  let selectedJob = $derived(
    repository.actions.actionRun?.jobs.find((job) => job.id === repository.actionJobId),
  );

  function formatDate(value: string) {
    return dateFormatter.format(new Date(value));
  }

  function formatSize(bytes: number) {
    if (bytes <= 0) return "0 B";
    const exponent = Math.min(
      Math.floor(Math.log(bytes) / Math.log(1024)),
      sizeUnits.length - 1,
    );
    return `${sizeFormatter.format(bytes / 1024 ** exponent)} ${sizeUnits[exponent]}`;
  }

  function updateFollowLog() {
    if (!logElement) return;
    followLog =
      logElement.scrollHeight - logElement.scrollTop - logElement.clientHeight <
      32;
  }

  async function jumpToLatest() {
    followLog = true;
    await tick();
    logElement?.scrollTo({ top: logElement.scrollHeight });
  }

  $effect.pre(() => {
    repository.actions.actionLogs?.text;
    if (!followLog) return;
    void tick().then(() => {
      logElement?.scrollTo({ top: logElement.scrollHeight });
    });
  });
</script>

{#if repository.actions.actionsLoading && !repository.actions.actionRuns}
  <div class="py-16 text-center text-sm text-muted-foreground">
    Loading Actions…
  </div>
{:else if repository.actions.actionRun}
  <div class="mx-auto grid max-w-5xl gap-5">
    <div>
      <Button
        type="button"
        variant="ghost"
        class="-ml-3"
        onclick={() =>
          repository.navigate("actions", {
            commit: repository.actionCommit,
            page: repository.actionPage,
          })}
      >
        <ArrowLeft data-icon="inline-start" />All runs
      </Button>
    </div>
    <header
      class="flex flex-wrap items-start justify-between gap-4 border-b pb-5"
    >
      <div>
        <div class="flex items-center gap-2">
          <ActionStatusBadge status={repository.actions.actionRun.run.status} />
          <span class="text-sm text-muted-foreground"
            >Run #{repository.actions.actionRun.run.number}</span
          >
        </div>
        <h1 class="mt-3 text-xl font-semibold">
          {repository.actions.actionRun.run.workflow_name}
        </h1>
        <p class="mt-1 font-mono text-xs text-muted-foreground">
          {repository.actions.actionRun.run.reference} · {repository.actions.actionRun.run.after_sha.slice(
            0,
            12,
          )}
        </p>
      </div>
      {#if repository.actions.actionRun.can_cancel}
        <Button
          variant="destructive"
          disabled={repository.actions.actionsPending}
          onclick={() => void repository.actions.cancelActionRun()}
        >
          {repository.actions.actionsPending ? "Cancelling…" : "Cancel run"}
        </Button>
      {/if}
    </header>

    {#if repository.actions.actionRun.diagnostic}
      <Alert.Root variant="destructive">
        <Alert.Title>Workflow could not run</Alert.Title>
        <Alert.Description>
          <pre class="overflow-x-auto whitespace-pre-wrap text-xs">{repository
            .actions.actionRun.diagnostic}</pre>
        </Alert.Description>
      </Alert.Root>
    {/if}

    <div class="grid gap-5 lg:grid-cols-[18rem_minmax(0,1fr)]">
      <div class="overflow-hidden rounded-lg border">
        {#each repository.actions.actionRun.jobs as job (job.id)}
          <button
            type="button"
            class={[
              "flex w-full items-center justify-between gap-3 border-b px-4 py-3 text-left last:border-b-0 hover:bg-muted/40",
              repository.actionJobId === job.id && "bg-muted/60",
            ]}
            onclick={() => repository.actions.selectActionJob(job.id)}
          >
            <span class="min-w-0">
              <span class="block truncate text-sm font-medium">{job.name}</span>
              <span class="mt-1 block truncate text-xs text-muted-foreground"
                >{job.labels.join(", ")}</span
              >
            </span>
            <ActionStatusBadge status={job.status} />
          </button>
        {/each}
      </div>

      <div class="grid gap-3">
        {#if selectedJob?.failure_summary}
          <div
            class="rounded-md border border-destructive/30 bg-destructive/5 p-3"
          >
            <p class="text-sm font-medium text-destructive">
              {selectedJob.failure_summary}
            </p>
            {#if selectedJob.failure_kind}
              <p class="mt-1 font-mono text-xs text-destructive/80">
                {selectedJob.failure_kind}
              </p>
            {/if}
          </div>
        {/if}
        <div
          class="relative min-h-80 overflow-hidden rounded-lg border bg-zinc-950 text-zinc-100"
        >
          {#if repository.actionJobId}
            <pre
              bind:this={logElement}
              role="log"
              aria-live="polite"
              onscroll={updateFollowLog}
              class="h-[32rem] overflow-auto p-4 font-mono text-xs leading-5 whitespace-pre-wrap">{repository
                .actions.actionLogs?.text || "Waiting for log output…"}</pre>
            <div class="absolute right-4 bottom-4 flex gap-2">
              {#if repository.actions.actionLogs && !repository.actions.actionLogs.complete}
                <Button
                  type="button"
                  size="sm"
                  variant="secondary"
                  disabled={repository.actions.actionLogsLoading}
                  onclick={() => void repository.actions.loadActionLog()}
                >
                  {repository.actions.actionLogsLoading ? "Loading…" : "Load more"}
                </Button>
              {/if}
              {#if !followLog}
                <Button
                  type="button"
                  size="sm"
                  onclick={() => void jumpToLatest()}
                >
                  Jump to latest
                </Button>
              {/if}
            </div>
          {:else}
            <div class="grid h-80 place-items-center text-sm text-zinc-400">
              Select a job to view its log.
            </div>
          {/if}
        </div>
      </div>
    </div>

    <section class="grid gap-3" aria-labelledby="run-artifacts-heading">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2
            id="run-artifacts-heading"
            class="flex items-center gap-2 font-semibold"
          >
            <PackageOpen class="size-4 text-muted-foreground" />Artifacts
          </h2>
          <p class="mt-1 text-sm text-muted-foreground">
            ZIP archives uploaded by this workflow run.
          </p>
        </div>
        {#if repository.actions.actionArtifactsError}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onclick={() => void repository.actions.loadActionArtifacts()}>Retry</Button
          >
        {/if}
      </div>

      {#if repository.actions.actionArtifactsLoading && repository.actions.actionArtifacts.length === 0}
        <p
          class="rounded-lg border border-dashed p-4 text-sm text-muted-foreground"
          aria-live="polite"
        >
          Loading artifacts…
        </p>
      {:else if repository.actions.actionArtifactsError}
        <Alert.Root variant="destructive">
          <Alert.Title>Artifacts unavailable</Alert.Title>
          <Alert.Description>
            Could not load artifacts: {repository.actions.actionArtifactsError}
          </Alert.Description>
        </Alert.Root>
      {:else if repository.actions.actionArtifacts.length === 0}
        <div class="rounded-lg border border-dashed p-4">
          <p class="font-medium">No artifacts for this run</p>
          <p class="mt-1 text-sm leading-5 text-muted-foreground">
            Artifacts uploaded by workflow jobs will appear here.
          </p>
        </div>
      {:else}
        <ul class="overflow-hidden rounded-lg border">
          {#each repository.actions.actionArtifacts as artifact (artifact.id)}
            <li
              class="flex flex-col gap-3 border-b p-4 last:border-b-0 sm:flex-row sm:items-center sm:justify-between"
            >
              <div class="min-w-0">
                <p class="truncate font-medium">{artifact.name}</p>
                <p class="mt-1 text-xs text-muted-foreground">
                  {formatSize(artifact.size_bytes)} · Expires
                  {formatDate(artifact.expires_at)}
                </p>
              </div>
              <Button
                href={artifact.download_url}
                download
                variant="outline"
                size="sm"
                class="self-start sm:self-auto"
              >
                <Download data-icon="inline-start" />Download
              </Button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  </div>
{:else if !repository.actions.actionRuns || repository.actions.actionRuns.runs.length === 0}
  <Empty.Root class="mx-auto max-w-xl border border-dashed">
    <Empty.Header>
      <Empty.Media variant="icon">
        <Workflow />
      </Empty.Media>
      <Empty.Title>No workflow runs</Empty.Title>
      <Empty.Description>
        Push a supported workflow in <code>.forgejo/workflows</code>,
        <code>.gitea/workflows</code>, or <code>.github/workflows</code>. The
        first directory that exists takes precedence.
      </Empty.Description>
    </Empty.Header>
  </Empty.Root>
{:else}
  <div class="mx-auto max-w-5xl">
    <header class="mb-5 flex items-center justify-between gap-4">
      <div>
        <h1 class="text-xl font-semibold">Actions</h1>
        <p class="mt-1 text-sm text-muted-foreground">
          Push-triggered repository workflows
        </p>
      </div>
      <Button
        variant="outline"
        size="sm"
        class="gap-2"
        onclick={() => void repository.actions.loadActions()}
      >
        <RefreshCw data-icon="inline-start" />Refresh
      </Button>
    </header>
    <div class="overflow-hidden rounded-lg border">
      {#each repository.actions.actionRuns.runs as run (run.id)}
        <button
          type="button"
          class="grid w-full gap-3 border-b px-4 py-4 text-left last:border-b-0 hover:bg-muted/40 sm:grid-cols-[minmax(0,1fr)_auto]"
          onclick={() => repository.actions.selectActionRun(run.id)}
        >
          <span class="min-w-0">
            <span class="flex items-center gap-2">
              <span class="truncate font-medium">{run.workflow_name}</span>
              <span class="text-xs text-muted-foreground">#{run.number}</span>
            </span>
            <span
              class="mt-1 flex items-center gap-1.5 font-mono text-xs text-muted-foreground"
            >
              <GitCommit class="size-3" />{run.after_sha.slice(0, 12)} · {formatDate(
                run.created_at,
              )}
            </span>
            {#if run.failure_summary}
              <span class="mt-2 block text-sm text-destructive"
                >{run.failure_summary}</span
              >
            {/if}
          </span>
          <ActionStatusBadge status={run.status} />
        </button>
      {/each}
    </div>
    <div class="mt-4 flex justify-between">
      <Button
        variant="outline"
        disabled={repository.actions.actionRuns.page <= 1}
        onclick={() =>
          repository.navigate("actions", {
            page: repository.actions.actionRuns!.page - 1,
            commit: repository.actionCommit,
          })}>Previous</Button
      >
      <Button
        variant="outline"
        disabled={!repository.actions.actionRuns.has_more}
        onclick={() =>
          repository.navigate("actions", {
            page: repository.actions.actionRuns!.page + 1,
            commit: repository.actionCommit,
          })}>Next</Button
      >
    </div>
  </div>
{/if}
