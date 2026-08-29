import {
  actionArtifactsSchema,
  actionLogsSchema,
  actionRunDetailSchema,
  actionRunSummarySchema,
  actionStatusesSchema,
  type ActionArtifact,
  type ActionCommitStatus,
  type ActionLogs,
  type ActionRunDetail,
  type ActionRuns,
} from "$lib/api/actions.js";
import { requestJson } from "$lib/api/transport.js";
import { loadRepositoryActionRuns } from "$lib/repository/repository-data-cache.js";
import type { RepositoryFeatureContext } from "./shared.js";
import { errorMessage, repositoryActionsApi } from "./shared.js";

type ActionsCallbacks = {
  setError(message: string): void;
  navigate(
    view: "actions",
    options?: {
      run?: string | null;
      job?: number | null;
      commit?: string;
      page?: number;
    },
  ): void;
  getView(): string;
  getActionSelection(): {
    runId: string | null;
    jobId: number | null;
    commit: string;
    page: number;
  };
  isScopeCurrent(): boolean;
};

export class RepositoryActionsState {
  readonly namespace: string;
  readonly name: string;
  readonly scope: RepositoryFeatureContext["scope"];
  readonly setError: (message: string) => void;
  readonly navigate: ActionsCallbacks["navigate"];
  readonly getView: ActionsCallbacks["getView"];
  readonly getActionSelection: ActionsCallbacks["getActionSelection"];
  readonly isScopeCurrent: ActionsCallbacks["isScopeCurrent"];

  actionArtifacts = $state.raw<ActionArtifact[]>([]);
  actionRuns = $state.raw<ActionRuns | null>(null);
  actionRun = $state.raw<ActionRunDetail | null>(null);
  actionLogs = $state.raw<ActionLogs | null>(null);
  actionCommitStatuses = $state.raw<Record<string, ActionCommitStatus>>({});
  actionsLoading = $state(false);
  actionArtifactsLoading = $state(false);
  actionLogsLoading = $state(false);
  actionArtifactsError = $state<string | null>(null);
  actionsPending = $state(false);

  #actionLogRequestSequence = 0;
  #actionsPollTimer: number | null = null;
  #destroyed = false;

  constructor(context: RepositoryFeatureContext, callbacks: ActionsCallbacks) {
    this.namespace = context.locator.namespace;
    this.name = context.locator.name;
    this.scope = context.scope;
    this.setError = callbacks.setError;
    this.navigate = callbacks.navigate;
    this.getView = callbacks.getView;
    this.getActionSelection = callbacks.getActionSelection;
    this.isScopeCurrent = callbacks.isScopeCurrent;
  }

  destroy(): void {
    this.#destroyed = true;
    if (this.#actionsPollTimer !== null)
      window.clearTimeout(this.#actionsPollTimer);
    this.#actionsPollTimer = null;
    this.#actionLogRequestSequence += 1;
  }

  async loadActions(init: RequestInit = {}, polling = false): Promise<void> {
    const selection = this.getActionSelection();
    if (!polling) this.actionsLoading = true;
    let actionRuns: ActionRuns | null = null;
    try {
      actionRuns = await loadRepositoryActionRuns(
        this.namespace,
        this.name,
        this.scope,
        selection.page,
        selection.commit,
        polling || !init.signal,
      );
      if (init.signal?.aborted || this.#destroyed || !this.isScopeCurrent())
        return;
      this.actionRuns = actionRuns;
      if (selection.runId) {
        const actionRun = await requestJson(
          repositoryActionsApi(
            this,
            `/runs/${encodeURIComponent(selection.runId)}`,
          ),
          actionRunDetailSchema,
          init,
        );
        if (!this.isScopeCurrent()) return;
        this.actionRun = actionRun;
        await this.loadActionArtifacts(init);
        if (
          selection.jobId &&
          actionRun.jobs.some((job) => job.id === selection.jobId)
        )
          await this.loadActionLog(init);
      } else {
        this.actionRun = null;
        this.actionLogs = null;
        this.actionArtifacts = [];
        this.actionArtifactsError = null;
      }
    } catch (caught) {
      if (
        !polling &&
        !(caught instanceof DOMException && caught.name === "AbortError")
      )
        this.setError(errorMessage(caught));
    } finally {
      if (!polling) this.actionsLoading = false;
      this.schedulePoll();
    }
  }

  async loadActionArtifacts(init: RequestInit = {}): Promise<void> {
    const runId = this.getActionSelection().runId;
    if (!runId) return;
    this.actionArtifactsLoading = true;
    this.actionArtifactsError = null;
    try {
      const response = await requestJson(
        repositoryActionsApi(
          this,
          `/runs/${encodeURIComponent(runId)}/artifacts`,
        ),
        actionArtifactsSchema,
        init,
      );
      if (
        this.getActionSelection().runId === runId &&
        !this.#destroyed &&
        this.isScopeCurrent()
      )
        this.actionArtifacts = response.artifacts;
    } catch (caught) {
      if (!(caught instanceof DOMException && caught.name === "AbortError"))
        this.actionArtifactsError = errorMessage(caught);
    } finally {
      if (this.getActionSelection().runId === runId && this.isScopeCurrent())
        this.actionArtifactsLoading = false;
    }
  }

  async loadActionStatuses(
    oids: string[],
    init: RequestInit = {},
  ): Promise<void> {
    if (!oids.length) return;
    try {
      const parameters = new URLSearchParams({
        oids: oids.slice(0, 50).join(","),
      });
      const response = await requestJson(
        `${repositoryActionsApi(this, "/statuses")}?${parameters}`,
        actionStatusesSchema,
        init,
      );
      if (!this.#destroyed && this.isScopeCurrent())
        this.actionCommitStatuses = Object.fromEntries(
          response.statuses.map((status) => [status.oid, status]),
        );
    } catch {
      // Commit checks are supplementary and do not block browsing.
    }
  }

  selectActionRun(runId: string): void {
    const current = this.getActionSelection();
    this.actionArtifacts = [];
    this.actionArtifactsError = null;
    this.navigate("actions", {
      run: runId,
      commit: current.commit,
      page: current.page,
    });
  }

  selectActionJob(jobId: number): void {
    const current = this.getActionSelection();
    this.actionLogs = null;
    this.navigate("actions", {
      run: current.runId,
      job: jobId,
      commit: current.commit,
      page: current.page,
    });
    // The coordinator updates the URL selection before invoking this loader.
    void this.loadActionLogFor(current.runId, jobId);
  }

  async loadActionLog(init: RequestInit = {}): Promise<void> {
    const selection = this.getActionSelection();
    await this.loadActionLogFor(selection.runId, selection.jobId, init);
  }

  private async loadActionLogFor(
    runId: string | null,
    jobId: number | null,
    init: RequestInit = {},
  ): Promise<void> {
    if (!runId || !jobId) return;
    const requestSequence = ++this.#actionLogRequestSequence;
    this.actionLogsLoading = true;
    try {
      const parameters = new URLSearchParams();
      if (this.actionLogs?.next_cursor)
        parameters.set("cursor", this.actionLogs.next_cursor);
      const suffix = parameters.size ? `?${parameters}` : "";
      const delta = await requestJson(
        repositoryActionsApi(
          this,
          `/runs/${encodeURIComponent(runId)}/jobs/${jobId}/logs${suffix}`,
        ),
        actionLogsSchema,
        init,
      );
      const current = this.getActionSelection();
      if (
        requestSequence !== this.#actionLogRequestSequence ||
        current.runId !== runId ||
        current.jobId !== jobId ||
        this.#destroyed ||
        !this.isScopeCurrent()
      )
        return;
      this.actionLogs = {
        text: `${this.actionLogs?.text ?? ""}${this.actionLogs?.text && delta.text ? "\n" : ""}${delta.text}`,
        next_cursor: delta.next_cursor,
        complete: delta.complete,
      };
    } finally {
      if (
        requestSequence === this.#actionLogRequestSequence &&
        this.isScopeCurrent()
      )
        this.actionLogsLoading = false;
    }
  }

  async cancelActionRun(): Promise<void> {
    const runId = this.getActionSelection().runId;
    if (!runId || this.actionsPending) return;
    this.actionsPending = true;
    try {
      await requestJson(
        repositoryActionsApi(this, `/runs/${encodeURIComponent(runId)}/cancel`),
        actionRunSummarySchema,
        { method: "POST" },
      );
      await this.loadActions({}, true);
    } catch (caught) {
      this.setError(errorMessage(caught));
    } finally {
      this.actionsPending = false;
    }
  }

  handleVisibilityChange(): void {
    if (document.visibilityState === "visible" && this.getView() === "actions")
      void this.loadActions({}, true);
  }

  private schedulePoll(): void {
    if (this.#destroyed) return;
    if (this.#actionsPollTimer !== null)
      window.clearTimeout(this.#actionsPollTimer);
    this.#actionsPollTimer = null;
    const current = this.getActionSelection();
    if (
      this.getView() !== "actions" ||
      document.visibilityState !== "visible" ||
      !this.actionRuns?.runs.some((run) =>
        ["queued", "running"].includes(run.status),
      )
    )
      return;
    this.#actionsPollTimer = window.setTimeout(
      () => {
        if (this.#destroyed) return;
        this.#actionsPollTimer = null;
        void this.loadActions({}, true);
      },
      current.runId ? 2_000 : 5_000,
    );
  }
}
