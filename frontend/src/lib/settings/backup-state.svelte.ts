import { toString as cronToString } from "cronstrue";
import { toast } from "svelte-sonner";
import { z } from "zod";

import {
  backupProgressSchema,
  backupProviderSchema,
  backupProviderTestSchema,
  backupScheduledSchema,
  backupSnapshotSchema,
  restorePreflightSchema,
  type BackupProgress,
  type BackupProvider,
  type BackupProviderCatalogItem,
  type BackupSnapshot,
  type RestorePreflight,
} from "$lib/api/backups.js";
import {
  ApiFailure,
  errorMessage,
  jsonBody,
  requestEmpty,
  requestJson,
} from "$lib/api/transport.js";
import type { BackupProviderEditorValue } from "$lib/components/settings/backup-provider-editor.svelte";
import {
  clearBackupSettingsCache,
  loadBackupSettings,
} from "$lib/settings/backup-settings-cache.js";
import type { AppState } from "$lib/state/app-state.svelte.js";

export const CUSTOM_SCHEDULE = "custom";
const DEFAULT_CUSTOM_CRON = "0 2 * * *";
export const SCHEDULE_OPTIONS = [
  { value: "", label: "Automatic backups off" },
  { value: "0 * * * *", label: "Every hour" },
  { value: "0 */6 * * *", label: "Every 6 hours" },
  { value: "0 2 * * *", label: "Every day at 02:00 UTC" },
  { value: "0 2 * * 0", label: "Every Sunday at 02:00 UTC" },
  { value: CUSTOM_SCHEDULE, label: "Custom cron" },
];
const BACKUP_DEADLINE = 10 * 60 * 1_000;
// A restore replaces every repository, so it may run far longer than a
// backup; the server must at least have stopped well before that.
const RESTORE_SHUTDOWN_DEADLINE = 2 * 60 * 1_000;
const RESTORE_DEADLINE = 60 * 60 * 1_000;

class BackupProgressFailure extends Error {}

/** Everything the backup page can be waiting on, each tracked on its own. */
type Operation =
  | "loading"
  | "removingProvider"
  | "savingSchedule"
  | "refreshing"
  | "creating"
  | "deleting"
  | "validating"
  | "startingRestore";

export function backupProviderPath(id: string): string {
  return `/api/v1/admin/backup/providers/${encodeURIComponent(id)}`;
}

export function backupFileName(key: string): string {
  return key.split("/").at(-1) ?? key;
}

export function isAutomaticBackup(snapshot: BackupSnapshot): boolean {
  return (
    snapshot.name === "pre restore safety" ||
    snapshot.name === "scheduled automatic backup"
  );
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

function modeForSchedule(schedule: string | null): string {
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

/**
 * The state behind the backup administration page: providers, their
 * snapshots, the create and restore flows, and what each is waiting on.
 *
 * Methods are arrow properties so components can pass them as handlers.
 */
export class BackupState {
  providerCatalog = $state.raw<BackupProviderCatalogItem[]>([]);
  providers = $state.raw<BackupProvider[]>([]);
  selectedProviderId = $state<string | null>(null);
  readonly settings = $derived(
    this.providers.find(
      (provider) => provider.id === this.selectedProviderId,
    ) ?? null,
  );
  snapshots = $state.raw<BackupSnapshot[]>([]);
  preflight = $state.raw<RestorePreflight | null>(null);
  pendingDelete = $state.raw<BackupSnapshot | null>(null);
  deleteDialogOpen = $state(false);
  backupProgress = $state.raw<BackupProgress | null>(null);
  providerEditorOpen = $state(false);
  createDialogOpen = $state(false);
  createProviderId = $state<string | undefined>(undefined);
  editingProvider = $state.raw<BackupProvider | null>(null);
  editorRevision = $state(0);
  providerRemoveDialogOpen = $state(false);
  pendingProviderRemove = $state.raw<BackupProvider | null>(null);
  backupName = $state("");
  scheduleMode = $state("");
  customCron = $state(DEFAULT_CUSTOM_CRON);
  readonly scheduleExpression = $derived(
    this.scheduleMode === CUSTOM_SCHEDULE
      ? this.customCron.trim() || null
      : this.scheduleMode || null,
  );
  readonly cronExplanation = $derived(explainCron(this.customCron));
  readonly scheduleDirty = $derived(
    this.scheduleExpression !== (this.settings?.schedule ?? null),
  );
  readonly canSaveSchedule = $derived(
    this.scheduleDirty &&
      (this.scheduleMode !== CUSTOM_SCHEDULE || this.cronExplanation.valid),
  );
  readonly backupProgressPercent = $derived.by(() => {
    const progress = this.backupProgress;
    if (
      progress?.processed_bytes === null ||
      progress?.total_bytes === null ||
      !progress?.total_bytes
    ) {
      return null;
    }
    return Math.min(
      100,
      Math.round((progress.processed_bytes / progress.total_bytes) * 100),
    );
  });
  password = $state("");
  createSafetyBackup = $state(true);
  confirmReplacement = $state(false);
  /** Set from the moment a restore is accepted until the page reloads. */
  restoring = $state(false);
  error = $state<string | null>(null);

  // How many runs of each operation are in flight.
  #running = $state<Record<Operation, number>>({
    loading: 0,
    removingProvider: 0,
    savingSchedule: 0,
    refreshing: 0,
    creating: 0,
    deleting: 0,
    validating: 0,
    startingRestore: 0,
  });
  /** Whether any operation is in flight; the page locks its actions on it. */
  readonly busy = $derived(
    Object.values(this.#running).some((count) => count > 0),
  );

  // Aborted when the page is left, which ends every poll started here.
  readonly #lifetime = new AbortController();
  #progressFailure: string | null = null;
  #progressSource: EventSource | null = null;

  constructor(private readonly app: AppState) {}

  get loading(): boolean {
    return this.#running.loading > 0;
  }
  get removingProvider(): boolean {
    return this.#running.removingProvider > 0;
  }
  get savingSchedule(): boolean {
    return this.#running.savingSchedule > 0;
  }
  get refreshing(): boolean {
    return this.#running.refreshing > 0;
  }
  get creating(): boolean {
    return this.#running.creating > 0;
  }
  get deleting(): boolean {
    return this.#running.deleting > 0;
  }
  get validating(): boolean {
    return this.#running.validating > 0;
  }
  get startingRestore(): boolean {
    return this.#running.startingRestore > 0;
  }

  destroy = (): void => {
    this.#lifetime.abort();
    this.#progressSource?.close();
  };

  initialize = async (): Promise<void> => {
    const scope = this.app.authorizationScope;
    await this.#run(
      "loading",
      async () => {
        const loaded = await loadBackupSettings(scope);
        const providersValue = loaded.providers;
        const selected =
          providersValue.find(
            (provider) => provider.id === this.selectedProviderId,
          ) ??
          providersValue.find(
            (provider) => provider.id === loaded.selectedProviderId,
          ) ??
          null;
        const snapshotsValue =
          selected?.id === loaded.selectedProviderId
            ? loaded.snapshots
            : selected
              ? await this.#fetchSnapshots(selected.id)
              : [];
        if (this.app.authorizationScope !== scope) {
          void this.initialize();
          return;
        }
        this.providerCatalog = loaded.providerCatalog;
        this.providers = providersValue;
        this.selectedProviderId = selected?.id ?? null;
        this.#applyProvider(selected);
        this.snapshots = snapshotsValue;
      },
      "inline",
    );
  };

  #applyProvider(provider: BackupProvider | null): void {
    this.scheduleMode = modeForSchedule(provider?.schedule ?? null);
    if (this.scheduleMode === CUSTOM_SCHEDULE) {
      this.customCron = provider?.schedule ?? DEFAULT_CUSTOM_CRON;
    }
  }

  async #fetchSnapshots(
    providerId = this.selectedProviderId,
    init: RequestInit = {},
  ): Promise<BackupSnapshot[]> {
    if (!providerId) return [];
    return requestJson(
      `${backupProviderPath(providerId)}/backups`,
      z.array(backupSnapshotSchema),
      init,
    );
  }

  /** Resolves after `milliseconds`, or at once when the page is left. */
  #pause(milliseconds: number): Promise<void> {
    const signal = this.#lifetime.signal;
    return new Promise((resolve) => {
      if (signal.aborted) {
        resolve();
        return;
      }
      const finish = () => {
        window.clearTimeout(timer);
        signal.removeEventListener("abort", finish);
        resolve();
      };
      const timer = window.setTimeout(finish, milliseconds);
      signal.addEventListener("abort", finish);
    });
  }

  openCreateProvider = (): void => {
    this.editingProvider = null;
    this.editorRevision += 1;
    this.providerEditorOpen = true;
  };

  openConfigureProvider = (provider: BackupProvider): void => {
    this.editingProvider = provider;
    this.editorRevision += 1;
    this.providerEditorOpen = true;
  };

  openCreateBackup = (): void => {
    this.createProviderId =
      this.selectedProviderId ?? this.providers[0]?.id ?? undefined;
    this.backupName = "";
    this.createDialogOpen = true;
  };

  testProvider = async (value: BackupProviderEditorValue) => {
    return requestJson(
      "/api/v1/admin/backup/providers/test",
      backupProviderTestSchema,
      {
        method: "POST",
        body: jsonBody(providerPayload(value)),
      },
    );
  };

  saveProvider = async (value: BackupProviderEditorValue): Promise<void> => {
    const saved = await requestJson(
      value.id
        ? backupProviderPath(value.id)
        : "/api/v1/admin/backup/providers",
      backupProviderSchema,
      {
        method: value.id ? "PUT" : "POST",
        body: jsonBody(providerPayload(value)),
      },
    );
    clearBackupSettingsCache();
    this.providers = value.id
      ? this.providers.map((provider) =>
          provider.id === value.id ? saved : provider,
        )
      : [...this.providers, saved];
    this.selectedProviderId = saved.id;
    this.#applyProvider(saved);
    this.providerEditorOpen = false;
    this.snapshots = [];
    const snapshotsLoaded = await this.#run(
      "loading",
      async () => {
        this.snapshots = await this.#fetchSnapshots(saved.id);
      },
      "inline",
    );
    if (snapshotsLoaded) toast.success(`${saved.name} saved.`);
  };

  selectProvider = async (provider: BackupProvider): Promise<void> => {
    this.selectedProviderId = provider.id;
    this.preflight = null;
    this.#applyProvider(provider);
    await this.#run(
      "loading",
      async () => {
        this.snapshots = await this.#fetchSnapshots(provider.id);
      },
      "inline",
    );
  };

  requestProviderRemove = (provider: BackupProvider): void => {
    this.pendingProviderRemove = provider;
    this.providerRemoveDialogOpen = true;
  };

  confirmProviderRemove = async (): Promise<void> => {
    const provider = this.pendingProviderRemove;
    if (!provider) return;
    await this.#run("removingProvider", async () => {
      await requestEmpty(backupProviderPath(provider.id), {
        method: "DELETE",
      });
      clearBackupSettingsCache();
      this.providers = this.providers.filter(
        (candidate) => candidate.id !== provider.id,
      );
      const selected = this.providers[0] ?? null;
      this.selectedProviderId = selected?.id ?? null;
      this.#applyProvider(selected);
      this.providerRemoveDialogOpen = false;
      this.pendingProviderRemove = null;
      this.snapshots = [];
      toast.success(`${provider.name} removed.`);
      this.snapshots = selected ? await this.#fetchSnapshots(selected.id) : [];
    });
  };

  saveSchedule = async (): Promise<void> => {
    const provider = this.settings;
    if (!provider || !this.canSaveSchedule) return;
    await this.#run("savingSchedule", async () => {
      const loaded = await requestJson(
        `${backupProviderPath(provider.id)}/schedule`,
        backupProviderSchema,
        {
          method: "PUT",
          body: jsonBody({ schedule: this.scheduleExpression }),
        },
      );
      clearBackupSettingsCache();
      this.providers = this.providers.map((candidate) =>
        candidate.id === provider.id ? loaded : candidate,
      );
      this.selectedProviderId = loaded.id;
      this.#applyProvider(loaded);
      toast.success(
        loaded.schedule
          ? "Automatic backup schedule saved."
          : "Automatic backups disabled.",
      );
    });
  };

  refreshSnapshots = async (): Promise<void> => {
    await this.#run("refreshing", async () => {
      this.snapshots = await this.#fetchSnapshots();
      toast.success("Backup list refreshed.");
    });
  };

  createBackup = async (): Promise<void> => {
    const provider =
      this.providers.find(
        (candidate) => candidate.id === this.createProviderId,
      ) ?? null;
    if (!provider) return;
    this.selectedProviderId = provider.id;
    this.#applyProvider(provider);
    this.preflight = null;
    this.snapshots = [];
    try {
      await this.#run("creating", async () => {
        const response = await requestJson(
          `${backupProviderPath(provider.id)}/backups`,
          backupScheduledSchema,
          {
            method: "POST",
            body: jsonBody({ name: this.backupName.trim() || null }),
          },
        );
        clearBackupSettingsCache();
        toast.success(response.message);
        this.createDialogOpen = false;
        this.backupProgress = {
          operation_id: response.operation_id,
          key: response.key,
          operation: "create",
          phase: "scheduled",
          message: "Stopping Gitadel services for a consistent snapshot.",
          processed_bytes: null,
          total_bytes: null,
        };
        this.#watchBackupProgress(response.operation_id);
        await this.#waitForBackup(response.key, provider.id);
        this.backupName = "";
        this.backupProgress = null;
        toast.success("Backup created and added to snapshots.");
      });
    } finally {
      this.#progressSource?.close();
      this.#progressSource = null;
    }
  };

  #watchBackupProgress(operationId: string): void {
    this.#progressFailure = null;
    this.#progressSource?.close();
    const source = new EventSource(
      `/api/v1/admin/backups/progress/${encodeURIComponent(operationId)}`,
    );
    this.#progressSource = source;
    source.onmessage = (event) => {
      let payload: unknown;
      try {
        payload = JSON.parse(event.data) as unknown;
      } catch {
        return;
      }
      const parsed = backupProgressSchema.safeParse(payload);
      if (!parsed.success || parsed.data.operation_id !== operationId) return;
      this.backupProgress = parsed.data;
      if (parsed.data.phase === "failed") {
        this.#progressFailure = parsed.data.message;
        source.close();
      } else if (parsed.data.phase === "completed") {
        source.close();
      }
    };
  }

  async #waitForBackup(key: string, providerId: string): Promise<void> {
    const signal = this.#lifetime.signal;
    const deadline = Date.now() + BACKUP_DEADLINE;
    await this.#pause(1_500);
    while (Date.now() < deadline) {
      signal.throwIfAborted();
      if (this.#progressFailure) {
        throw new BackupProgressFailure(this.#progressFailure);
      }
      try {
        const updated = await this.#fetchSnapshots(providerId, { signal });
        if (updated.some((snapshot) => snapshot.key === key)) {
          this.snapshots = updated;
          return;
        }
      } catch (caught) {
        signal.throwIfAborted();
        if (
          caught instanceof ApiFailure &&
          ![0, 404, 502, 503].includes(caught.status)
        ) {
          throw caught;
        }
        // The full server is unavailable while the maintenance server reports
        // progress; status 0 is a request that reached neither.
      }
      await this.#pause(1_000);
    }
    throw new Error("Backup creation did not finish within 10 minutes.");
  }

  requestDelete = (snapshot: BackupSnapshot): void => {
    this.pendingDelete = snapshot;
    this.deleteDialogOpen = true;
  };

  deleteSnapshot = async (): Promise<void> => {
    const snapshot = this.pendingDelete;
    const provider = this.settings;
    if (!snapshot || !provider) return;
    await this.#run("deleting", async () => {
      await requestEmpty(
        `${backupProviderPath(provider.id)}/backups/object?key=${encodeURIComponent(snapshot.key)}`,
        { method: "DELETE" },
      );
      clearBackupSettingsCache();
      this.snapshots = this.snapshots.filter(
        (candidate) => candidate.key !== snapshot.key,
      );
      if (this.preflight?.key === snapshot.key) this.preflight = null;
      toast.success("Backup deleted.");
      this.deleteDialogOpen = false;
      this.pendingDelete = null;
    });
  };

  validateSnapshot = async (snapshot: BackupSnapshot): Promise<void> => {
    const provider = this.settings;
    if (!provider) return;
    await this.#run("validating", async () => {
      this.preflight = await requestJson(
        `${backupProviderPath(provider.id)}/backups/preflight`,
        restorePreflightSchema,
        {
          method: "POST",
          body: jsonBody({ key: snapshot.key }),
        },
      );
      this.password = "";
      this.createSafetyBackup = true;
      this.confirmReplacement = false;
      toast.success(
        "Backup copied and verified. Review the restore details below.",
      );
    });
  };

  restoreBackup = async (): Promise<void> => {
    const selected = this.preflight;
    const provider = this.settings;
    if (!selected || !provider) return;
    await this.#run("startingRestore", async () => {
      const response = await requestJson(
        `${backupProviderPath(provider.id)}/backups/restore`,
        backupScheduledSchema,
        {
          method: "POST",
          body: jsonBody({
            token: selected.token,
            password: this.password,
            create_safety_backup: this.createSafetyBackup,
          }),
        },
      );
      toast.success(response.message);
      void this.#waitForRestart();
    });
  };

  /**
   * Reloads once the restored server answers again. A healthy answer only
   * counts after the server was seen down, since the server keeps answering
   * for a moment before the restore stops it.
   */
  async #waitForRestart(): Promise<void> {
    const signal = this.#lifetime.signal;
    const started = Date.now();
    let wentDown = false;
    this.restoring = true;
    try {
      await this.#pause(1_500);
      while (!signal.aborted) {
        const healthy = await this.#isHealthy(signal);
        if (signal.aborted) return;
        if (!healthy) {
          wentDown = true;
        } else if (wentDown) {
          window.location.reload();
          return;
        }
        const elapsed = Date.now() - started;
        if (!wentDown && elapsed > RESTORE_SHUTDOWN_DEADLINE) {
          this.error =
            "The restore was scheduled, but Gitadel did not stop to run it. Check the server logs, then reload this page.";
          return;
        }
        if (elapsed > RESTORE_DEADLINE) {
          this.error =
            "Gitadel did not come back within an hour of starting the restore. Check the server logs, then reload this page.";
          return;
        }
        await this.#pause(1_000);
      }
    } finally {
      this.restoring = false;
    }
  }

  async #isHealthy(signal: AbortSignal): Promise<boolean> {
    try {
      const response = await fetch("/healthz", { cache: "no-store", signal });
      return response.ok;
    } catch {
      // The server is expected to be unavailable during maintenance.
      return false;
    }
  }

  async #run(
    operation: Operation,
    task: () => Promise<void>,
    failureTarget: "inline" | "toast" = "toast",
  ): Promise<boolean> {
    this.#running[operation] += 1;
    this.error = null;
    try {
      await task();
      return true;
    } catch (caught) {
      // Nothing is left to report to once the page has been closed.
      if (this.#lifetime.signal.aborted) return false;
      const message = errorMessage(caught);
      if (
        caught instanceof BackupProgressFailure ||
        failureTarget === "inline"
      ) {
        this.error = message;
      } else {
        toast.error(message);
      }
      return false;
    } finally {
      this.#running[operation] -= 1;
    }
  }
}
