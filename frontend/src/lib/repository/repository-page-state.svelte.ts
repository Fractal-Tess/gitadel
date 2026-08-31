import { toast } from "svelte-sonner";
import {
  loadRepositoryBootstrap,
  preloadRepository,
} from "$lib/repository/repository-preload.js";
import {
  preloadRepositoryData,
  clearRepositoryDataCache,
} from "$lib/repository/repository-data-cache.js";
import { clearRepositoryPreload } from "$lib/repository/repository-preload.js";
import { ApiFailure, requestJson } from "$lib/api/transport.js";
import {
  refsSchema,
  treeSchema,
  type Repository,
  type Tree,
} from "$lib/api/repositories.js";
import type { AuthStatus } from "$lib/api/auth.js";
import type { AppState } from "$lib/state/app-state.svelte.js";
import { copyText } from "$lib/clipboard.js";
import {
  errorMessage,
  repositoryApi,
  type RepositoryFeatureContext,
} from "./state/shared.js";
import { RepositoryBrowserState } from "./state/browser-state.svelte.js";
import { RepositoryActionsState } from "./state/actions-state.svelte.js";
import { RepositoryIssuesState } from "./state/issues-state.svelte.js";
import { RepositoryReleasesState } from "./state/releases-state.svelte.js";
import { RepositoryWebhooksState } from "./state/webhooks-state.svelte.js";
import { RepositorySettingsState } from "./state/settings-state.svelte.js";

export type RepositoryView =
  | "overview"
  | "history"
  | "commit"
  | "actions"
  | "tags"
  | "releases"
  | "issues"
  | "settings"
  | "integrations";
export type CopyTarget = "http" | "ssh";
const views: readonly RepositoryView[] = [
  "overview",
  "history",
  "commit",
  "actions",
  "tags",
  "releases",
  "issues",
  "settings",
  "integrations",
];
function isRepositoryView(value: string | null): value is RepositoryView {
  return views.includes(value as RepositoryView);
}

export class RepositoryPageState {
  readonly namespace: string;
  readonly name: string;
  readonly scope: AppState["authorizationScope"];
  readonly browser: RepositoryBrowserState;
  readonly actions: RepositoryActionsState;
  readonly issues: RepositoryIssuesState;
  readonly releases: RepositoryReleasesState;
  readonly webhooks: RepositoryWebhooksState;
  readonly settings: RepositorySettingsState;
  repository = $state.raw<Repository | null>(null);
  authStatus = $state.raw<AuthStatus | null>(null);
  view = $state<RepositoryView>("overview");
  integrationProvider = $state<string | null>(null);
  settingsTab = $state("general");
  revision = $state("");
  repositoryPath = $state("");
  commitOid = $state("");
  historyPage = $state(1);
  issueNumber = $state<number | null>(null);
  actionRunId = $state<string | null>(null);
  actionJobId = $state<number | null>(null);
  actionCommit = $state("");
  actionPage = $state(1);
  loading = $state(true);
  emptyRepository = $state(false);
  error = $state<string | null>(null);
  copied = $state<CopyTarget | null>(null);
  #app: AppState;
  #repositoryRequestSequence = 0;
  #viewRequestController: AbortController | null = null;
  #supplementaryRefreshTimers: number[] = [];
  #destroyed = false;

  constructor(namespace: string, name: string, app: AppState) {
    this.namespace = namespace;
    this.name = name;
    this.#app = app;
    this.scope = app.authorizationScope;
    const context: RepositoryFeatureContext = {
      locator: this,
      scope: this.scope,
      isScopeCurrent: () => this.isScopeCurrent(),
    };
    this.browser = new RepositoryBrowserState(context, {
      setError: (message) => this.setError(message),
      loadActionStatuses: (oids, init) =>
        void this.actions.loadActionStatuses(oids, init),
    });
    this.actions = new RepositoryActionsState(context, {
      setError: (message) => this.setError(message),
      navigate: (_view, options) => this.navigate("actions", options),
      getView: () => this.view,
      getActionSelection: () => ({
        runId: this.actionRunId,
        jobId: this.actionJobId,
        commit: this.actionCommit,
        page: this.actionPage,
      }),
      isScopeCurrent: () => this.isScopeCurrent(),
    });
    this.issues = new RepositoryIssuesState(context, {
      setError: (message) => this.setError(message),
      invalidateData: (datasets) =>
        clearRepositoryDataCache(
          this.namespace,
          this.name,
          this.scope,
          datasets,
        ),
      selectIssue: (number) => this.selectIssue(number),
      getIssueNumber: () => this.issueNumber,
      isScopeCurrent: () => this.isScopeCurrent(),
    });
    this.releases = new RepositoryReleasesState(context, {
      setError: (message) => this.setError(message),
      isScopeCurrent: () => this.isScopeCurrent(),
    });
    this.webhooks = new RepositoryWebhooksState(context, {
      setError: (message) => this.setError(message),
      canManage: () => Boolean(this.repository?.can_manage),
      isScopeCurrent: () => this.isScopeCurrent(),
    });
    this.settings = new RepositorySettingsState(context, {
      setError: (message) => this.setError(message),
      getRepository: () => this.repository,
      setRepository: (repository) => {
        this.repository = repository;
      },
      onDefaultBranchChanged: async () => {
        this.revision = this.repository?.default_branch ?? this.revision;
        await this.initialize();
      },
      invalidatePreload: () =>
        clearRepositoryPreload(this.namespace, this.name, this.scope),
      invalidateData: (datasets) =>
        clearRepositoryDataCache(
          this.namespace,
          this.name,
          this.scope,
          datasets,
        ),
      isAuthenticated: () => Boolean(this.authStatus?.authenticated),
      redirectToLogin: (returnTo) =>
        window.location.assign(
          `/login?returnTo=${encodeURIComponent(returnTo)}`,
        ),
    });
  }

  get httpCloneUrl(): string {
    return typeof window === "undefined"
      ? ""
      : `${window.location.origin}/${this.namespace}/${this.name}.git`;
  }
  get rawUrl(): string {
    if (!this.browser.blob) return "";
    return `${repositoryApi(this, "/raw")}?${new URLSearchParams({ rev: this.revision, path: this.browser.blob.path })}`;
  }
  private isScopeCurrent(): boolean {
    return !this.#destroyed && this.#app.authorizationScope === this.scope;
  }
  private setError(message: string): void {
    this.error = message || null;
  }

  async initialize(): Promise<void> {
    const sequence = ++this.#repositoryRequestSequence;
    this.loading = true;
    this.error = null;
    try {
      const { repository, refs, authStatus, organizations, topics } =
        await loadRepositoryBootstrap(this.namespace, this.name, this.scope);
      if (
        sequence !== this.#repositoryRequestSequence ||
        !this.isScopeCurrent()
      )
        return;
      this.repository = repository;
      this.browser.refs = refs;
      this.authStatus = authStatus;
      this.settings.topics = topics.topics;
      this.settings.ownedNamespaces = [
        ...(authStatus.user ? [authStatus.user.username] : []),
        ...organizations
          .filter((organization) => organization.role === "owner")
          .map((organization) => organization.slug),
      ];
      this.readLocation(repository);
      await this.loadView();
      if (
        sequence !== this.#repositoryRequestSequence ||
        !this.isScopeCurrent()
      )
        return;
      preloadRepository(this.namespace, this.name, this.scope, this.revision);
      void preloadRepositoryData(
        this.namespace,
        this.name,
        this.revision,
        this.scope,
        repository.can_manage,
      );
    } catch (caught) {
      if (sequence === this.#repositoryRequestSequence)
        this.setError(errorMessage(caught));
    } finally {
      if (sequence === this.#repositoryRequestSequence) this.loading = false;
    }
  }

  async loadView(): Promise<void> {
    const controller = new AbortController();
    this.#viewRequestController?.abort();
    this.#viewRequestController = controller;
    const init = { signal: controller.signal };
    this.error = null;
    this.emptyRepository = false;
    this.browser.resetView(this.revision);
    this.browser.selectedPath = this.repositoryPath;
    if (this.view !== "overview") void this.browser.loadStats(this.revision);
    try {
      switch (this.view) {
        case "overview":
          this.emptyRepository = await this.browser.loadOverview(
            this.revision,
            this.repositoryPath,
            init,
          );
          break;
        case "history":
          await this.browser.loadHistory(this.revision, this.historyPage, init);
          break;
        case "commit":
          if (!this.commitOid) throw new Error("No commit was selected.");
          await this.browser.loadCommit(this.commitOid, init);
          break;
        case "actions":
          await this.actions.loadActions(init);
          break;
        case "releases":
          await this.releases.loadReleases(init);
          break;
        case "issues":
          await Promise.all([
            this.issues.loadIssues(init),
            this.issues.loadIssueLabels(init),
            this.issueNumber
              ? this.issues.loadIssue(this.issueNumber, init)
              : Promise.resolve(),
          ]);
          break;
        case "settings":
          await this.webhooks.loadWebhooks(init);
          break;
        case "tags":
        case "integrations":
          if (this.view === "integrations" && !this.integrationProvider)
            this.view = "settings";
          break;
      }
    } catch (caught) {
      if (!(caught instanceof DOMException && caught.name === "AbortError")) {
        if (caught instanceof ApiFailure && caught.status === 404)
          this.emptyRepository = true;
        else this.setError(errorMessage(caught));
      }
    } finally {
      if (this.#viewRequestController === controller)
        this.#viewRequestController = null;
      if (!this.#destroyed) this.scheduleSupplementaryRefresh();
    }
  }

  navigate(
    nextView: RepositoryView,
    options: {
      path?: string;
      oid?: string;
      page?: number;
      rev?: string;
      issue?: number | null;
      provider?: string;
      settingsTab?: string;
      run?: string | null;
      job?: number | null;
      commit?: string;
    } = {},
  ): void {
    this.view =
      (nextView === "settings" || nextView === "integrations") &&
      !this.repository?.can_manage
        ? "overview"
        : nextView;
    this.integrationProvider =
      nextView === "integrations" ? (options.provider ?? null) : null;
    this.settingsTab =
      nextView === "settings"
        ? (options.settingsTab ?? "general")
        : nextView === "integrations"
          ? "integrations"
          : "general";
    this.repositoryPath = options.path ?? "";
    this.commitOid = options.oid ?? "";
    this.historyPage = options.page ?? 1;
    this.issueNumber = nextView === "issues" ? (options.issue ?? null) : null;
    this.revision = options.rev ?? this.revision;
    this.actionRunId = nextView === "actions" ? (options.run ?? null) : null;
    this.actionJobId = nextView === "actions" ? (options.job ?? null) : null;
    this.actionCommit = nextView === "actions" ? (options.commit ?? "") : "";
    this.actionPage = nextView === "actions" ? (options.page ?? 1) : 1;
    this.writeLocation();
    void this.loadView();
  }
  restoreLocation(): void {
    if (!this.repository) return;
    this.readLocation(this.repository);
    void this.loadView();
  }
  changeRevision(nextRevision: string): void {
    this.navigate(this.view === "commit" ? "overview" : this.view, {
      path: this.repositoryPath,
      rev: nextRevision,
    });
  }
  selectEntry(entry: Tree["entries"][number]): void {
    this.browser.selectedPath = entry.path;
    if (entry.kind === "tree") {
      void this.browser.toggleDirectory(entry.path, this.revision);
      return;
    }
    this.view = "overview";
    this.repositoryPath = entry.path;
    this.commitOid = "";
    this.historyPage = 1;
    this.writeLocation();
    void this.browser.selectBlob(entry.path, this.revision);
  }
  selectIssue(number: number | null): void {
    this.issues.selectedIssue = null;
    this.issues.issueComments = [];
    this.navigate("issues", { issue: number });
  }
  async copyCloneUrl(target: CopyTarget): Promise<void> {
    const value =
      target === "http" ? this.httpCloneUrl : this.repository?.ssh_clone_url;
    if (!value) return;
    try {
      await copyText(value);
      this.copied = target;
      toast.success(`${target.toUpperCase()} clone URL copied`, {
        description: value,
      });
      window.setTimeout(() => {
        if (this.copied === target) this.copied = null;
      }, 1600);
    } catch {
      toast.error("The clone URL could not be copied.");
    }
  }
  handleVisibilityChange(): void {
    this.actions.handleVisibilityChange();
  }
  destroy(): void {
    if (this.#destroyed) return;
    this.#destroyed = true;
    this.#repositoryRequestSequence += 1;
    this.#viewRequestController?.abort();
    this.#viewRequestController = null;
    for (const timer of this.#supplementaryRefreshTimers)
      window.clearTimeout(timer);
    this.#supplementaryRefreshTimers = [];
    this.browser.destroy();
    this.actions.destroy();
    this.issues.destroy();
    this.releases.destroy();
    this.webhooks.destroy();
  }
  private scheduleSupplementaryRefresh(): void {
    for (const timer of this.#supplementaryRefreshTimers)
      window.clearTimeout(timer);
    this.#supplementaryRefreshTimers = [];
    if (
      this.browser.refs?.size_bytes != null &&
      this.browser.commitCount != null
    )
      return;
    const sequence = this.#repositoryRequestSequence;
    const revision = this.revision;
    for (const delay of [1000, 5000]) {
      this.#supplementaryRefreshTimers.push(
        window.setTimeout(async () => {
          if (
            sequence !== this.#repositoryRequestSequence ||
            revision !== this.revision
          )
            return;
          const refs =
            this.browser.refs?.size_bytes == null
              ? await requestJson(
                  repositoryApi(this, "/refs"),
                  refsSchema,
                ).catch(() => null)
              : null;
          const tree =
            this.browser.commitCount == null
              ? await requestJson(
                  `${repositoryApi(this, "/tree")}?${new URLSearchParams({ rev: revision })}`,
                  treeSchema,
                ).catch(() => null)
              : null;
          if (
            sequence !== this.#repositoryRequestSequence ||
            revision !== this.revision
          )
            return;
          if (refs) this.browser.refs = refs;
          if (tree?.commit_count != null)
            this.browser.commitCount = tree.commit_count;
        }, delay),
      );
    }
  }
  private readLocation(repository: Repository): void {
    const parameters = new URLSearchParams(window.location.search);
    const requestedView = parameters.get("view");
    const view = isRepositoryView(requestedView) ? requestedView : "overview";
    this.view =
      view === "settings" && !repository.can_manage ? "overview" : view;
    this.settingsTab =
      this.view === "settings"
        ? parameters.get("tab") || "general"
        : this.view === "integrations"
          ? "integrations"
          : "general";
    this.revision = parameters.get("rev") || repository.default_branch;
    this.repositoryPath = parameters.get("path") || "";
    this.commitOid = parameters.get("oid") || "";
    this.historyPage = Math.max(1, Number(parameters.get("page")) || 1);
    this.actionRunId = view === "actions" ? parameters.get("run") : null;
    const actionJobId = Number(parameters.get("job"));
    this.actionJobId =
      view === "actions" && actionJobId > 0 ? actionJobId : null;
    this.actionCommit =
      view === "actions" ? parameters.get("commit") || "" : "";
    this.actionPage =
      view === "actions" ? Math.max(1, Number(parameters.get("page")) || 1) : 1;
    const issueNumber = Number(parameters.get("issue"));
    this.issueNumber =
      view === "issues" && issueNumber > 0 ? issueNumber : null;
    this.integrationProvider =
      view === "integrations" ? parameters.get("provider") : null;
  }
  private writeLocation(): void {
    const parameters = new URLSearchParams();
    if (this.view !== "overview") parameters.set("view", this.view);
    if (this.view === "settings" && this.settingsTab !== "general")
      parameters.set("tab", this.settingsTab);
    if (this.revision && this.revision !== this.repository?.default_branch)
      parameters.set("rev", this.revision);
    if (this.repositoryPath) parameters.set("path", this.repositoryPath);
    if (this.commitOid) parameters.set("oid", this.commitOid);
    if (this.view === "history" && this.historyPage > 1)
      parameters.set("page", String(this.historyPage));
    if (this.view === "issues" && this.issueNumber)
      parameters.set("issue", String(this.issueNumber));
    if (this.view === "integrations" && this.integrationProvider)
      parameters.set("provider", this.integrationProvider);
    if (this.view === "actions") {
      if (this.actionRunId) parameters.set("run", this.actionRunId);
      if (this.actionJobId) parameters.set("job", String(this.actionJobId));
      if (this.actionCommit) parameters.set("commit", this.actionCommit);
      if (this.actionPage > 1) parameters.set("page", String(this.actionPage));
    }
    const search = parameters.toString();
    window.history.pushState(
      {},
      "",
      `/${this.namespace}/${this.name}${search ? `?${search}` : ""}`,
    );
  }
}
