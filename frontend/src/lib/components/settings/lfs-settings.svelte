<script lang="ts">
  import Cloud from "@lucide/svelte/icons/cloud";
  import Database from "@lucide/svelte/icons/database";
  import HardDrive from "@lucide/svelte/icons/hard-drive";
  import Search from "@lucide/svelte/icons/search";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { toast } from "svelte-sonner";

  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as Empty from "$lib/components/ui/empty/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import * as InputGroup from "$lib/components/ui/input-group/index.js";
  import * as NativeSelect from "$lib/components/ui/native-select/index.js";
  import * as Table from "$lib/components/ui/table/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Progress } from "$lib/components/ui/progress/index.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import { ApiFailure, jsonBody, requestJson } from "$lib/api/transport.js";
  import {
    lfsRepositoryUsageResponseSchema,
    storageMigrationProgressSchema,
    storageMigrationScheduledSchema,
    type LfsRepositoryUsageResponse,
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
  } from "$lib/settings/settings-data-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  let targets = $state.raw<StorageTarget[]>([]);
  let status = $state.raw<LfsStorageStatus | null>(null);
  let usage = $state.raw<LfsRepositoryUsageResponse | null>(null);
  let usageSearch = $state("");
  let usageOwner = $state("");
  let usageOwnerType = $state("");
  let usageMinBytes = $state("");
  let usageMaxBytes = $state("");
  let usageSort = $state("bytes_desc");
  let usageLoading = $state(true);
  let usageLoadingMore = $state(false);
  let usageError = $state<string | null>(null);
  let usageRequestVersion = 0;
  let loading = $state(true);
  let working = $state(false);
  let error = $state<string | null>(null);
  let migration = $state.raw<StorageMigrationProgress | null>(null);
  let pendingTarget = $state.raw<StorageTarget | null>(null);
  let targetDialogOpen = $state(false);
  let progressSource: EventSource | null = null;

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
      progressSource = null;
      usageRequestVersion += 1;
    };
  });

  $effect(() => {
    const scope = app.authorizationScope;
    void loadUsage(scope, false);
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

  async function loadUsage(
    scope: typeof app.authorizationScope,
    append: boolean,
  ) {
    const version = ++usageRequestVersion;
    usageError = null;
    if (append) usageLoadingMore = true;
    else {
      usageLoading = true;
    }
    const parameters = new URLSearchParams({
      limit: "10",
      offset: append ? String(usage?.repositories.length ?? 0) : "0",
      sort: usageSort,
    });
    if (usageSearch.trim()) parameters.set("search", usageSearch.trim());
    if (usageOwner.trim()) parameters.set("owner", usageOwner.trim());
    if (usageOwnerType) parameters.set("owner_type", usageOwnerType);
    if (usageMinBytes.trim()) parameters.set("min_bytes", usageMinBytes.trim());
    if (usageMaxBytes.trim()) parameters.set("max_bytes", usageMaxBytes.trim());
    try {
      const loaded = await requestJson(
        `/api/v1/admin/storage/lfs/repositories?${parameters}`,
        lfsRepositoryUsageResponseSchema,
      );
      if (version !== usageRequestVersion || app.authorizationScope !== scope)
        return;
      usage =
        append && usage
          ? {
              ...loaded,
              repositories: [...usage.repositories, ...loaded.repositories],
            }
          : loaded;
    } catch (caught) {
      if (version === usageRequestVersion && app.authorizationScope === scope) {
        usageError = message(caught);
      }
    } finally {
      if (version === usageRequestVersion) {
        usageLoading = false;
        usageLoadingMore = false;
      }
    }
  }

  function clearUsageFilters() {
    usageSearch = "";
    usageOwner = "";
    usageOwnerType = "";
    usageMinBytes = "";
    usageMaxBytes = "";
    usageSort = "bytes_desc";
  }

  function repositoryHref(
    repository: NonNullable<LfsRepositoryUsageResponse>["repositories"][number],
  ) {
    return `/${encodeURIComponent(repository.owner_name)}/${encodeURIComponent(repository.repository_name)}`;
  }

  function openTargetDialog() {
    if (working) return;
    pendingTarget = availableTargets[0] ?? null;
    targetDialogOpen = true;
  }

  async function confirmMigration() {
    const target = pendingTarget;
    const scope = app.authorizationScope;
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
      if (app.authorizationScope !== scope) return;
      invalidateAdminActivity(scope);
      migration = {
        operation_id: response.operation_id,
        key: "",
        operation: "lfs_migrate",
        phase: "scheduled",
        message: response.message,
        processed_bytes: null,
        total_bytes: null,
      };
      toast.success(response.message);
      watchMigration(response.operation_id);
    } catch (caught) {
      working = false;
      toast.error(message(caught));
    }
  }

  function watchMigration(operationId: string) {
    const scope = app.authorizationScope;
    progressSource?.close();
    const source = new EventSource(
      `/api/v1/admin/storage/progress/${encodeURIComponent(operationId)}`,
    );
    progressSource = source;
    source.onmessage = (event) => {
      if (progressSource !== source || app.authorizationScope !== scope) return;
      let value: unknown;
      try {
        value = JSON.parse(event.data);
      } catch {
        return;
      }
      const parsed = storageMigrationProgressSchema.safeParse(value);
      if (!parsed.success || parsed.data.operation_id !== operationId) return;
      migration = parsed.data;
      if (parsed.data.phase === "failed" || parsed.data.phase === "completed") {
        source.close();
        progressSource = null;
        working = false;
        if (parsed.data.phase === "failed") {
          error = parsed.data.message;
        } else {
          migration = null;
          toast.success("Git LFS storage migration completed.");
          void refreshAfterMigration(scope);
        }
      }
    };
  }

  async function refreshAfterMigration(scope: typeof app.authorizationScope) {
    try {
      const [updatedTargets, updatedStatus] = await Promise.all([
        refreshStorageTargets(scope),
        refreshLfsStatus(scope),
      ]);
      if (app.authorizationScope !== scope) return;
      targets = updatedTargets;
      status = updatedStatus;
      await loadUsage(scope, false);
    } catch (caught) {
      if (app.authorizationScope === scope) error = message(caught);
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
          <Spinner class="mt-0.5 size-5 text-primary" />
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
              <p class="mt-2 text-xs text-muted-foreground">
                {formatBytes(migration.processed_bytes ?? 0)} copied. Gitadel remains
                online; LFS writes wait during final cutover.
              </p>
            {/if}
          </div>
        </div>
      </section>
    {/if}
    <section
      class="rounded-xl border bg-card/40 p-5 shadow-sm"
      aria-labelledby="lfs-repository-usage-title"
    >
      <div class="flex flex-col gap-1">
        <h2 id="lfs-repository-usage-title" class="text-base font-semibold">
          Repository usage
        </h2>
        <p class="text-sm leading-6 text-muted-foreground">
          Find repositories that currently consume Git LFS space. Usage is
          logical: an object shared by repositories is counted once for each
          repository that owns it.
        </p>
      </div>

      <Field.Group class="mt-5 grid gap-3 md:grid-cols-2 xl:grid-cols-4">
        <Field.Field>
          <Field.Label for="lfs-search">Repository search</Field.Label>
          <InputGroup.Root>
            <InputGroup.Input
              id="lfs-search"
              bind:value={usageSearch}
              placeholder="Search by name"
            />
            <InputGroup.Addon><Search /></InputGroup.Addon>
          </InputGroup.Root>
        </Field.Field>
        <Field.Field>
          <Field.Label for="lfs-owner">Owner</Field.Label>
          <Input
            id="lfs-owner"
            bind:value={usageOwner}
            placeholder="Username or organization"
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="lfs-owner-type">Owner type</Field.Label>
          <NativeSelect.Root id="lfs-owner-type" bind:value={usageOwnerType}>
            <NativeSelect.Option value="">All owners</NativeSelect.Option>
            <NativeSelect.Option value="user">Users</NativeSelect.Option>
            <NativeSelect.Option value="organization"
              >Organizations</NativeSelect.Option
            >
          </NativeSelect.Root>
        </Field.Field>
        <Field.Field>
          <Field.Label for="lfs-sort">Sort</Field.Label>
          <NativeSelect.Root id="lfs-sort" bind:value={usageSort}>
            <NativeSelect.Option value="bytes_desc"
              >Largest first</NativeSelect.Option
            >
            <NativeSelect.Option value="bytes_asc"
              >Smallest first</NativeSelect.Option
            >
            <NativeSelect.Option value="name"
              >Repository name</NativeSelect.Option
            >
          </NativeSelect.Root>
        </Field.Field>
        <Field.Field>
          <Field.Label for="lfs-minimum">Minimum space (bytes)</Field.Label>
          <Input
            id="lfs-minimum"
            type="number"
            min="0"
            step="1"
            value={usageMinBytes}
            placeholder="No minimum"
            oninput={(event) => (usageMinBytes = event.currentTarget.value)}
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="lfs-maximum">Maximum space (bytes)</Field.Label>
          <Input
            id="lfs-maximum"
            type="number"
            min="0"
            step="1"
            value={usageMaxBytes}
            placeholder="No maximum"
            oninput={(event) => (usageMaxBytes = event.currentTarget.value)}
          />
        </Field.Field>
        <div class="flex items-end md:col-span-2">
          <Button type="button" variant="outline" onclick={clearUsageFilters}>
            Clear filters
          </Button>
        </div>
      </Field.Group>

      {#if usageError}
        <Alert.Root class="mt-5" variant="destructive">
          <Alert.Title>Could not load repository usage</Alert.Title>
          <Alert.Description>{usageError}</Alert.Description>
        </Alert.Root>
      {:else if usageLoading}
        <div
          class="mt-5 flex items-center gap-2 rounded-lg border p-4 text-sm text-muted-foreground"
          aria-live="polite"
        >
          <Spinner class="size-4" /> Loading repository usage…
        </div>
      {:else if usage?.repositories.length === 0}
        <Empty.Root class="mt-5 border border-dashed">
          <Empty.Header>
            <Empty.Title>No repositories match these filters</Empty.Title>
            <Empty.Description
              >Try another name, owner, or space range.</Empty.Description
            >
          </Empty.Header>
        </Empty.Root>
      {:else}
        <div class="mt-5 overflow-x-auto rounded-lg border">
          <Table.Root class="min-w-[620px]">
            <Table.Header>
              <Table.Row>
                <Table.Head>Repository</Table.Head>
                <Table.Head>Owner</Table.Head>
                <Table.Head class="text-right">Objects</Table.Head>
                <Table.Head class="text-right">Logical space</Table.Head>
              </Table.Row>
            </Table.Header>
            <Table.Body>
              {#each usage?.repositories ?? [] as repository (repository.repository_id)}
                <Table.Row>
                  <Table.Cell>
                    <a
                      class="font-medium text-primary underline-offset-4 hover:underline"
                      href={repositoryHref(repository)}
                    >
                      {repository.repository_name}
                    </a>
                  </Table.Cell>
                  <Table.Cell>
                    <div>{repository.owner_name}</div>
                    <div class="text-xs capitalize text-muted-foreground">
                      {repository.owner_type}
                    </div>
                  </Table.Cell>
                  <Table.Cell class="text-right tabular-nums">
                    {repository.object_count.toLocaleString()}
                  </Table.Cell>
                  <Table.Cell class="text-right tabular-nums">
                    {formatBytes(repository.total_bytes)}
                  </Table.Cell>
                </Table.Row>
              {/each}
            </Table.Body>
          </Table.Root>
        </div>
        <div class="mt-4 flex flex-wrap items-center justify-between gap-3">
          <p class="text-xs text-muted-foreground">
            Showing {usage?.repositories.length.toLocaleString() ?? 0} of
            {usage?.total.toLocaleString() ?? 0} repositories with LFS usage.
          </p>
          {#if usage && usage.repositories.length < usage.total}
            <Button
              type="button"
              variant="outline"
              disabled={usageLoadingMore}
              onclick={() => void loadUsage(app.authorizationScope, true)}
            >
              {#if usageLoadingMore}
                <Spinner data-icon="inline-start" /> Loading…
              {:else}
                Load more
              {/if}
            </Button>
          {/if}
        </div>
      {/if}
    </section>
  {/if}
</div>

<Dialog.Root bind:open={targetDialogOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>Configure Git LFS storage</Dialog.Title>
      <Dialog.Description>
        Choose a configured destination. Gitadel will copy and verify objects in
        the background, then select it as the active target without restarting.
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
        Browsing, Git operations, and LFS downloads remain available. LFS
        uploads continue during copying and wait while the final changes are
        moved.
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
