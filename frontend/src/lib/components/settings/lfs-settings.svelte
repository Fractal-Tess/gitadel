<script lang="ts">
  import CheckCircle2 from "@lucide/svelte/icons/check-circle-2";
  import Cloud from "@lucide/svelte/icons/cloud";
  import Database from "@lucide/svelte/icons/database";
  import HardDrive from "@lucide/svelte/icons/hard-drive";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { toast } from "svelte-sonner";

  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Progress } from "$lib/components/ui/progress/index.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import { ApiFailure, jsonBody, requestJson } from "$lib/api/transport.js";
  import {
    storageMigrationProgressSchema,
    storageMigrationScheduledSchema,
    type LfsStorageStatus,
    type StorageMigrationProgress,
    type StorageTarget,
  } from "$lib/api/storage.js";
  import {
    invalidateAdminActivity,
    loadLfsStatus,
    loadStorageTargets,
    peekLfsStatus,
    peekStorageTargets,
    refreshLfsStatus,
    refreshStorageTargets,
    setLfsStatus,
    setStorageTargets,
  } from "$lib/settings/settings-data-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  let targets = $state.raw<StorageTarget[]>([]);
  let status = $state.raw<LfsStorageStatus | null>(null);
  let loading = $state(true);
  let working = $state(false);
  let error = $state<string | null>(null);
  let migration = $state.raw<StorageMigrationProgress | null>(null);
  let pendingTarget = $state.raw<StorageTarget | null>(null);
  let targetDialogOpen = $state(false);
  let progressSource: EventSource | null = null;
  let migrationPoll = 0;

  let activeTarget = $derived(targets.find((target) => target.active) ?? null);
  let availableTargets = $derived(targets.filter((target) => !target.active));
  let migrationPercent = $derived.by(() => {
    if (!migration?.total_bytes) return null;
    return Math.min(
      100,
      Math.round(
        ((migration.processed_bytes ?? 0) / migration.total_bytes) * 100,
      ),
    );
  });

  $effect(() => {
    const scope = app.authorizationScope;
    void load(scope);
    return () => {
      progressSource?.close();
      migrationPoll += 1;
    };
  });

  async function load(scope: typeof app.authorizationScope) {
    const cachedTargets = peekStorageTargets(scope);
    const cachedStatus = peekLfsStatus(scope);
    if (cachedTargets && cachedStatus) {
      targets = cachedTargets;
      status = cachedStatus;
      loading = false;
      return;
    }
    loading = true;
    error = null;
    try {
      const [loadedTargets, loadedStatus] = await Promise.all([
        loadStorageTargets(scope),
        loadLfsStatus(scope),
      ]);
      if (app.authorizationScope !== scope) return;
      targets = loadedTargets;
      status = loadedStatus;
    } catch (caught) {
      if (app.authorizationScope === scope) error = message(caught);
    } finally {
      if (app.authorizationScope === scope) loading = false;
    }
  }

  function openTargetDialog() {
    pendingTarget = availableTargets[0] ?? null;
    targetDialogOpen = true;
  }

  async function confirmMigration() {
    const target = pendingTarget;
    if (!target) return;
    targetDialogOpen = false;
    pendingTarget = null;
    working = true;
    error = null;
    try {
      const response = await requestJson(
        "/api/v1/admin/storage/migrations",
        storageMigrationScheduledSchema,
        {
          method: "POST",
          body: jsonBody({ target_id: target.id, batch_size: 100 }),
        },
      );
      invalidateAdminActivity(app.authorizationScope);
      migration = {
        operation_id: response.operation_id,
        key: "",
        operation: "lfs_migrate",
        phase: "scheduled",
        message: "Gitadel is entering maintenance mode.",
        processed_bytes: null,
        total_bytes: null,
      };
      toast.success(response.message);
      watchMigration(response.operation_id);
      void pollForMigration(target.id);
    } catch (caught) {
      working = false;
      toast.error(message(caught));
    }
  }

  function watchMigration(operationId: string) {
    progressSource?.close();
    const source = new EventSource(
      `/api/v1/admin/storage/progress/${encodeURIComponent(operationId)}`,
    );
    progressSource = source;
    source.onmessage = (event) => {
      let value: unknown;
      try {
        value = JSON.parse(event.data);
      } catch {
        return;
      }
      const parsed = storageMigrationProgressSchema.safeParse(value);
      if (!parsed.success || parsed.data.operation_id !== operationId) return;
      migration = parsed.data;
      if (parsed.data.phase === "failed") {
        error = parsed.data.message;
        source.close();
        working = false;
        migrationPoll += 1;
      }
    };
  }

  async function pollForMigration(targetId: string) {
    const poll = ++migrationPoll;
    const deadline = Date.now() + 10 * 60 * 1_000;
    let sawMaintenance = false;
    while (poll === migrationPoll && Date.now() < deadline) {
      try {
        const updated = await refreshStorageTargets(app.authorizationScope);
        if (updated.some((target) => target.id === targetId && target.active)) {
          targets = updated;
          setStorageTargets(app.authorizationScope, targets);
          status = await refreshLfsStatus(app.authorizationScope);
          setLfsStatus(app.authorizationScope, status);
          progressSource?.close();
          progressSource = null;
          migration = null;
          working = false;
          toast.success("Git LFS storage migration completed.");
          return;
        }
        if (sawMaintenance) {
          progressSource?.close();
          progressSource = null;
          migration = null;
          working = false;
          error =
            "Git LFS storage migration ended without selecting the destination.";
          return;
        }
      } catch {
        sawMaintenance = true;
      }
      await new Promise((resolve) => window.setTimeout(resolve, 1_000));
    }
    if (poll === migrationPoll) {
      error = "Git LFS storage migration did not finish within 10 minutes.";
      working = false;
    }
  }

  function targetDetail(target: StorageTarget) {
    if (target.configuration.kind === "filesystem") {
      return target.configuration.path ?? "Path from Gitadel configuration";
    }
    return `${target.configuration.s3.bucket} · ${target.configuration.s3.endpoint}`;
  }

  function formatBytes(bytes: number) {
    const units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let value = bytes;
    let unit = units[0];
    for (const candidate of units) {
      unit = candidate;
      if (value < 1024 || candidate === units.at(-1)) break;
      value /= 1024;
    }
    return `${value >= 10 || unit === "B" ? value.toFixed(0) : value.toFixed(1)} ${unit}`;
  }

  function message(caught: unknown) {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : "The request failed.";
  }
</script>

<div class="flex flex-col gap-8">
  {#if error}
    <Alert.Root variant="destructive">
      <Alert.Title>Git LFS operation failed</Alert.Title>
      <Alert.Description>{error}</Alert.Description>
    </Alert.Root>
  {/if}

  {#if loading}
    <div
      class="flex items-center gap-2 rounded-xl border p-5 text-sm text-muted-foreground"
    >
      <Spinner class="size-4" /> Loading Git LFS storage…
    </div>
  {:else}
    <div class="flex flex-col gap-3">
      <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        <IntegrationAddCard
          title="Change storage target"
          description="Select another configured destination for Git LFS objects."
          onclick={openTargetDialog}
        />
        {#if activeTarget}
          <IntegrationConnectionCard
            name={activeTarget.name}
            provider={activeTarget.kind}
            providerName={activeTarget.kind === "filesystem"
              ? "Local filesystem"
              : "S3-compatible"}
            subtitle={targetDetail(activeTarget)}
            description="Currently stores Git LFS objects."
            enabled
            statusLabel="Active"
            statusHealthy
            detailLabel={activeTarget.managed_by_config
              ? "Config managed"
              : null}
            onconfigure={openTargetDialog}
            configureLabel="Change target"
          >
            {#snippet icon()}
              {#if activeTarget.kind === "s3"}
                <Cloud class="size-6 text-primary" />
              {:else if activeTarget.managed_by_config}
                <Database class="size-6 text-primary" />
              {:else}
                <HardDrive class="size-6 text-primary" />
              {/if}
            {/snippet}
          </IntegrationConnectionCard>
        {/if}
      </div>
    </div>

    <section class="grid gap-4 sm:grid-cols-3" aria-label="Git LFS usage">
      <div class="rounded-xl border bg-card/40 p-5 shadow-sm">
        <p
          class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
        >
          Used storage
        </p>
        <p class="mt-2 text-2xl font-semibold tabular-nums">
          {formatBytes(status?.total_bytes ?? 0)}
        </p>
      </div>
      <div class="rounded-xl border bg-card/40 p-5 shadow-sm">
        <p
          class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
        >
          LFS objects
        </p>
        <p class="mt-2 text-2xl font-semibold tabular-nums">
          {(status?.object_count ?? 0).toLocaleString()}
        </p>
      </div>
      <div class="rounded-xl border bg-card/40 p-5 shadow-sm">
        <p
          class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
        >
          Active target
        </p>
        <p class="mt-2 truncate text-lg font-semibold">
          {activeTarget?.name ?? "Unavailable"}
        </p>
      </div>
    </section>

    {#if working && migration}
      <section
        class="rounded-xl border bg-card/40 p-5 shadow-sm"
        aria-live="polite"
      >
        <div class="flex items-start gap-3">
          {#if migration.phase === "completed"}
            <CheckCircle2 class="mt-0.5 size-5 text-emerald-500" />
          {:else}
            <Spinner class="mt-0.5 size-5 text-primary" />
          {/if}
          <div class="min-w-0 flex-1">
            <div class="flex items-center justify-between gap-3">
              <p class="text-sm font-medium">{migration.message}</p>
              {#if migrationPercent !== null}
                <span class="text-xs tabular-nums text-muted-foreground"
                  >{migrationPercent}%</span
                >
              {/if}
            </div>
            {#if migrationPercent !== null}
              <Progress class="mt-3 h-2" value={migrationPercent} max={100} />
              <p class="mt-2 text-xs text-muted-foreground">
                {formatBytes(migration.processed_bytes ?? 0)} of {formatBytes(
                  migration.total_bytes ?? 0,
                )}
              </p>
            {:else}
              <Progress class="mt-3 h-2" value={0} max={100} />
              <p class="mt-2 text-xs text-muted-foreground">
                This page reconnects automatically while Gitadel is in
                maintenance mode.
              </p>
            {/if}
          </div>
        </div>
      </section>
    {/if}
  {/if}
</div>

<Dialog.Root bind:open={targetDialogOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>Configure Git LFS storage</Dialog.Title>
      <Dialog.Description>
        Choose a configured destination. Gitadel will enter maintenance mode,
        copy and verify every object, then select it as the active target.
        Source objects will not be deleted.
      </Dialog.Description>
    </Dialog.Header>
    {#if availableTargets.length > 0}
      <div class="grid gap-4 sm:grid-cols-2">
        {#each availableTargets as target (target.id)}
          <IntegrationConnectionCard
            name={target.name}
            provider={target.kind}
            providerName={target.kind === "filesystem"
              ? "Local filesystem"
              : "S3-compatible"}
            subtitle={targetDetail(target)}
            description="Available as a Git LFS destination."
            enabled
            statusLabel="Available"
            statusHealthy
            detailLabel={target.managed_by_config ? "Config managed" : null}
            compact
            selected={pendingTarget?.id === target.id}
            onselect={() => (pendingTarget = target)}
          >
            {#snippet icon()}
              {#if target.kind === "s3"}
                <Cloud class="size-6 text-primary" />
              {:else if target.managed_by_config}
                <Database class="size-6 text-primary" />
              {:else}
                <HardDrive class="size-6 text-primary" />
              {/if}
            {/snippet}
          </IntegrationConnectionCard>
        {/each}
      </div>
      <p class="text-xs leading-5 text-muted-foreground">
        Gitadel remains unavailable to normal requests during the copy and
        cutover.
      </p>
      <Dialog.Footer>
        <Button
          type="button"
          variant="ghost"
          disabled={working}
          onclick={() => (targetDialogOpen = false)}>Cancel</Button
        >
        <Button
          type="button"
          disabled={working || !pendingTarget}
          onclick={() => void confirmMigration()}
        >
          Migrate and enable
        </Button>
      </Dialog.Footer>
    {:else}
      <div
        class="rounded-lg border bg-card/20 p-4 text-sm text-muted-foreground"
      >
        Add another destination on the Storage page before changing the Git LFS
        target.
      </div>
      <Dialog.Footer>
        <Button
          type="button"
          variant="ghost"
          onclick={() => (targetDialogOpen = false)}>Close</Button
        >
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
