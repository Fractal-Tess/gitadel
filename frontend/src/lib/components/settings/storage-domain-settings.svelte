<script lang="ts">
  import Cloud from "@lucide/svelte/icons/cloud";
  import Database from "@lucide/svelte/icons/database";
  import HardDrive from "@lucide/svelte/icons/hard-drive";
  import Search from "@lucide/svelte/icons/search";
  import { toast } from "svelte-sonner";

  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Empty from "$lib/components/ui/empty/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import * as InputGroup from "$lib/components/ui/input-group/index.js";
  import * as NativeSelect from "$lib/components/ui/native-select/index.js";
  import * as Table from "$lib/components/ui/table/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Progress } from "$lib/components/ui/progress/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import { ApiFailure, jsonBody, requestJson } from "$lib/api/transport.js";
  import {
    LOCAL_STORAGE_TARGET_ID,
    storageDomainMigrationSchema,
    storageDomainMigrationStartedSchema,
    storageDomainPath,
    storageDomainRepositoriesPath,
    storageDomainRepositoryUsagePageSchema,
    type StorageDomainMigration,
    type StorageDomainRepositoryUsage,
    type StorageDomainRepositoryUsagePage,
    type StorageDomainStatus,
    type StorageTarget,
    type StorageUsageDetail,
  } from "$lib/api/storage.js";
  import {
    invalidateAdminActivity,
    loadStorageDomain,
    loadStorageTargets,
    peekStorageDomain,
    peekStorageTargets,
    refreshStorageDomain,
    refreshStorageTargets,
  } from "$lib/settings/settings-data-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  type Scope = ReturnType<typeof useAppState>["authorizationScope"];

  let {
    domain,
    noun,
    objectsLabel,
    migrationNote,
  }: {
    /** Storage domain name, such as `lfs` or `registry`. */
    domain: string;
    /** The domain's name within a sentence, such as "container registry". */
    noun: string;
    /** Label of the object count, such as "LFS objects". */
    objectsLabel: string;
    /** What stays available while a migration runs. */
    migrationNote: string;
  } = $props();

  const app = useAppState();
  let targets = $state.raw<StorageTarget[]>([]);
  let status = $state.raw<StorageDomainStatus | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let usage = $state.raw<StorageDomainRepositoryUsagePage | null>(null);
  let filters = $state({
    search: "",
    owner: "",
    ownerType: "",
    minBytes: "",
    maxBytes: "",
    sort: "bytes_desc",
  });
  let usageLoading = $state(true);
  let usageLoadingMore = $state(false);
  let usageError = $state<string | null>(null);
  let usageRequestVersion = 0;

  let migration = $state.raw<StorageDomainMigration | null>(null);
  let working = $state(false);
  let targetDialogOpen = $state(false);
  // `null` selects the domain's local storage.
  let pendingTargetId = $state<string | null | undefined>(undefined);
  let progressSource: EventSource | null = null;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let watchedOperation: string | null = null;

  let label = $derived(status?.label ?? domain);
  let localActive = $derived(status?.active_target_id === null);
  let activeTarget = $derived(
    targets.find((target) => target.id === status?.active_target_id) ?? null,
  );
  // The nil target is the LFS configuration root; every domain offers its
  // own local storage instead.
  let availableTargets = $derived(
    targets.filter(
      (target) =>
        target.id !== LOCAL_STORAGE_TARGET_ID &&
        target.id !== status?.active_target_id,
    ),
  );
  let detailFields = $derived(status?.detail_fields ?? []);
  let repositoryFields = $derived(
    detailFields.filter((field) => field.per_repository),
  );
  let migrationPercent = $derived.by(() => {
    if (!migration?.total_bytes) return null;
    return Math.min(
      100,
      Math.round((migration.processed_bytes / migration.total_bytes) * 100),
    );
  });

  $effect(() => {
    const scope = app.authorizationScope;
    const name = domain;
    void load(scope, name);
    return () => {
      stopWatching();
      usageRequestVersion += 1;
    };
  });

  $effect(() => {
    const scope = app.authorizationScope;
    const name = domain;
    // Reading every filter reloads the first page when one changes.
    const current = { ...filters };
    void loadUsage(scope, name, current, false);
  });

  async function load(scope: Scope, name: string) {
    const cachedTargets = peekStorageTargets(scope);
    const cachedStatus = peekStorageDomain(scope, name);
    if (cachedTargets && cachedStatus) {
      applyStatus(cachedTargets, cachedStatus, scope);
      loading = false;
      return;
    }
    loading = true;
    error = null;
    try {
      const [loadedTargets, loadedStatus] = await Promise.all([
        loadStorageTargets(scope),
        loadStorageDomain(scope, name),
      ]);
      if (app.authorizationScope !== scope || domain !== name) return;
      applyStatus(loadedTargets, loadedStatus, scope);
    } catch (caught) {
      if (app.authorizationScope === scope) error = message(caught);
    } finally {
      if (app.authorizationScope === scope) loading = false;
    }
  }

  function applyStatus(
    loadedTargets: StorageTarget[],
    loadedStatus: StorageDomainStatus,
    scope: Scope,
  ) {
    targets = loadedTargets;
    status = loadedStatus;
    // Resume a migration started elsewhere or before a reload.
    const active = loadedStatus.active_migration;
    if (active && watchedOperation !== active.operation_id) {
      migration = active;
      working = true;
      watchMigration(active.operation_id, scope);
    }
  }

  async function loadUsage(
    scope: Scope,
    name: string,
    current: typeof filters,
    append: boolean,
  ) {
    const version = ++usageRequestVersion;
    usageError = null;
    if (append) usageLoadingMore = true;
    else usageLoading = true;
    const offset = append ? (usage?.repositories.length ?? 0) : 0;
    try {
      const loaded = await requestJson(
        storageDomainRepositoriesPath(name, current, offset),
        storageDomainRepositoryUsagePageSchema,
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

  function clearFilters() {
    filters = {
      search: "",
      owner: "",
      ownerType: "",
      minBytes: "",
      maxBytes: "",
      sort: "bytes_desc",
    };
  }

  function openTargetDialog() {
    if (working) return;
    pendingTargetId = !localActive
      ? null
      : (availableTargets[0]?.id ?? undefined);
    targetDialogOpen = true;
  }

  async function confirmMigration() {
    const targetId = pendingTargetId;
    const scope = app.authorizationScope;
    if (targetId === undefined) return;
    targetDialogOpen = false;
    working = true;
    error = null;
    try {
      const response = await requestJson(
        storageDomainPath(domain, "migrate"),
        storageDomainMigrationStartedSchema,
        {
          method: "POST",
          body: jsonBody(
            targetId === null
              ? { local: true, batch_size: 100 }
              : { target_id: targetId, batch_size: 100 },
          ),
        },
      );
      if (app.authorizationScope !== scope) return;
      invalidateAdminActivity(scope);
      toast.success(response.message);
      migration = null;
      watchMigration(response.operation_id, scope);
    } catch (caught) {
      working = false;
      toast.error(message(caught));
    }
  }

  function stopWatching() {
    progressSource?.close();
    progressSource = null;
    if (reconnectTimer) clearTimeout(reconnectTimer);
    reconnectTimer = null;
    watchedOperation = null;
  }

  function watchMigration(operationId: string, scope: Scope) {
    stopWatching();
    watchedOperation = operationId;
    connect(operationId, scope);
  }

  function connect(operationId: string, scope: Scope) {
    const source = new EventSource(
      storageDomainPath(domain, "migrations", operationId, "events"),
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
      const parsed = storageDomainMigrationSchema.safeParse(value);
      if (!parsed.success || parsed.data.operation_id !== operationId) return;
      migration = parsed.data;
      if (parsed.data.phase === "failed" || parsed.data.phase === "completed") {
        stopWatching();
        working = false;
        if (parsed.data.phase === "failed") {
          error = parsed.data.message;
        } else {
          migration = null;
          toast.success(parsed.data.message);
        }
        void refresh(scope);
      }
    };
    source.onerror = () => {
      source.close();
      if (watchedOperation !== operationId || progressSource !== source) return;
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null;
        if (
          watchedOperation === operationId &&
          app.authorizationScope === scope
        ) {
          connect(operationId, scope);
        }
      }, 1500);
    };
  }

  async function refresh(scope: Scope) {
    const name = domain;
    try {
      const [updatedTargets, updatedStatus] = await Promise.all([
        refreshStorageTargets(scope),
        refreshStorageDomain(scope, name),
      ]);
      if (app.authorizationScope !== scope || domain !== name) return;
      targets = updatedTargets;
      status = updatedStatus;
      await loadUsage(scope, name, { ...filters }, false);
    } catch (caught) {
      if (app.authorizationScope === scope) error = message(caught);
    }
  }

  function repositoryHref(repository: StorageDomainRepositoryUsage) {
    return `/${encodeURIComponent(repository.owner_name)}/${encodeURIComponent(repository.repository_name)}`;
  }

  function targetDetail(target: StorageTarget) {
    if (target.configuration.kind === "filesystem") {
      return target.configuration.path ?? "Path from Gitadel configuration";
    }
    return `${target.configuration.s3.bucket} · ${target.configuration.s3.endpoint}`;
  }

  function formatDetail(field: StorageUsageDetail, value: number | undefined) {
    return field.unit === "bytes"
      ? formatBytes(value ?? 0)
      : (value ?? 0).toLocaleString();
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

{#snippet targetIcon(target: StorageTarget | null)}
  {#if target?.kind === "s3"}
    <Cloud class="size-6 text-primary" />
  {:else if target === null || target.managed_by_config}
    <Database class="size-6 text-primary" />
  {:else}
    <HardDrive class="size-6 text-primary" />
  {/if}
{/snippet}

<div class="flex flex-col gap-8">
  {#if error}
    <Alert.Root variant="destructive">
      <Alert.Title>{label} operation failed</Alert.Title>
      <Alert.Description>{error}</Alert.Description>
    </Alert.Root>
  {/if}

  {#if loading || !status}
    {#if loading}
      <div
        class="flex items-center gap-2 rounded-xl border p-5 text-sm text-muted-foreground"
      >
        <Spinner class="size-4" /> Loading storage…
      </div>
    {/if}
  {:else}
    <section
      aria-label="{label} storage target"
      class="flex flex-col gap-3"
    >
      <div class="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
        <IntegrationAddCard
          title="Change storage target"
          description="Select another destination for {noun} data."
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
            description="Currently stores {noun} data."
            enabled
            statusLabel="Active"
            statusHealthy
            onconfigure={openTargetDialog}
            configureLabel="Change target"
          >
            {#snippet icon()}{@render targetIcon(activeTarget)}{/snippet}
          </IntegrationConnectionCard>
        {:else if localActive}
          <IntegrationConnectionCard
            name={status.local_label}
            provider="local"
            providerName="Local storage"
            subtitle={status.local_root}
            description="Currently stores {noun} data."
            enabled
            statusLabel="Active"
            statusHealthy
            detailLabel="Config managed"
            onconfigure={openTargetDialog}
            configureLabel="Change target"
          >
            {#snippet icon()}{@render targetIcon(null)}{/snippet}
          </IntegrationConnectionCard>
        {/if}
      </div>
      <p class="text-sm text-muted-foreground">
        Storage destinations are created on the <a
          class="text-primary underline-offset-4 hover:underline"
          href="/-/administration/storage">Storage page</a
        >.
      </p>
    </section>

    <section
      class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4"
      aria-label="{label} usage"
    >
      {@render stat(
        "Stored bytes",
        formatBytes(status.usage.total_bytes),
        "Persisted data only",
      )}
      {@render stat(objectsLabel, status.usage.object_count.toLocaleString())}
      {@render stat(
        "Repositories",
        status.usage.repository_count.toLocaleString(),
        "With stored data",
      )}
      {@render stat(
        "Active target",
        status.active_target_name ?? status.local_label,
      )}
    </section>

    {#if detailFields.length > 0}
      <dl
        class="grid grid-cols-2 gap-x-6 gap-y-4 rounded-xl border bg-card/40 p-5 shadow-sm sm:grid-cols-3 lg:grid-cols-6"
        aria-label="{label} details"
      >
        {#each detailFields as field (field.key)}
          <div class="min-w-0">
            <dt class="truncate text-xs text-muted-foreground">
              {field.label}{field.per_repository ? "" : " *"}
            </dt>
            <dd class="mt-1 text-lg font-semibold tabular-nums">
              {formatDetail(field, status.usage.details[field.key])}
            </dd>
          </div>
        {/each}
        {#if detailFields.some((field) => !field.per_repository)}
          <p class="col-span-full text-xs text-muted-foreground">
            * Temporary data, not included in stored bytes.
          </p>
        {/if}
      </dl>
    {/if}

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
                {formatBytes(migration.processed_bytes)} of {formatBytes(
                  migration.total_bytes ?? 0,
                )}
              </p>
            {:else}
              <p class="mt-2 text-xs text-muted-foreground">
                {formatBytes(migration.processed_bytes)} copied. {migrationNote}
              </p>
            {/if}
          </div>
        </div>
      </section>
    {/if}
  {/if}

  <section
    class="rounded-xl border bg-card/40 p-5 shadow-sm"
    aria-labelledby="{domain}-repository-usage-title"
  >
    <div class="flex flex-col gap-1">
      <h2 id="{domain}-repository-usage-title" class="text-base font-semibold">
        Repository usage
      </h2>
      <p class="text-sm leading-6 text-muted-foreground">
        Find repositories that consume {noun} space. Usage is logical: an object
        shared by repositories is counted once for each repository that owns it.
      </p>
    </div>

    <Field.Group class="mt-5 grid gap-3 md:grid-cols-2 xl:grid-cols-4">
      <Field.Field>
        <Field.Label for="{domain}-search">Repository search</Field.Label>
        <InputGroup.Root>
          <InputGroup.Input
            id="{domain}-search"
            bind:value={filters.search}
            placeholder="Search by name"
          />
          <InputGroup.Addon><Search /></InputGroup.Addon>
        </InputGroup.Root>
      </Field.Field>
      <Field.Field>
        <Field.Label for="{domain}-owner">Owner</Field.Label>
        <Input
          id="{domain}-owner"
          bind:value={filters.owner}
          placeholder="Username or organization"
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="{domain}-owner-type">Owner type</Field.Label>
        <NativeSelect.Root
          id="{domain}-owner-type"
          bind:value={filters.ownerType}
        >
          <NativeSelect.Option value="">All owners</NativeSelect.Option>
          <NativeSelect.Option value="user">Users</NativeSelect.Option>
          <NativeSelect.Option value="organization"
            >Organizations</NativeSelect.Option
          >
        </NativeSelect.Root>
      </Field.Field>
      <Field.Field>
        <Field.Label for="{domain}-sort">Sort</Field.Label>
        <NativeSelect.Root id="{domain}-sort" bind:value={filters.sort}>
          <NativeSelect.Option value="bytes_desc"
            >Largest first</NativeSelect.Option
          >
          <NativeSelect.Option value="bytes_asc"
            >Smallest first</NativeSelect.Option
          >
          <NativeSelect.Option value="name">Repository name</NativeSelect.Option
          >
        </NativeSelect.Root>
      </Field.Field>
      <Field.Field>
        <Field.Label for="{domain}-minimum">Minimum space (bytes)</Field.Label>
        <Input
          id="{domain}-minimum"
          type="number"
          min="0"
          step="1"
          value={filters.minBytes}
          placeholder="No minimum"
          oninput={(event) => (filters.minBytes = event.currentTarget.value)}
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="{domain}-maximum">Maximum space (bytes)</Field.Label>
        <Input
          id="{domain}-maximum"
          type="number"
          min="0"
          step="1"
          value={filters.maxBytes}
          placeholder="No maximum"
          oninput={(event) => (filters.maxBytes = event.currentTarget.value)}
        />
      </Field.Field>
      <div class="flex items-end md:col-span-2">
        <Button type="button" variant="outline" onclick={clearFilters}>
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
    {:else if usage}
      <div class="mt-5 overflow-x-auto rounded-lg border">
        <Table.Root class="min-w-[620px]">
          <Table.Header>
            <Table.Row>
              <Table.Head>Repository</Table.Head>
              <Table.Head>Owner</Table.Head>
              <Table.Head class="text-right">Objects</Table.Head>
              {#each repositoryFields as field (field.key)}
                <Table.Head class="text-right">{field.label}</Table.Head>
              {/each}
              <Table.Head class="text-right">Logical space</Table.Head>
            </Table.Row>
          </Table.Header>
          <Table.Body>
            {#each usage.repositories as repository (repository.repository_id)}
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
                {#each repositoryFields as field (field.key)}
                  <Table.Cell class="text-right tabular-nums">
                    {formatDetail(field, repository.details[field.key])}
                  </Table.Cell>
                {/each}
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
          Showing {usage.repositories.length.toLocaleString()} of
          {usage.total.toLocaleString()} repositories with {noun} usage.
        </p>
        {#if usage.repositories.length < usage.total}
          <Button
            type="button"
            variant="outline"
            disabled={usageLoadingMore}
            onclick={() =>
              void loadUsage(
                app.authorizationScope,
                domain,
                { ...filters },
                true,
              )}
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
</div>

{#snippet stat(title: string, value: string, note: string | null = null)}
  <div class="rounded-xl border bg-card/40 p-5 shadow-sm">
    <p
      class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
    >
      {title}
    </p>
    <p class="mt-2 truncate text-2xl font-semibold tabular-nums">{value}</p>
    {#if note}
      <p class="mt-1 text-xs text-muted-foreground">{note}</p>
    {/if}
  </div>
{/snippet}

<Dialog.Root bind:open={targetDialogOpen}>
  <Dialog.Content
    class="max-h-[calc(100dvh-2rem)] overflow-y-auto ring-foreground/20 sm:max-w-2xl"
  >
    <Dialog.Header>
      <Dialog.Title class="pr-8">Configure {noun} storage</Dialog.Title>
      <Dialog.Description>
        Choose a configured destination or local storage. Gitadel copies and
        verifies objects in the background, then selects the new target without
        restarting. Source objects are not deleted.
      </Dialog.Description>
    </Dialog.Header>
    <div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
      {#if status && !localActive}
        <IntegrationConnectionCard
          name={status.local_label}
          provider="local"
          providerName="Local storage"
          subtitle={status.local_root}
          description="Return {noun} data to local storage."
          enabled
          statusLabel="Available"
          statusHealthy
          detailLabel="Config managed"
          compact
          selected={pendingTargetId === null}
          onselect={() => (pendingTargetId = null)}
        >
          {#snippet icon()}{@render targetIcon(null)}{/snippet}
        </IntegrationConnectionCard>
      {/if}
      {#each availableTargets as target (target.id)}
        <IntegrationConnectionCard
          name={target.name}
          provider={target.kind}
          providerName={target.kind === "filesystem"
            ? "Local filesystem"
            : "S3-compatible"}
          subtitle={targetDetail(target)}
          description="Available as a {noun} destination."
          enabled
          statusLabel="Available"
          statusHealthy
          compact
          selected={pendingTargetId === target.id}
          onselect={() => (pendingTargetId = target.id)}
        >
          {#snippet icon()}{@render targetIcon(target)}{/snippet}
        </IntegrationConnectionCard>
      {/each}
    </div>
    {#if availableTargets.length === 0}
      <p class="text-xs leading-5 text-muted-foreground">
        Add another destination on the Storage page before selecting an external
        {noun} target.
      </p>
    {:else}
      <p class="text-xs leading-5 text-muted-foreground">{migrationNote}</p>
    {/if}
    <Dialog.Footer>
      <Button
        type="button"
        variant="ghost"
        disabled={working}
        onclick={() => (targetDialogOpen = false)}>Cancel</Button
      >
      <Button
        type="button"
        disabled={working || pendingTargetId === undefined}
        onclick={() => void confirmMigration()}>Migrate and enable</Button
      >
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
