<script lang="ts">
  import Cloud from "@lucide/svelte/icons/cloud";
  import HardDrive from "@lucide/svelte/icons/hard-drive";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
  import { toast } from "svelte-sonner";

  import {
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
