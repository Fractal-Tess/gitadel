<script lang="ts">
  import Cloud from "@lucide/svelte/icons/cloud";
  import Gauge from "@lucide/svelte/icons/gauge";
  import HardDrive from "@lucide/svelte/icons/hard-drive";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
  import { toast } from "svelte-sonner";

  import {
    measuredUsageSchema,
    storageTargetSchema,
    storageTargetTestSchema,
    type StorageTarget,
    type StorageTargetKind,
  } from "$lib/api/storage.js";
  import {
    ApiFailure,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import {
    invalidateAdminActivity,
    loadStorageTargets,
    peekStorageTargets,
    setStorageTargets,
  } from "$lib/settings/settings-data-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  let targets = $state.raw<StorageTarget[]>([]);
  let loading = $state(true);
  let working = $state(false);
  let error = $state<string | null>(null);
  let adding = $state(false);
  let kind = $state<StorageTargetKind>("filesystem");
  let name = $state("");
  let path = $state("");
  let endpoint = $state("");
  let bucket = $state("");
  let accessKey = $state("");
  let secretKey = $state("");
  let region = $state("us-east-1");
  let prefix = $state("gitadel-lfs");
  let testedFingerprint = $state<string | null>(null);
  let removeDialogOpen = $state(false);
  let pendingRemoveTarget = $state.raw<StorageTarget | null>(null);
  let measuring = $state<string | null>(null);

  const inputFingerprint = $derived(
    JSON.stringify({
      kind,
      name,
      path,
      endpoint,
      bucket,
      accessKey,
      secretKey,
      region,
      prefix,
    }),
  );
  const canTest = $derived(
    name.trim().length > 0 &&
      (kind === "filesystem"
        ? path.trim().length > 0
        : endpoint.trim().length > 0 &&
          bucket.trim().length > 0 &&
          accessKey.length > 0 &&
          secretKey.length > 0),
  );
  const canSave = $derived(canTest && testedFingerprint === inputFingerprint);

  $effect(() => {
    const scope = app.authorizationScope;
    void loadTargets(scope);
  });

  function payload(includeName = true) {
    return {
      name: includeName ? name.trim() : undefined,
      kind,
      path: kind === "filesystem" ? path.trim() : undefined,
      endpoint: kind === "s3" ? endpoint.trim() : undefined,
      bucket: kind === "s3" ? bucket.trim() : undefined,
      access_key: kind === "s3" ? accessKey : undefined,
      secret_key: kind === "s3" ? secretKey : undefined,
      region: kind === "s3" ? region.trim() : undefined,
      prefix: kind === "s3" ? prefix.trim() : undefined,
    };
  }

  async function loadTargets(scope: typeof app.authorizationScope) {
    const cached = peekStorageTargets(scope);
    if (cached) {
      targets = cached;
      loading = false;
      return;
    }
    loading = true;
    error = null;
    try {
      const loaded = await loadStorageTargets(scope);
      if (app.authorizationScope === scope) targets = loaded;
    } catch (caught) {
      if (app.authorizationScope === scope) error = message(caught);
    } finally {
      if (app.authorizationScope === scope) loading = false;
    }
  }

  async function testNewTarget() {
    testedFingerprint = null;
    await run(async () => {
      const response = await requestJson(
        "/api/v1/admin/storage/targets/test",
        storageTargetTestSchema,
        { method: "POST", body: jsonBody(payload(false)) },
      );
      testedFingerprint = inputFingerprint;
      toast.success(response.message);
    });
  }

  async function saveTarget() {
    if (!canSave) return;
    await run(async () => {
      const target = await requestJson(
        "/api/v1/admin/storage/targets",
        storageTargetSchema,
        { method: "POST", body: jsonBody(payload()) },
      );
      targets = [...targets, target];
      setStorageTargets(app.authorizationScope, targets);
      invalidateAdminActivity(app.authorizationScope);
      adding = false;
      resetForm();
      toast.success(`Added ${target.name}.`);
    });
  }

  async function testSavedTarget(target: StorageTarget) {
    if (target.managed_by_config) {
      toast.success(
        "Configured local storage is checked every time Gitadel starts.",
      );
      return;
    }
    await run(async () => {
      const response = await requestJson(
        `/api/v1/admin/storage/targets/${encodeURIComponent(target.id)}/test`,
        storageTargetTestSchema,
        { method: "POST" },
      );
      toast.success(response.message);
    });
  }

  function requestRemoveTarget(target: StorageTarget) {
    pendingRemoveTarget = target;
    removeDialogOpen = true;
  }

  async function confirmRemoveTarget() {
    const target = pendingRemoveTarget;
    if (!target) return;
    removeDialogOpen = false;
    pendingRemoveTarget = null;
    await run(async () => {
      await requestEmpty(
        `/api/v1/admin/storage/targets/${encodeURIComponent(target.id)}`,
        { method: "DELETE" },
      );
      targets = targets.filter((candidate) => candidate.id !== target.id);
      setStorageTargets(app.authorizationScope, targets);
      invalidateAdminActivity(app.authorizationScope);
      toast.success(
        `Removed ${target.name}. Its stored objects were left intact.`,
      );
    });
  }

  async function run(task: () => Promise<void>) {
    working = true;
    error = null;
    try {
      await task();
    } catch (caught) {
      toast.error(message(caught));
    } finally {
      working = false;
    }
  }

  function resetForm() {
    kind = "filesystem";
    name = "";
    path = "";
    endpoint = "";
    bucket = "";
    accessKey = "";
    secretKey = "";
    region = "us-east-1";
    prefix = "gitadel-lfs";
    testedFingerprint = null;
  }

  function message(caught: unknown) {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : "The request failed.";
  }

  function targetDetail(target: StorageTarget) {
    if (target.configuration.kind === "filesystem") {
      return target.configuration.path ?? "Path from Gitadel configuration";
    }
    return `${target.configuration.s3.bucket} · ${target.configuration.s3.endpoint}`;
  }

  async function measureTarget(target: StorageTarget) {
    measuring = target.id;
    try {
      const measured = await requestJson(
        `/api/v1/admin/storage/targets/${encodeURIComponent(target.id)}/usage`,
        measuredUsageSchema,
        { method: "POST" },
      );
      targets = targets.map((candidate) =>
        candidate.id === target.id
          ? { ...candidate, usage: { ...candidate.usage, measured } }
          : candidate,
      );
      setStorageTargets(app.authorizationScope, targets);
    } catch (caught) {
      toast.error(message(caught));
    } finally {
      measuring = null;
    }
  }

  // The scan is the fuller figure when it exists, since it also counts objects
  // the database has no record of. Without one, the LFS total is all we know.
  function storedBytes(target: StorageTarget) {
    return target.usage.measured?.total_bytes ?? target.usage.lfs_bytes;
  }

  function usageBar(target: StorageTarget) {
    const capacity = target.capacity;
    if (!capacity || capacity.total_bytes === 0) return null;
    const used = capacity.total_bytes - capacity.available_bytes;
    const gitadel = Math.min(storedBytes(target), used);
    const percent = (bytes: number) => (bytes / capacity.total_bytes) * 100;
    return {
      gitadel: percent(gitadel),
      // Whatever else shares the volume. Gitadel cannot name it, but leaving it
      // out would imply the free space is all ours to use.
      other: percent(Math.max(used - gitadel, 0)),
      label: `${formatBytes(gitadel)} used by Gitadel, ${formatBytes(
        capacity.available_bytes,
      )} free of ${formatBytes(capacity.total_bytes)}`,
    };
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

  function formatMeasuredAt(value: string) {
    const elapsed = Date.now() - new Date(value).getTime();
    if (elapsed < 60_000) return "just now";
    const minutes = Math.round(elapsed / 60_000);
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.round(minutes / 60);
    return hours < 24 ? `${hours}h ago` : `${Math.round(hours / 24)}d ago`;
  }
</script>

<div class="flex flex-col gap-8">
  {#if error}
    <Alert.Root variant="destructive">
      <TriangleAlert class="size-4" />
      <Alert.Title>Storage operation failed</Alert.Title>
      <Alert.Description>{error}</Alert.Description>
    </Alert.Root>
  {/if}

  <div class="flex flex-col gap-3">
    {#if loading}
      <div
        class="flex items-center gap-2 rounded-xl border p-5 text-sm text-muted-foreground"
      >
        <Spinner class="size-4" /> Loading storage targets…
      </div>
    {:else}
      <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        <IntegrationAddCard
          title="Add storage target"
          description="Connect a local filesystem or S3-compatible destination."
          onclick={() => {
            resetForm();
            adding = true;
          }}
        />
        {#each targets as target (target.id)}
          <IntegrationConnectionCard
            name={target.name}
            provider={target.kind}
            providerName={target.kind === "filesystem"
              ? "Local filesystem"
              : "S3-compatible"}
            subtitle={targetDetail(target)}
            description="Available to Git LFS and backup workflows."
            enabled
            statusLabel={target.active ? "Used by Git LFS" : "Ready"}
            statusHealthy
            detailLabel={target.managed_by_config ? "Config managed" : null}
          >
            {#snippet icon()}
              {#if target.kind === "s3"}
                <Cloud class="size-6 text-primary" />
              {:else}
                <HardDrive class="size-6 text-primary" />
              {/if}
            {/snippet}
            {#snippet details()}
              {@const bar = usageBar(target)}
              <div class="mt-4 flex flex-col gap-2">
                {#if bar}
                  <div
                    class="flex h-1.5 overflow-hidden rounded-full bg-muted"
                    role="img"
                    aria-label={bar.label}
                  >
                    <div class="bg-primary" style="width: {bar.gitadel}%"></div>
                    <div
                      class="bg-foreground/25"
                      style="width: {bar.other}%"
                    ></div>
                  </div>
                  <div
                    class="flex items-baseline justify-between gap-3 text-[11px] text-muted-foreground"
                  >
                    <span class="truncate">
                      {formatBytes(storedBytes(target))} Gitadel
                    </span>
                    <span class="shrink-0 tabular-nums">
                      {formatBytes(target.capacity?.available_bytes ?? 0)} free of
                      {formatBytes(target.capacity?.total_bytes ?? 0)}
                    </span>
                  </div>
                {:else}
                  <div class="text-[11px] text-muted-foreground">
                    {formatBytes(storedBytes(target))} stored · object storage reports
                    no capacity
                  </div>
                {/if}
                <div class="text-[11px] text-muted-foreground">
                  {#if target.usage.measured}
                    Scanned {formatMeasuredAt(
                      target.usage.measured.measured_at,
                    )}:
                    {target.usage.measured.object_count.toLocaleString()} objects.
                  {:else}
                    {target.usage.lfs_object_count.toLocaleString()} LFS objects tracked.
                    Scan to include anything else stored here.
                  {/if}
                </div>
              </div>
            {/snippet}
            {#snippet actions()}
              <Button
                variant="outline"
                size="sm"
                class="flex-1"
                onclick={() => testSavedTarget(target)}
                disabled={working}
              >
                <RefreshCw data-icon="inline-start" /> Test
              </Button>
              <Button
                variant="outline"
                size="sm"
                class="flex-1"
                onclick={() => measureTarget(target)}
                disabled={working || measuring !== null}
              >
                {#if measuring === target.id}
                  <Spinner class="size-4" data-icon="inline-start" />
                {:else}
                  <Gauge data-icon="inline-start" />
                {/if}
                Scan
              </Button>
              {#if !target.managed_by_config}
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="text-muted-foreground hover:text-destructive"
                  aria-label={`Remove ${target.name}`}
                  title={target.active
                    ? "Select another target on the Git LFS page before removing this one."
                    : undefined}
                  onclick={() => requestRemoveTarget(target)}
                  disabled={working || target.active}
                >
                  <Trash2 class="size-4" />
                </Button>
              {/if}
            {/snippet}
          </IntegrationConnectionCard>
        {/each}
      </div>
    {/if}
  </div>
</div>

<Dialog.Root bind:open={adding}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>Add storage target</Dialog.Title>
      <Dialog.Description>
        Connect an empty filesystem or S3-compatible destination. Test it before
        saving.
      </Dialog.Description>
    </Dialog.Header>
    <form
      class="grid gap-5"
      onsubmit={(event) => {
        event.preventDefault();
        void saveTarget();
      }}
    >
      <div class="grid gap-4 md:grid-cols-2">
        <Field.Field>
          <Field.Label for="storage-name">Name</Field.Label>
          <Input
            id="storage-name"
            bind:value={name}
            placeholder="Primary LFS storage"
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="storage-kind">Type</Field.Label>
          <Select.Root type="single" bind:value={kind}>
            <Select.Trigger id="storage-kind" class="w-full">
              {kind === "filesystem" ? "Local filesystem" : "S3-compatible"}
            </Select.Trigger>
            <Select.Content>
              <Select.Group>
                <Select.Label>Storage backend</Select.Label>
                <Select.Item value="filesystem" label="Local filesystem">
                  Local filesystem
                </Select.Item>
                <Select.Item value="s3" label="S3-compatible">
                  S3-compatible
                </Select.Item>
              </Select.Group>
            </Select.Content>
          </Select.Root>
        </Field.Field>
        {#if kind === "filesystem"}
          <Field.Field class="md:col-span-2">
            <Field.Label for="storage-path">Path</Field.Label>
            <Input
              id="storage-path"
              bind:value={path}
              placeholder="/data/gitadel-lfs-secondary"
            />
          </Field.Field>
        {:else}
          <Field.Field>
            <Field.Label for="storage-endpoint">Endpoint</Field.Label>
            <Input
              id="storage-endpoint"
              bind:value={endpoint}
              placeholder="https://s3.example.com"
            />
          </Field.Field>
          <Field.Field>
            <Field.Label for="storage-bucket">Bucket</Field.Label>
            <Input id="storage-bucket" bind:value={bucket} />
          </Field.Field>
          <Field.Field>
            <Field.Label for="storage-access-key">Access key</Field.Label>
            <Input
              id="storage-access-key"
              bind:value={accessKey}
              autocomplete="off"
            />
          </Field.Field>
          <Field.Field>
            <Field.Label for="storage-secret-key">Secret key</Field.Label>
            <Input
              id="storage-secret-key"
              bind:value={secretKey}
              type="password"
              autocomplete="new-password"
            />
          </Field.Field>
          <Field.Field>
            <Field.Label for="storage-region">Region</Field.Label>
            <Input id="storage-region" bind:value={region} />
          </Field.Field>
          <Field.Field>
            <Field.Label for="storage-prefix">Prefix</Field.Label>
            <Input id="storage-prefix" bind:value={prefix} />
          </Field.Field>
        {/if}
      </div>
      <Dialog.Footer>
        <Button
          type="button"
          variant="ghost"
          onclick={() => {
            adding = false;
            resetForm();
          }}
          disabled={working}>Cancel</Button
        >
        <Button
          type="button"
          variant="outline"
          onclick={testNewTarget}
          disabled={!canTest || working}
        >
          {#if working}
            <Spinner class="size-4" data-icon="inline-start" />
          {:else}
            <RefreshCw class="size-4" data-icon="inline-start" />
          {/if}
          Test target
        </Button>
        <Button type="submit" disabled={!canSave || working}>Save target</Button
        >
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={removeDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>
        Remove {pendingRemoveTarget?.name ?? "this storage target"}?
      </AlertDialog.Title>
      <AlertDialog.Description>
        Gitadel will remove this target from its configuration. Stored objects
        at the destination will not be deleted.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={working}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={working}
        onclick={() => void confirmRemoveTarget()}
      >
        Remove target
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
