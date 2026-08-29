<script lang="ts">
  import { toString as cronToString } from "cronstrue";
  import { onMount } from "svelte";
  import {
    ArchiveRestore,
    Bot,
    CalendarClock,
    CalendarDays,
    CheckCircle2,
    CloudUpload,
    Download,
    HardDrive,
    LoaderCircle,
    RefreshCw,
    ShieldCheck,
    Tag,
    Trash2,
    TriangleAlert,
    UserRound,
  } from "lucide-svelte";

  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import BackupProviderEditor, {
    type BackupProviderEditorValue,
  } from "$lib/components/settings/backup-provider-editor.svelte";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import {
    ApiFailure,
    backupProgressSchema,
    backupProviderSchema,
    backupProvidersSchema,
    backupProviderTestSchema,
    backupScheduledSchema,
    backupSnapshotSchema,
    jsonBody,
    requestEmpty,
    requestJson,
    restorePreflightSchema,
    type BackupProgress,
    type BackupProvider,
    type BackupProviderCatalogItem,
    type BackupSnapshot,
    type RestorePreflight,
  } from "$lib/api.js";
  import { z } from "zod";

  const CUSTOM_SCHEDULE = "custom";
  const DEFAULT_CUSTOM_CRON = "0 2 * * *";
  const SCHEDULE_OPTIONS = [
    { value: "", label: "Automatic backups off" },
    { value: "0 * * * *", label: "Every hour" },
    { value: "0 */6 * * *", label: "Every 6 hours" },
    { value: "0 2 * * *", label: "Every day at 02:00 UTC" },
    { value: "0 2 * * 0", label: "Every Sunday at 02:00 UTC" },
    { value: CUSTOM_SCHEDULE, label: "Custom cron" },
  ];

  let providerCatalog = $state.raw<BackupProviderCatalogItem[]>([]);
  let providers = $state.raw<BackupProvider[]>([]);
  let selectedProviderId = $state<string | null>(null);
  let settings = $derived(
    providers.find((provider) => provider.id === selectedProviderId) ?? null,
  );
  let snapshots = $state.raw<BackupSnapshot[]>([]);
  let preflight = $state.raw<RestorePreflight | null>(null);
  let pendingDelete = $state.raw<BackupSnapshot | null>(null);
  let deleteDialogOpen = $state(false);
  let deleting = $state(false);
  let creating = $state(false);
  let backupProgress = $state.raw<BackupProgress | null>(null);
  let progressFailure = $state<string | null>(null);
  let progressSource: EventSource | null = null;
  let providerEditorOpen = $state(false);
  let editingProvider = $state.raw<BackupProvider | null>(null);
  let editorRevision = $state(0);
  let providerRemoveDialogOpen = $state(false);
  let pendingProviderRemove = $state.raw<BackupProvider | null>(null);
  let backupName = $state("");
  let scheduleMode = $state("");
  let customCron = $state(DEFAULT_CUSTOM_CRON);
  let savingSchedule = $state(false);
  let scheduleExpression = $derived(
    scheduleMode === CUSTOM_SCHEDULE
      ? customCron.trim() || null
      : scheduleMode || null,
  );
  let cronExplanation = $derived(explainCron(customCron));
  let scheduleDirty = $derived(
    scheduleExpression !== (settings?.schedule ?? null),
  );
  let canSaveSchedule = $derived(
    scheduleDirty &&
      (scheduleMode !== CUSTOM_SCHEDULE || cronExplanation.valid),
  );
  let backupProgressPercent = $derived.by(() => {
    if (
      backupProgress?.processed_bytes === null ||
      backupProgress?.total_bytes === null ||
      !backupProgress?.total_bytes
    ) {
      return null;
    }
    return Math.min(
      100,
      Math.round(
        (backupProgress.processed_bytes / backupProgress.total_bytes) * 100,
      ),
    );
  });
  let password = $state("");
  let createSafetyBackup = $state(true);
  let confirmReplacement = $state(false);
  let working = $state(false);
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);

  onMount(() => {
    void initialize();
    return () => progressSource?.close();
  });

  async function initialize() {
    await run(async () => {
      const loaded = await requestJson(
        "/api/v1/admin/backup/providers",
        backupProvidersSchema,
      );
      providerCatalog = loaded.providers;
      providers = loaded.connections;
      const selected =
        providers.find((provider) => provider.id === selectedProviderId) ??
        providers[0] ??
        null;
      selectedProviderId = selected?.id ?? null;
      applyProvider(selected);
      snapshots = selected ? await fetchSnapshots(selected.id) : [];
    });
  }

  function applyProvider(provider: BackupProvider | null) {
    scheduleMode = modeForSchedule(provider?.schedule ?? null);
    if (scheduleMode === CUSTOM_SCHEDULE) {
      customCron = provider?.schedule ?? DEFAULT_CUSTOM_CRON;
    }
  }

  function providerPath(id: string) {
    return `/api/v1/admin/backup/providers/${encodeURIComponent(id)}`;
  }

  function providerPayload(value: BackupProviderEditorValue) {
    return {
      id: value.id,
      name: value.name,
      provider: value.provider,
      path: value.path || null,
      endpoint: value.endpoint || null,
      bucket: value.bucket || null,
      access_key: value.accessKey || null,
      secret_key: value.secretKey || null,
      region: value.region || null,
      prefix: value.prefix,
      test_token: value.testToken,
    };
  }

  async function fetchSnapshots(providerId = selectedProviderId) {
    if (!providerId) return [];
    return requestJson(
      `${providerPath(providerId)}/backups`,
      z.array(backupSnapshotSchema),
    );
  }

  function openCreateProvider() {
    editingProvider = null;
    editorRevision += 1;
    providerEditorOpen = true;
  }

  function openConfigureProvider(provider: BackupProvider) {
    editingProvider = provider;
    editorRevision += 1;
    providerEditorOpen = true;
  }

  async function testProvider(value: BackupProviderEditorValue) {
    return requestJson(
      "/api/v1/admin/backup/providers/test",
      backupProviderTestSchema,
      {
        method: "POST",
        body: jsonBody(providerPayload(value)),
      },
    );
  }

  async function saveProvider(value: BackupProviderEditorValue) {
    const saved = await requestJson(
      value.id
        ? providerPath(value.id)
        : "/api/v1/admin/backup/providers",
      backupProviderSchema,
      {
        method: value.id ? "PUT" : "POST",
        body: jsonBody(providerPayload(value)),
      },
    );
    providers = value.id
      ? providers.map((provider) =>
          provider.id === value.id ? saved : provider,
        )
      : [...providers, saved];
    selectedProviderId = saved.id;
    applyProvider(saved);
    providerEditorOpen = false;
    snapshots = [];
    await run(async () => {
      snapshots = await fetchSnapshots(saved.id);
    });
    notice = `${saved.name} saved.`;
  }

  async function selectProvider(provider: BackupProvider) {
    selectedProviderId = provider.id;
    preflight = null;
    applyProvider(provider);
    await run(async () => {
      snapshots = await fetchSnapshots(provider.id);
    });
  }

  function requestProviderRemove(provider: BackupProvider) {
    pendingProviderRemove = provider;
    providerRemoveDialogOpen = true;
  }

  async function confirmProviderRemove() {
    const provider = pendingProviderRemove;
    if (!provider) return;
    await run(async () => {
      await requestEmpty(providerPath(provider.id), { method: "DELETE" });
      providers = providers.filter((candidate) => candidate.id !== provider.id);
      const selected = providers[0] ?? null;
      selectedProviderId = selected?.id ?? null;
      applyProvider(selected);
      providerRemoveDialogOpen = false;
      pendingProviderRemove = null;
      snapshots = [];
      notice = `${provider.name} removed.`;
      snapshots = selected ? await fetchSnapshots(selected.id) : [];
    });
  }

  async function saveSchedule() {
    const provider = settings;
    if (!provider || !canSaveSchedule) return;
    savingSchedule = true;
    try {
      await run(async () => {
        const loaded = await requestJson(
          `${providerPath(provider.id)}/schedule`,
          backupProviderSchema,
          {
            method: "PUT",
            body: jsonBody({ schedule: scheduleExpression }),
          },
        );
        providers = providers.map((candidate) =>
          candidate.id === provider.id ? loaded : candidate,
        );
        selectedProviderId = loaded.id;
        applyProvider(loaded);
        notice = loaded.schedule
          ? "Automatic backup schedule saved."
          : "Automatic backups disabled.";
      });
    } finally {
      savingSchedule = false;
    }
  }

  async function refreshSnapshots() {
    await run(async () => {
      snapshots = await fetchSnapshots();
      notice = "Backup list refreshed.";
    });
  }

  async function createBackup() {
    const provider = settings;
    if (!provider) return;
    creating = true;
    try {
      await run(async () => {
        const response = await requestJson(
          `${providerPath(provider.id)}/backups`,
          backupScheduledSchema,
          {
            method: "POST",
            body: jsonBody({ name: backupName.trim() || null }),
          },
        );
        notice = response.message;
        backupProgress = {
          operation_id: response.operation_id,
          key: response.key,
          operation: "create",
          phase: "scheduled",
          message: "Stopping Gitadel services for a consistent snapshot.",
          processed_bytes: null,
          total_bytes: null,
        };
        watchBackupProgress(response.operation_id);
        await waitForBackup(response.key, provider.id);
        backupName = "";
        backupProgress = null;
        notice = "Backup created and added to snapshots.";
      });
    } finally {
      creating = false;
      progressSource?.close();
      progressSource = null;
    }
  }

  function watchBackupProgress(operationId: string) {
    progressFailure = null;
    progressSource?.close();
    const source = new EventSource(
      `/api/v1/admin/backups/progress/${encodeURIComponent(operationId)}`,
    );
    progressSource = source;
    source.onmessage = (event) => {
      let payload: unknown;
      try {
        payload = JSON.parse(event.data) as unknown;
      } catch {
        return;
      }
      const parsed = backupProgressSchema.safeParse(payload);
      if (!parsed.success || parsed.data.operation_id !== operationId) return;
      backupProgress = parsed.data;
      if (parsed.data.phase === "failed") {
        progressFailure = parsed.data.message;
        source.close();
      } else if (parsed.data.phase === "completed") {
        source.close();
      }
    };
  }

  async function waitForBackup(key: string, providerId: string) {
    const deadline = Date.now() + 10 * 60 * 1_000;
    await new Promise((resolve) => window.setTimeout(resolve, 1_500));
    while (Date.now() < deadline) {
      if (progressFailure) throw new Error(progressFailure);
      try {
        const updated = await fetchSnapshots(providerId);
        if (updated.some((snapshot) => snapshot.key === key)) {
          snapshots = updated;
          return;
        }
      } catch (caught) {
        if (
          caught instanceof ApiFailure &&
          ![404, 502, 503].includes(caught.status)
        ) {
          throw caught;
        }
        // The full server is unavailable while the maintenance server reports progress.
      }
      await new Promise((resolve) => window.setTimeout(resolve, 1_000));
    }
    throw new Error("Backup creation did not finish within 10 minutes.");
  }

  function requestDelete(snapshot: BackupSnapshot) {
    pendingDelete = snapshot;
    deleteDialogOpen = true;
  }

  async function deleteSnapshot() {
    const snapshot = pendingDelete;
    const provider = settings;
    if (!snapshot || !provider) return;
    deleting = true;
    let deleted = false;
    try {
      await run(async () => {
        await requestEmpty(
          `${providerPath(provider.id)}/backups/object?key=${encodeURIComponent(snapshot.key)}`,
          { method: "DELETE" },
        );
        snapshots = snapshots.filter(
          (candidate) => candidate.key !== snapshot.key,
        );
        if (preflight?.key === snapshot.key) preflight = null;
        notice = "Backup deleted.";
        deleted = true;
      });
      if (deleted) {
        deleteDialogOpen = false;
        pendingDelete = null;
      }
    } finally {
      deleting = false;
    }
  }

  async function validateSnapshot(snapshot: BackupSnapshot) {
    const provider = settings;
    if (!provider) return;
    await run(async () => {
      preflight = await requestJson(
        `${providerPath(provider.id)}/backups/preflight`,
        restorePreflightSchema,
        {
          method: "POST",
          body: jsonBody({ key: snapshot.key }),
        },
      );
      password = "";
      createSafetyBackup = true;
      confirmReplacement = false;
      notice = "Backup copied and verified. Review the restore details below.";
    });
  }

  async function restoreBackup() {
    const selected = preflight;
    const provider = settings;
    if (!selected || !provider) return;
    await run(async () => {
      const response = await requestJson(
        `${providerPath(provider.id)}/backups/restore`,
        backupScheduledSchema,
        {
          method: "POST",
          body: jsonBody({
            token: selected.token,
            password,
            create_safety_backup: createSafetyBackup,
          }),
        },
      );
      notice = response.message;
      void waitForRestart();
    });
  }

  async function waitForRestart() {
    await new Promise((resolve) => window.setTimeout(resolve, 1_500));
    for (;;) {
      try {
        const response = await fetch("/healthz", { cache: "no-store" });
        if (response.ok) {
          window.location.reload();
          return;
        }
      } catch {
        // The server is expected to be unavailable during maintenance.
      }
      await new Promise((resolve) => window.setTimeout(resolve, 1_000));
    }
  }

  async function run(task: () => Promise<void>) {
    working = true;
    error = null;
    notice = null;
    try {
      await task();
    } catch (caught) {
      error =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "The request failed.";
    } finally {
      working = false;
    }
  }

  function fileName(key: string) {
    return key.split("/").at(-1) ?? key;
  }

  function isAutomaticBackup(snapshot: BackupSnapshot) {
    return (
      snapshot.name === "pre restore safety" ||
      snapshot.name === "scheduled automatic backup"
    );
  }

  function formatBytes(bytes: number) {
    if (bytes < 1_024) return `${bytes} B`;
    const units = ["KiB", "MiB", "GiB", "TiB"];
    let value = bytes / 1_024;
    let unit = units[0];
    for (const next of units.slice(1)) {
      if (value < 1_024) break;
      value /= 1_024;
      unit = next;
    }
    return `${value.toFixed(value >= 10 ? 1 : 2)} ${unit}`;
  }

  function formatDate(value: string) {
    const date = new Date(value);
    return Number.isNaN(date.valueOf()) ? value : date.toLocaleString();
  }

  function modeForSchedule(schedule: string | null) {
    if (schedule === null) return "";
    return SCHEDULE_OPTIONS.some(
      (option) => option.value !== CUSTOM_SCHEDULE && option.value === schedule,
    )
      ? schedule
      : CUSTOM_SCHEDULE;
  }

  function explainCron(expression: string) {
    const normalized = expression.trim();
    if (![5, 6, 7].includes(normalized.split(/\s+/).filter(Boolean).length)) {
      return {
        valid: false,
        description: "Use a five-, six-, or seven-field cron expression.",
      };
    }
    try {
      const description = cronToString(normalized, {
        use24HourTimeFormat: true,
        verbose: true,
      }).replace(/\.$/, "");
      return { valid: true, description: `${description} (UTC).` };
    } catch {
      return {
        valid: false,
        description: "This is not a valid cron expression.",
      };
    }
  }
</script>

<section class="space-y-6" aria-labelledby="backups-heading">
  <header>
    <h2 id="backups-heading" class="text-lg font-semibold tracking-tight">
      Backups
    </h2>
    <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
      Add backup providers, then create and schedule complete instance snapshots.
    </p>
  </header>

  {#if notice}
    <p class="rounded-md border border-emerald-500/25 bg-emerald-500/8 p-3 text-sm text-emerald-300">
      {notice}
    </p>
  {/if}
  {#if error}
    <p class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
      {error}
    </p>
  {/if}

  {#if working && providers.length === 0}
    <p class="flex items-center gap-2 text-sm text-muted-foreground">
      <LoaderCircle class="size-4 animate-spin" />Loading backup providers…
    </p>
  {:else}
    <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
      <IntegrationAddCard
        title="Add backup provider"
        description="Connect another filesystem or S3 backup destination."
        onclick={openCreateProvider}
      />
      {#each providers as provider (provider.id)}
        {@const catalogProvider =
          providerCatalog.find(
            (candidate) => candidate.slug === provider.provider,
          ) ?? null}
        <IntegrationConnectionCard
          name={provider.name}
          provider={provider.provider}
          providerName={catalogProvider?.name ?? provider.provider}
          subtitle={provider.provider === "filesystem"
            ? (provider.path ?? "Filesystem")
            : (provider.bucket ?? "S3")}
          description={catalogProvider?.description ??
            "Configured backup destination."}
          enabled
          statusLabel={provider.schedule ? "Scheduled" : "Ready"}
          statusHealthy
          detailLabel={provider.schedule
            ? `Next ${provider.next_backup_at ? formatDate(provider.next_backup_at) : "run pending"}`
            : null}
          selected={provider.id === selectedProviderId}
          onselect={() => void selectProvider(provider)}
        >
          {#snippet icon()}
            {#if provider.provider === "filesystem"}
              <HardDrive class="size-6 text-primary" />
            {:else}
              <CloudUpload class="size-6 text-primary" />
            {/if}
          {/snippet}
        </IntegrationConnectionCard>
      {/each}
    </div>
  {/if}

  {#if settings}
    <div
      class="flex flex-wrap items-center justify-between gap-3 rounded-xl border bg-card/25 px-4 py-3"
    >
      <div class="min-w-0">
        <p class="truncate text-sm font-medium">Managing {settings.name}</p>
        <p class="mt-0.5 text-xs text-muted-foreground">
          {settings.provider === "filesystem"
            ? settings.path
            : `${settings.bucket} at ${settings.endpoint}`}
        </p>
      </div>
      <div class="flex gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          onclick={() => openConfigureProvider(settings)}
        >
          Configure
        </Button>
        {#if !settings.managed_by_config}
          <Button
            type="button"
            variant="ghost"
            size="sm"
            class="text-muted-foreground hover:text-destructive"
            onclick={() => requestProviderRemove(settings)}
          >
            Remove
          </Button>
        {/if}
      </div>
    </div>
  {/if}

  {#if settings}

    <section class="overflow-hidden rounded-xl border bg-card/40 shadow-sm">
      <header class="flex items-center gap-3 border-b px-5 py-4">
        <CalendarClock class="size-4 text-muted-foreground" />
        <div>
          <h3 class="text-sm font-semibold">Automatic backups</h3>
          <p class="mt-0.5 text-xs text-muted-foreground">
            Create complete snapshots with {settings.name} on a predefined
            interval or UTC cron schedule.
          </p>
        </div>
      </header>
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void saveSchedule();
        }}
      >
        <Field.Group class="p-5">
          <Field.Field>
            <Field.Label for="automatic-backup-schedule">Schedule</Field.Label>
            <Select.Root type="single" bind:value={scheduleMode}>
              <Select.Trigger id="automatic-backup-schedule" class="w-full">
                {SCHEDULE_OPTIONS.find((option) => option.value === scheduleMode)
                  ?.label ?? "Choose a schedule"}
              </Select.Trigger>
              <Select.Content>
                <Select.Group>
                  <Select.Label>Frequency</Select.Label>
                  {#each SCHEDULE_OPTIONS as option (option.value)}
                    <Select.Item value={option.value} label={option.label}>
                      {option.label}
                    </Select.Item>
                  {/each}
                </Select.Group>
              </Select.Content>
            </Select.Root>
            <Field.Description>
              Scheduled backups use this provider and briefly restart Gitadel.
            </Field.Description>
          </Field.Field>

          {#if scheduleMode === CUSTOM_SCHEDULE}
            <Field.Field data-invalid={!cronExplanation.valid}>
              <Field.Label for="automatic-backup-cron">Cron expression</Field.Label>
              <Input
                id="automatic-backup-cron"
                bind:value={customCron}
                class="font-mono"
                aria-invalid={!cronExplanation.valid}
                autocomplete="off"
                spellcheck="false"
                placeholder="0 2 * * *"
              />
              <Field.Description>
                Five-field cron uses minute, hour, day of month, month, and day of week.
                Six- and seven-field expressions may include seconds and year.
              </Field.Description>
            </Field.Field>
            <Alert.Root variant={cronExplanation.valid ? "default" : "destructive"}>
              <CalendarClock />
              <Alert.Title>
                {cronExplanation.valid ? "Cron translation" : "Invalid cron"}
              </Alert.Title>
              <Alert.Description>{cronExplanation.description}</Alert.Description>
            </Alert.Root>
          {/if}

          <div class="flex flex-wrap items-end justify-between gap-3">
            <dl class="text-sm">
              <dt class="text-xs text-muted-foreground">Next automatic backup</dt>
              <dd class="mt-1 font-medium">
                {#if scheduleDirty}
                  Save the schedule to calculate the next run.
                {:else if settings.next_backup_at}
                  {formatDate(settings.next_backup_at)}
                {:else}
                  Not scheduled
                {/if}
              </dd>
            </dl>
            <Button type="submit" disabled={working || !canSaveSchedule}>
              {#if savingSchedule}
                <LoaderCircle data-icon="inline-start" class="animate-spin" />
                Saving…
              {:else}
                Save schedule
              {/if}
            </Button>
          </div>
        </Field.Group>
      </form>
    </section>

    <section class="overflow-hidden rounded-xl border bg-card/40 shadow-sm">
      <header class="flex flex-wrap items-center justify-between gap-3 border-b px-5 py-4">
        <div class="flex items-center gap-3">
          <ArchiveRestore class="size-4 text-muted-foreground" />
          <div>
            <h3 class="text-sm font-semibold">Snapshots</h3>
            <p class="mt-0.5 text-xs text-muted-foreground">
              Each snapshot includes the database, repositories, LFS, assets, host key, and effective configuration.
            </p>
          </div>
        </div>
        <Button type="button" variant="outline" size="sm" disabled={working} onclick={() => void refreshSnapshots()}>
          <RefreshCw class="size-3.5" /> Refresh
        </Button>
      </header>
      <form
        class="grid gap-3 border-b px-5 py-4 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-end"
        onsubmit={(event) => {
          event.preventDefault();
          void createBackup();
        }}
      >
        <label class="grid gap-1.5 text-sm font-medium">
          Backup name <span class="text-xs font-normal text-muted-foreground">(optional)</span>
          <input
            class="rounded-md border bg-background px-3 py-2 outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
            bind:value={backupName}
            maxlength="80"
            pattern="[A-Za-z0-9 _.-]+"
            placeholder="Before platform upgrade"
          />
        </label>
        <Button type="submit" disabled={working}>
          {#if creating}
            <LoaderCircle class="size-3.5 animate-spin" />
            Creating backup…
          {:else}
            <CloudUpload class="size-3.5" />
            Create backup
          {/if}
        </Button>
      </form>

      {#if creating && backupProgress}
        <div
          class="border-b bg-muted/20 px-5 py-4"
          role="status"
          aria-live="polite"
        >
          <div class="flex items-start gap-3">
            {#if backupProgress.phase === "completed"}
              <CheckCircle2 class="mt-0.5 size-4 shrink-0 text-emerald-400" />
            {:else}
              <LoaderCircle class="mt-0.5 size-4 shrink-0 animate-spin text-primary" />
            {/if}
            <div class="min-w-0 flex-1">
              <div class="flex items-center justify-between gap-3">
                <p class="text-sm font-medium">{backupProgress.message}</p>
                {#if backupProgressPercent !== null}
                  <span class="text-xs tabular-nums text-muted-foreground">
                    {backupProgressPercent}%
                  </span>
                {/if}
              </div>
              {#if backupProgressPercent !== null}
                <div
                  class="mt-2 h-1.5 overflow-hidden rounded-full bg-muted"
                  aria-label="Backup upload progress"
                  aria-valuemin="0"
                  aria-valuemax="100"
                  aria-valuenow={backupProgressPercent}
                  role="progressbar"
                >
                  <div
                    class="h-full rounded-full bg-primary transition-[width] duration-300"
                    style:width={`${backupProgressPercent}%`}
                  ></div>
                </div>
                <p class="mt-1.5 text-xs text-muted-foreground">
                  {formatBytes(backupProgress.processed_bytes ?? 0)} of
                  {formatBytes(backupProgress.total_bytes ?? 0)} uploaded
                </p>
              {:else}
                <p class="mt-1 text-xs text-muted-foreground">
                  The server remains unavailable to normal requests while this phase runs.
                </p>
              {/if}
            </div>
          </div>
        </div>
      {/if}

      {#if snapshots.length === 0}
        <p class="p-5 text-sm text-muted-foreground">No snapshots found under this prefix.</p>
      {:else}
        <div class="divide-y">
          {#each snapshots as snapshot (snapshot.key)}
            <article class="flex flex-wrap items-center justify-between gap-4 px-5 py-4">
              <div class="min-w-0">
                <p class="truncate text-sm font-medium" title={snapshot.key}>
                  {snapshot.name ?? "Unnamed backup"}
                </p>
                <p class="mt-0.5 truncate font-mono text-xs text-muted-foreground">
                  {fileName(snapshot.key)}
                </p>
                <div class="mt-2 flex flex-wrap gap-1.5">
                  <Badge variant="outline">
                    <CalendarDays data-icon="inline-start" />
                    {formatDate(snapshot.created_at)}
                  </Badge>
                  <Badge variant="outline">
                    <HardDrive data-icon="inline-start" />
                    {formatBytes(snapshot.size)}
                  </Badge>
                  <Badge variant="outline">
                    <Tag data-icon="inline-start" />
                    Gitadel {snapshot.gitadel_version ?? "unknown"}
                  </Badge>
                  <Badge variant="secondary">
                    {#if isAutomaticBackup(snapshot)}
                      <Bot data-icon="inline-start" /> Auto-generated
                    {:else}
                      <UserRound data-icon="inline-start" /> Manual
                    {/if}
                  </Badge>
                </div>
              </div>
              <div class="flex flex-wrap items-center gap-2">
                <Button
                  href={`${providerPath(settings.id)}/backups/object?key=${encodeURIComponent(snapshot.key)}`}
                  download={fileName(snapshot.key)}
                  variant="outline"
                  size="sm"
                  disabled={working}
                >
                  <Download class="size-3.5" /> Download
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={working}
                  onclick={() => void validateSnapshot(snapshot)}
                >
                  Validate restore
                </Button>
                <Button
                  type="button"
                  variant="destructive"
                  size="sm"
                  disabled={working}
                  onclick={() => requestDelete(snapshot)}
                >
                  <Trash2 class="size-3.5" /> Delete
                </Button>
              </div>
            </article>
          {/each}
        </div>
      {/if}
    </section>
  {/if}

  {#if preflight}
    <section class="overflow-hidden rounded-xl border border-destructive/35 bg-destructive/3 shadow-sm">
      <header class="flex items-start gap-3 border-b border-destructive/20 px-5 py-4">
        <TriangleAlert class="mt-0.5 size-4 shrink-0 text-destructive" />
        <div>
          <h3 class="text-sm font-semibold">Replace this instance</h3>
          <p class="mt-1 text-xs leading-relaxed text-muted-foreground">
            Restoring <span class="font-medium text-foreground">{preflight.backup_name ?? "this backup"}</span>
            from Gitadel {preflight.gitadel_version ?? "unknown"} will stop Gitadel and completely replace the current database, users, settings, repositories, LFS data, assets, and SSH host key.
          </p>
        </div>
      </header>
      <div class="grid gap-5 p-5">
        <dl class="grid gap-3 rounded-lg border bg-background/50 p-4 text-sm sm:grid-cols-3">
          <div>
            <dt class="text-xs text-muted-foreground">Created</dt>
            <dd class="mt-1 font-medium">{formatDate(preflight.created_at)}</dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">Verified files</dt>
            <dd class="mt-1 font-medium">{preflight.file_count}</dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">Restored size</dt>
            <dd class="mt-1 font-medium">{formatBytes(preflight.uncompressed_size)}</dd>
          </div>
        </dl>

        {#if preflight.version_warning}
          <p class="rounded-md border border-amber-500/30 bg-amber-500/8 p-3 text-sm text-amber-200">
            {preflight.version_warning} Review release notes and compatibility before continuing.
          </p>
        {/if}

        <label class="flex items-start gap-3 rounded-lg border p-4 text-sm">
          <input class="mt-0.5 size-4 accent-primary" type="checkbox" bind:checked={createSafetyBackup} />
          <span>
            <span class="flex items-center gap-2 font-medium"><ShieldCheck class="size-4 text-emerald-400" /> Back up the current instance first</span>
            <span class="mt-1 block text-xs leading-relaxed text-muted-foreground">
              Recommended. Restore will stop if this safety backup cannot be created with {settings?.name ?? "the selected provider"}.
            </span>
          </span>
        </label>

        {#if !createSafetyBackup}
          <p class="rounded-md border border-amber-500/30 bg-amber-500/8 p-3 text-sm text-amber-200">
            No recovery snapshot of the current instance will be created.
          </p>
        {/if}

        <label class="grid gap-1.5 text-sm font-medium">
          Administrator password
          <input
            class="rounded-md border bg-background px-3 py-2 outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
            bind:value={password}
            type="password"
            autocomplete="current-password"
            required
          />
        </label>
        <label class="flex items-start gap-3 text-sm">
          <input class="mt-0.5 size-4 accent-destructive" type="checkbox" bind:checked={confirmReplacement} />
          <span>I understand that all current instance data will be replaced.</span>
        </label>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="outline" disabled={working} onclick={() => (preflight = null)}>Cancel</Button>
          <Button
            type="button"
            variant="destructive"
            disabled={working || !password || !confirmReplacement}
            onclick={() => void restoreBackup()}
          >
            Restore this backup
          </Button>
        </div>
      </div>
    </section>
  {/if}
</section>

<Dialog.Root bind:open={providerEditorOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>
        {editingProvider ? `Configure ${editingProvider.name}` : "Add backup provider"}
      </Dialog.Title>
      <Dialog.Description>
        {editingProvider
          ? "Update this destination, test it, then save the exact configuration."
          : "Choose a provider, test the destination, then add it to the backup catalog."}
      </Dialog.Description>
    </Dialog.Header>
    {#key editorRevision}
      <BackupProviderEditor
        providers={providerCatalog}
        connection={editingProvider}
        onsave={saveProvider}
        ontest={testProvider}
        oncancel={() => (providerEditorOpen = false)}
      />
    {/key}
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={providerRemoveDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>
        Remove {pendingProviderRemove?.name ?? "this backup provider"}?
      </AlertDialog.Title>
      <AlertDialog.Description>
        Existing backup files remain at the destination, but Gitadel will no
        longer list or schedule them.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => void confirmProviderRemove()}
      >
        Remove provider
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root bind:open={deleteDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>
        Delete {pendingDelete?.name ?? "this backup"}?
      </AlertDialog.Title>
      <AlertDialog.Description>
        This permanently removes {pendingDelete ? fileName(pendingDelete.key) : "the snapshot"}
        from {settings?.name ?? "the selected provider"}. It cannot be restored after deletion.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={deleting}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={deleting}
        onclick={() => void deleteSnapshot()}
      >
        {#if deleting}
          <LoaderCircle class="size-3.5 animate-spin" /> Deleting…
        {:else}
          Delete backup
        {/if}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
