import { SvelteDate, SvelteSet } from "svelte/reactivity";
import { toast } from "svelte-sonner";

import { z } from "zod";

import {
  ApiFailure,
  blobSchema,
  commitSchema,
  diffSchema,
  historySchema,
  issueAttachmentSchema,
  issueCommentSchema,
  issueLabelSchema,
  issueSchema,
  issueUserSchema,
  jsonBody,
  languageStatSchema,
  refsSchema,
  releaseAssetSchema,
  releaseSchema,
  renderedMarkdownSchema,
  repositorySchema,
  requestEmpty,
  requestJson,
  topicsSchema,
  treeSchema,
  webhookSchema,
  webhookDeliverySchema,
  type AuthStatus,
  type Blob,
  type Commit,
  type Diff,
  type History,
  type Issue,
  type IssueAttachment,
  type IssueComment,
  type IssueLabel,
  type IssueUser,
  type LanguageStat,
  type Release,
  type Repository,
  type RepositoryRefs,
  type Tree,
  type Webhook,
  type WebhookDelivery,
} from "$lib/api.js";
import { copyText } from "$lib/clipboard.js";
import { escapeHtml, languageLabel } from "$lib/repository/format.js";
import {
  clearRepositoryPreload,
  loadRepositoryBootstrap,
  takePreloadedRepositoryOverview,
} from "$lib/repository/repository-preload.js";

export type RepositoryView =
  | "overview"
  | "history"
  | "commit"
  | "tags"
  | "releases"
  | "issues"
  | "settings";
export type CopyTarget = "http" | "ssh";

const MAX_HIGHLIGHT_CHARACTERS = 200_000;

function errorMessage(caught: unknown): string {
  if (caught instanceof ApiFailure || caught instanceof Error)
    return caught.message;
  return "The request failed.";
}

function isRepositoryView(value: string | null): value is RepositoryView {
  return [
    "overview",
    "history",
    "commit",
    "tags",
    "releases",
    "issues",
    "settings",
  ].includes(value ?? "");
}

export class RepositoryPageState {
  readonly namespace: string;
  readonly name: string;

  repository = $state.raw<Repository | null>(null);
  refs = $state.raw<RepositoryRefs | null>(null);
  repositoryTree = $state.raw<Tree | null>(null);
  expandedTrees = $state.raw<Record<string, Tree>>({});
  expandedPaths = $state.raw<SvelteSet<string>>(new SvelteSet());
  loadingPaths = $state.raw<SvelteSet<string>>(new SvelteSet());
  selectedPath = $state("");
  blob = $state.raw<Blob | null>(null);
  history = $state.raw<History | null>(null);
  commit = $state.raw<Commit | null>(null);
  diff = $state.raw<Diff | null>(null);
  stats = $state.raw<LanguageStat[]>([]);
  commitCount = $state.raw<number | null>(null);
  readme = $state.raw<Blob | null>(null);
  authStatus = $state.raw<AuthStatus | null>(null);
  webhooks = $state.raw<Webhook[]>([]);
  webhookDeliveries = $state<Record<string, WebhookDelivery[]>>({});
  expandedWebhookId = $state<string | null>(null);
  webhookDeliveriesLoadingId = $state<string | null>(null);
  redeliveringDeliveryId = $state<string | null>(null);
  releases = $state.raw<Release[]>([]);
  issues = $state.raw<Issue[]>([]);
  selectedIssue = $state.raw<Issue | null>(null);
  issueComments = $state.raw<IssueComment[]>([]);
  issueLabels = $state.raw<IssueLabel[]>([]);
  assignableUsers = $state.raw<IssueUser[]>([]);
  topics = $state.raw<string[]>([]);
  ownedNamespaces = $state.raw<string[]>([]);
  view = $state<RepositoryView>("overview");
  revision = $state("");
  repositoryPath = $state("");
  commitOid = $state("");
  historyPage = $state(1);
  issueNumber = $state<number | null>(null);
  webhookUrl = $state("");
  webhookSecret = $state("");
  webhookActive = $state(true);
  webhooksLoading = $state(false);
  webhooksLoaded = $state(false);
  releasesLoading = $state(false);
  releasesLoaded = $state(false);
  releasesLoadFailed = $state(false);
  releasePending = $state(false);
  releaseAssetPending = $state(false);
  issuesLoading = $state(false);
  issuesLoaded = $state(false);
  issuePending = $state(false);
  commentPending = $state(false);
  labelPending = $state(false);
  webhookCreating = $state(false);
  webhookUpdatingId = $state<string | null>(null);
  webhookPingingId = $state<string | null>(null);
  webhookDeletingId = $state<string | null>(null);
  repositoryControlPending = $state(false);
  lifecyclePending = $state(false);
  error = $state<string | null>(null);
  notice = $state<string | null>(null);
  copied = $state<CopyTarget | null>(null);
  favoritePending = $state(false);
  wrapLines = $state(false);
  emptyRepository = $state(false);
  loading = $state(true);
  highlighted = $state("");

  selectedLanguage = $derived(this.blob ? languageLabel(this.blob.path) : "");
  totalLines = $derived(
    this.stats.reduce((sum, item) => sum + item.code + item.comments, 0),
  );
  webhookActionPending = $derived(
    this.webhookUpdatingId !== null ||
      this.webhookPingingId !== null ||
      this.webhookDeletingId !== null,
  );

  #repositoryRequestSequence = 0;
  #highlightRequestSequence = 0;
  #supplementaryRefreshTimers: number[] = [];
  #viewRequestController: AbortController | null = null;
  #statsRevision = "";
  #sidebarRevision = "";

  constructor(namespace: string, name: string) {
    this.namespace = namespace;
    this.name = name;
  }

  destroy(): void {
    this.#repositoryRequestSequence += 1;
    this.#viewRequestController?.abort();
    this.#viewRequestController = null;
    this.#supplementaryRefreshTimers.forEach((timer) =>
      window.clearTimeout(timer),
    );
    this.#supplementaryRefreshTimers = [];
  }

  get httpCloneUrl(): string {
    return typeof window === "undefined"
      ? ""
      : `${window.location.origin}/${this.namespace}/${this.name}.git`;
  }

  get rawUrl(): string {
    if (!this.blob) return "";
    const parameters = new URLSearchParams({
      rev: this.revision,
      path: this.blob.path,
    });
    return `${this.#api("/raw")}?${parameters}`;
  }

  async initialize(): Promise<void> {
    const sequence = ++this.#repositoryRequestSequence;
    this.loading = true;
    this.error = null;
    try {
      const { repository, refs, authStatus, organizations, topics } =
        await loadRepositoryBootstrap(this.namespace, this.name);
      if (sequence !== this.#repositoryRequestSequence) return;
      this.repository = repository;
      this.refs = refs;
      this.authStatus = authStatus;
      this.topics = topics.topics;
      this.ownedNamespaces = [
        ...(authStatus.user ? [authStatus.user.username] : []),
        ...organizations
          .filter((organization) => organization.role === "owner")
          .map((organization) => organization.slug),
      ];
      this.#readLocation(repository);
      void this.#loadReleases({}).catch(() => undefined);
      await this.loadView();
    } catch (caught) {
      if (sequence === this.#repositoryRequestSequence) {
        this.error = errorMessage(caught);
      }
    } finally {
      if (sequence === this.#repositoryRequestSequence) this.loading = false;
    }
  }

  async loadView(): Promise<void> {
    this.#viewRequestController?.abort();
    const controller = new AbortController();
    this.#viewRequestController = controller;
    const init = { signal: controller.signal };
    this.error = null;
    this.emptyRepository = false;
    this.repositoryTree = null;
    this.expandedTrees = {};
    this.expandedPaths = new SvelteSet();
    this.loadingPaths = new SvelteSet();
    this.selectedPath = this.repositoryPath;
    this.blob = null;
    this.highlighted = "";
    this.#highlightRequestSequence += 1;
    this.history = null;
    this.commit = null;
    this.diff = null;
    this.readme = null;

    // The sidebar describes the revision rather than the active view, so its
    // figures survive tab changes and are only discarded once they belong to a
    // revision that is no longer on screen.
    if (this.#sidebarRevision !== this.revision) {
      this.#sidebarRevision = this.revision;
      this.stats = [];
      this.commitCount = null;
      this.#statsRevision = "";
    }
    // Only the overview fetches a tree, so every other view has to ask for the
    // statistics on its own.
    if (this.view !== "overview") void this.#loadStats(this.revision);

    try {
      switch (this.view) {
        case "overview":
          await this.#loadOverview(init);
          break;
        case "history":
          this.history = await requestJson(
            `${this.#api("/history")}?${new URLSearchParams({ rev: this.revision, page: String(this.historyPage), per_page: "30" })}`,
            historySchema,
            init,
          );
          break;
        case "commit":
          if (!this.commitOid) throw new Error("No commit was selected.");
          [this.commit, this.diff] = await Promise.all([
            requestJson(
              this.#api(`/commits/${encodeURIComponent(this.commitOid)}`),
              commitSchema,
              init,
            ),
            requestJson(
              this.#api(`/commits/${encodeURIComponent(this.commitOid)}/diff`),
              diffSchema,
              init,
            ),
          ]);
          break;
        case "tags":
          break;
        case "releases":
          await this.#loadReleases(init);
          break;
        case "issues":
          await Promise.all([
            this.loadIssues(init),
            this.loadIssueLabels(init),
            this.issueNumber
              ? this.loadIssue(this.issueNumber, init)
              : Promise.resolve(),
          ]);
          break;
        case "settings":
          await this.#loadWebhooks(init);
          break;
      }
    } catch (caught) {
      if (!(caught instanceof DOMException && caught.name === "AbortError")) {
        this.error = errorMessage(caught);
      }
    } finally {
      if (this.#viewRequestController === controller)
        this.#viewRequestController = null;
      this.#scheduleSupplementaryRefresh();
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
    } = {},
  ): void {
    this.view =
      nextView === "settings" && !this.repository?.can_manage
        ? "overview"
        : nextView;
    this.repositoryPath = options.path ?? "";
    this.commitOid = options.oid ?? "";
    this.historyPage = options.page ?? 1;
    this.issueNumber = nextView === "issues" ? (options.issue ?? null) : null;
    this.revision = options.rev ?? this.revision;
    this.#writeLocation();
    void this.loadView();
  }

  restoreLocation(): void {
    if (!this.repository) return;
    this.#readLocation(this.repository);
    void this.loadView();
  }

  changeRevision(nextRevision: string): void {
    this.navigate(this.view === "commit" ? "overview" : this.view, {
      path: this.repositoryPath,
      rev: nextRevision,
    });
  }

  selectEntry(entry: Tree["entries"][number]): void {
    this.selectedPath = entry.path;
    if (entry.kind === "tree") {
      void this.toggleDirectory(entry.path);
      return;
    }
    this.view = "overview";
    this.repositoryPath = entry.path;
    this.commitOid = "";
    this.historyPage = 1;
    this.#writeLocation();
    void this.#selectBlob(entry.path);
  }

  async toggleDirectory(path: string): Promise<void> {
    const expanded = new SvelteSet(this.expandedPaths);
    if (expanded.has(path)) {
      expanded.delete(path);
      this.expandedPaths = expanded;
      return;
    }
    expanded.add(path);
    this.expandedPaths = expanded;
    if (this.expandedTrees[path] || this.loadingPaths.has(path)) return;

    const loading = new SvelteSet(this.loadingPaths);
    loading.add(path);
    this.loadingPaths = loading;
    const requestedRevision = this.revision;
    try {
      const tree = await requestJson(
        `${this.#api("/tree")}?${this.#query(path)}`,
        treeSchema,
      );
      if (this.revision === requestedRevision) {
        this.expandedTrees = { ...this.expandedTrees, [path]: tree };
      }
    } catch (caught) {
      this.error = errorMessage(caught);
    } finally {
      const nextLoading = new SvelteSet(this.loadingPaths);
      nextLoading.delete(path);
      this.loadingPaths = nextLoading;
    }
  }

  async toggleFavorite(): Promise<void> {
    if (!this.repository) return;
    if (!this.authStatus?.authenticated) {
      const returnTo = encodeURIComponent(`/${this.namespace}/${this.name}`);
      window.location.assign(`/login?returnTo=${returnTo}`);
      return;
    }
    const favorited = !this.repository.favorited;
    this.favoritePending = true;
    this.error = null;
    try {
      await requestEmpty(`${this.#api("/favorite")}`, {
        method: favorited ? "PUT" : "DELETE",
      });
      clearRepositoryPreload(this.namespace, this.name);
      this.repository = { ...this.repository, favorited };
    } catch (caught) {
      this.error = errorMessage(caught);
    } finally {
      this.favoritePending = false;
    }
  }

  async updateRepositoryControl(values: {
    description?: string | null;
    visibility?: "public" | "private";
    default_branch?: string;
    name?: string;
    namespace?: string;
  }): Promise<void> {
    this.repositoryControlPending = true;
    this.error = null;
    this.notice = null;
    try {
      const repository = await requestJson(
        this.#api("/control"),
        repositorySchema,
        {
          method: "PATCH",
          body: jsonBody(values),
        },
      );
      const moved =
        repository.namespace !== this.namespace ||
        repository.name !== this.name;
      clearRepositoryPreload(this.namespace, this.name);
      this.repository = repository;
      if (moved) {
        window.location.assign(
          `/${encodeURIComponent(repository.namespace)}/${encodeURIComponent(repository.name)}?view=settings`,
        );
        return;
      }
      this.notice = "Repository settings saved.";
      if (values.default_branch) {
        this.revision = values.default_branch;
        await this.initialize();
      }
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.repositoryControlPending = false;
    }
  }

  async saveTopics(topics: string[]): Promise<void> {
    this.error = null;
    try {
      const saved = await requestJson(this.#api("/topics"), topicsSchema, {
        method: "PUT",
        body: jsonBody({ topics }),
      });
      clearRepositoryPreload(this.namespace, this.name);
      this.topics = saved.topics;
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    }
  }

  async suggestTopics(query: string, init?: RequestInit): Promise<string[]> {
    const parameters = query ? `?${new URLSearchParams({ q: query })}` : "";
    const { topics } = await requestJson(
      `/api/v1/topics${parameters}`,
      topicsSchema,
      init,
    );
    return topics;
  }

  async setArchived(archived: boolean): Promise<void> {
    await this.#lifecycleRequest(
      "/archive",
      archived ? "POST" : "DELETE",
      archived
        ? "Repository archived. Cloning remains available; pushes are blocked."
        : "Repository unarchived.",
    );
  }

  async softDelete(): Promise<void> {
    await this.#lifecycleRequest(
      "/delete",
      "POST",
      "Repository deleted. You can restore it during the recovery period.",
    );
    window.location.assign("/");
  }

  selectIssue(number: number | null) {
    this.selectedIssue = null;
    this.issueComments = [];
    this.navigate("issues", { issue: number });
  }

  async loadIssues(init: RequestInit = {}) {
    this.issuesLoading = true;
    try {
      this.issues = await requestJson(
        this.#api("/issues"),
        z.array(issueSchema),
        init,
      );
      this.issuesLoaded = true;
    } finally {
      this.issuesLoading = false;
    }
  }

  async loadIssue(number: number, init: RequestInit = {}) {
    const [issue, comments] = await Promise.all([
      requestJson(this.#api(`/issues/${number}`), issueSchema, init),
      requestJson(
        this.#api(`/issues/${number}/comments`),
        z.array(issueCommentSchema),
        init,
      ),
    ]);
    if (this.issueNumber === number) {
      this.selectedIssue = issue;
      this.issueComments = comments;
    }
    return issue;
  }

  async loadIssueLabels(init: RequestInit = {}) {
    this.issueLabels = await requestJson(
      this.#api("/issue-labels"),
      z.array(issueLabelSchema),
      init,
    );
  }

  async loadAssignableUsers(init: RequestInit = {}) {
    this.assignableUsers = await requestJson(
      this.#api("/assignable-users"),
      z.array(issueUserSchema),
      init,
    );
  }

  async previewMarkdown(markdown: string) {
    const { rendered_html } = await requestJson(
      `${this.#api("/markdown-preview")}`,
      renderedMarkdownSchema,
      { method: "POST", body: jsonBody({ markdown }) },
    );
    return rendered_html;
  }

  async uploadIssueAttachment(file: File): Promise<IssueAttachment> {
    return requestJson(
      `${this.#api("/issue-attachments")}?${new URLSearchParams({ name: file.name })}`,
      issueAttachmentSchema,
      {
        method: "PUT",
        headers: { "content-type": file.type || "application/octet-stream" },
        body: file,
      },
    );
  }

  async createIssue(values: {
    title: string;
    body: string;
    label_ids: string[];
    assignee?: string;
    attachment_ids?: string[];
  }) {
    this.issuePending = true;
    this.error = null;
    try {
      const issue = await requestJson(this.#api("/issues"), issueSchema, {
        method: "POST",
        body: jsonBody(values),
      });
      this.notice = `Issue #${issue.number} opened.`;
      await this.loadIssues();
      this.selectIssue(issue.number);
      return issue;
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.issuePending = false;
    }
  }

  async updateIssue(
    number: number,
    values: {
      title?: string;
      body?: string;
      state?: "open" | "closed";
      assignee?: string;
      label_ids?: string[];
      attachment_ids?: string[];
    },
  ) {
    this.issuePending = true;
    this.error = null;
    try {
      const issue = await requestJson(
        this.#api(`/issues/${number}`),
        issueSchema,
        { method: "PATCH", body: jsonBody(values) },
      );
      this.selectedIssue = issue;
      this.issues = this.issues.map((item) =>
        item.number === issue.number ? issue : item,
      );
      this.notice = `Issue #${number} updated.`;
      return issue;
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.issuePending = false;
    }
  }

  async deleteIssue(number: number) {
    this.issuePending = true;
    this.error = null;
    try {
      await requestEmpty(this.#api(`/issues/${number}`), { method: "DELETE" });
      this.notice = `Issue #${number} deleted.`;
      this.issueNumber = null;
      this.selectedIssue = null;
      this.issueComments = [];
      await this.loadIssues();
      this.#writeLocation();
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.issuePending = false;
    }
  }

  async createIssueComment(number: number, body: string) {
    this.commentPending = true;
    this.error = null;
    try {
      const comment = await requestJson(
        this.#api(`/issues/${number}/comments`),
        issueCommentSchema,
        { method: "POST", body: jsonBody({ body }) },
      );
      this.issueComments = [...this.issueComments, comment];
      if (this.selectedIssue?.number === number) {
        this.selectedIssue = {
          ...this.selectedIssue,
          comment_count: this.selectedIssue.comment_count + 1,
        };
      }
      await this.loadIssues();
      return comment;
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.commentPending = false;
    }
  }

  async updateIssueComment(number: number, id: string, body: string) {
    this.commentPending = true;
    this.error = null;
    try {
      const comment = await requestJson(
        this.#api(`/issues/${number}/comments/${id}`),
        issueCommentSchema,
        { method: "PATCH", body: jsonBody({ body }) },
      );
      this.issueComments = this.issueComments.map((item) =>
        item.id === id ? comment : item,
      );
      return comment;
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.commentPending = false;
    }
  }

  async deleteIssueComment(number: number, id: string) {
    this.commentPending = true;
    this.error = null;
    try {
      await requestEmpty(this.#api(`/issues/${number}/comments/${id}`), {
        method: "DELETE",
      });
      this.issueComments = this.issueComments.filter((item) => item.id !== id);
      if (this.selectedIssue?.number === number) {
        this.selectedIssue = {
          ...this.selectedIssue,
          comment_count: Math.max(0, this.selectedIssue.comment_count - 1),
        };
      }
      await this.loadIssues();
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.commentPending = false;
    }
  }

  async createIssueLabel(values: {
    name: string;
    color: string;
    description: string;
  }) {
    this.labelPending = true;
    this.error = null;
    try {
      const label = await requestJson(
        this.#api("/issue-labels"),
        issueLabelSchema,
        { method: "POST", body: jsonBody(values) },
      );
      this.issueLabels = [...this.issueLabels, label].sort((left, right) =>
        left.name.localeCompare(right.name),
      );
      return label;
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.labelPending = false;
    }
  }

  async deleteIssueLabel(id: string) {
    this.labelPending = true;
    this.error = null;
    try {
      await requestEmpty(this.#api(`/issue-labels/${id}`), {
        method: "DELETE",
      });
      this.issueLabels = this.issueLabels.filter((label) => label.id !== id);
      await this.loadIssues();
      if (this.issueNumber) await this.loadIssue(this.issueNumber);
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.labelPending = false;
    }
  }

  async createRelease(
    values: {
      target_revision: string;
      title: string;
      body: string;
      prerelease: boolean;
    },
    files: File[],
  ) {
    this.releasePending = true;
    this.error = null;
    this.notice = null;
    try {
      let release = await requestJson(this.#api("/releases"), releaseSchema, {
        method: "POST",
        body: jsonBody(values),
      });
      this.releases = [release, ...this.releases];
      this.releasesLoaded = true;
      if (files.length) {
        this.releaseAssetPending = true;
        try {
          for (const file of files) {
            const asset = await requestJson(
              `${this.#api(`/releases/${release.id}/assets`)}?${new URLSearchParams({ name: file.name })}`,
              releaseAssetSchema,
              {
                method: "PUT",
                headers: {
                  "content-type": file.type || "application/octet-stream",
                },
                body: file,
              },
            );
            release = { ...release, assets: [...release.assets, asset] };
          }
        } catch (caught) {
          await this.#loadReleases({}).catch(() => undefined);
          this.error = `Release “${release.title}” was published, but some assets could not be uploaded: ${errorMessage(caught)} Add the remaining files from the published release.`;
          return release;
        }
      }
      await this.#loadReleases({});
      this.notice = `Release “${release.title}” published.`;
      return release;
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.releasePending = false;
      this.releaseAssetPending = false;
    }
  }

  async updateRelease(
    id: string,
    values: {
      target_revision: string;
      title: string;
      body: string;
      prerelease: boolean;
    },
  ) {
    this.releasePending = true;
    this.error = null;
    this.notice = null;
    try {
      const release = await requestJson(
        this.#api(`/releases/${id}`),
        releaseSchema,
        { method: "PATCH", body: jsonBody(values) },
      );
      await this.#loadReleases({});
      this.notice = "Release updated.";
      return release;
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.releasePending = false;
    }
  }

  async uploadReleaseAssets(releaseId: string, files: File[]) {
    this.releaseAssetPending = true;
    this.error = null;
    try {
      for (const file of files) {
        await requestJson(
          `${this.#api(`/releases/${releaseId}/assets`)}?${new URLSearchParams({ name: file.name })}`,
          releaseAssetSchema,
          {
            method: "PUT",
            headers: {
              "content-type": file.type || "application/octet-stream",
            },
            body: file,
          },
        );
      }
      await this.#loadReleases({});
      this.notice = `${files.length} release asset${files.length === 1 ? "" : "s"} uploaded.`;
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.releaseAssetPending = false;
    }
  }

  async deleteRelease(id: string) {
    this.releasePending = true;
    this.error = null;
    try {
      await requestEmpty(this.#api(`/releases/${id}`), { method: "DELETE" });
      await this.#loadReleases({});
      this.notice = "Release deleted. Its Git target was not changed.";
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.releasePending = false;
    }
  }

  async deleteReleaseAsset(releaseId: string, assetId: string) {
    this.releaseAssetPending = true;
    this.error = null;
    try {
      await requestEmpty(
        this.#api(`/releases/${releaseId}/assets/${assetId}`),
        { method: "DELETE" },
      );
      await this.#loadReleases({});
      this.notice = "Release asset deleted.";
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.releaseAssetPending = false;
    }
  }

  async createWebhook(): Promise<void> {
    this.webhookCreating = true;
    this.error = null;
    this.notice = null;
    try {
      const hook = await requestJson(this.#hooksApi(), webhookSchema, {
        method: "POST",
        body: jsonBody({
          name: "web",
          active: this.webhookActive,
          events: ["push"],
          config: {
            url: this.webhookUrl,
            content_type: "json",
            ...(this.webhookSecret && { secret: this.webhookSecret }),
          },
        }),
      });
      this.webhooks = [...this.webhooks, hook];
      this.webhooksLoaded = true;
      this.webhookUrl = "";
      this.webhookSecret = "";
      this.webhookActive = true;
      this.notice = "Webhook created. A ping delivery has been queued.";
      window.setTimeout(() => void this.#refreshWebhooks(), 1500);
    } catch (caught) {
      this.error = errorMessage(caught);
    } finally {
      this.webhookCreating = false;
    }
  }

  async updateWebhook(hook: Webhook, url: string, secret: string) {
    this.webhookUpdatingId = hook.id;
    this.error = null;
    this.notice = null;
    try {
      const updated = await requestJson(
        `${this.#hooksApi()}/${hook.id}`,
        webhookSchema,
        {
          method: "PATCH",
          body: jsonBody({
            config: {
              url,
              content_type: "json",
              ...(secret && { secret }),
            },
          }),
        },
      );
      this.webhooks = this.webhooks.map((item) =>
        item.id === updated.id ? updated : item,
      );
      this.notice = "Webhook updated. Send a ping to verify the endpoint.";
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.webhookUpdatingId = null;
    }
  }

  async setWebhookActive(hook: Webhook, active: boolean): Promise<void> {
    this.webhookUpdatingId = hook.id;
    this.error = null;
    this.notice = null;
    try {
      const updated = await requestJson(
        `${this.#hooksApi()}/${hook.id}`,
        webhookSchema,
        {
          method: "PATCH",
          body: jsonBody({ active }),
        },
      );
      this.webhooks = this.webhooks.map((item) =>
        item.id === updated.id ? updated : item,
      );
      this.notice = active ? "Webhook enabled." : "Webhook disabled.";
    } catch (caught) {
      this.error = errorMessage(caught);
    } finally {
      this.webhookUpdatingId = null;
    }
  }

  async pingWebhook(id: string): Promise<void> {
    this.webhookPingingId = id;
    this.error = null;
    this.notice = null;
    try {
      await requestEmpty(`${this.#hooksApi()}/${id}/pings`, {
        method: "POST",
      });
      this.notice = "Ping delivery queued.";
      window.setTimeout(() => void this.#refreshWebhookActivity(id), 1500);
    } catch (caught) {
      this.error = errorMessage(caught);
    } finally {
      this.webhookPingingId = null;
    }
  }

  async toggleWebhookDeliveries(hookId: string): Promise<void> {
    if (this.expandedWebhookId === hookId) {
      this.expandedWebhookId = null;
      return;
    }
    this.expandedWebhookId = hookId;
    if (!this.webhookDeliveries[hookId]) {
      await this.#loadWebhookDeliveries(hookId);
    }
  }

  async redeliverWebhookDelivery(
    hookId: string,
    deliveryId: string,
  ): Promise<void> {
    this.redeliveringDeliveryId = deliveryId;
    this.error = null;
    this.notice = null;
    try {
      await requestEmpty(
        `${this.#hooksApi()}/${hookId}/deliveries/${deliveryId}/attempts`,
        { method: "POST" },
      );
      this.notice = "Redelivery queued.";
      window.setTimeout(() => void this.#refreshWebhookActivity(hookId), 1500);
    } catch (caught) {
      this.error = errorMessage(caught);
    } finally {
      this.redeliveringDeliveryId = null;
    }
  }

  async deleteWebhook(id: string): Promise<void> {
    this.webhookDeletingId = id;
    this.error = null;
    this.notice = null;
    try {
      await requestEmpty(`${this.#hooksApi()}/${id}`, { method: "DELETE" });
      this.webhooks = this.webhooks.filter((hook) => hook.id !== id);
      if (this.expandedWebhookId === id) {
        this.expandedWebhookId = null;
      }
      const { [id]: removed, ...deliveries } = this.webhookDeliveries;
      this.webhookDeliveries = deliveries;
      this.notice = "Webhook deleted.";
    } catch (caught) {
      this.error = errorMessage(caught);
    } finally {
      this.webhookDeletingId = null;
    }
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
      this.error = "The clone URL could not be copied.";
      toast.error(this.error, {
        description: "Select the URL and copy it manually instead.",
      });
    }
  }

  async #lifecycleRequest(
    path: string,
    method: string,
    notice: string,
  ): Promise<void> {
    this.lifecyclePending = true;
    this.error = null;
    this.notice = null;
    try {
      await requestEmpty(this.#api(path), { method });
      clearRepositoryPreload(this.namespace, this.name);
      if (this.repository && path === "/archive") {
        this.repository = {
          ...this.repository,
          archived_at:
            method === "POST" ? new SvelteDate().toISOString() : null,
        };
      }
      this.notice = notice;
    } catch (caught) {
      this.error = errorMessage(caught);
      throw caught;
    } finally {
      this.lifecyclePending = false;
    }
  }

  async refreshReleases() {
    try {
      await this.#loadReleases({});
    } catch (caught) {
      this.error = errorMessage(caught);
    }
  }

  async #loadReleases(init: RequestInit) {
    if (this.releasesLoading || (this.releasesLoaded && init.signal)) return;
    this.releasesLoading = true;
    this.releasesLoadFailed = false;
    try {
      this.releases = await requestJson(
        this.#api("/releases"),
        z.array(releaseSchema),
        init,
      );
      this.releasesLoaded = true;
    } catch (caught) {
      this.releasesLoadFailed = true;
      throw caught;
    } finally {
      this.releasesLoading = false;
    }
  }

  async #loadWebhooks(init: RequestInit): Promise<void> {
    if (!this.repository?.can_manage) return;
    this.webhooksLoading = true;
    try {
      this.webhooks = await requestJson(
        this.#hooksApi(),
        z.array(webhookSchema),
        init,
      );
      this.webhooksLoaded = true;
    } finally {
      this.webhooksLoading = false;
    }
  }

  async #refreshWebhooks(): Promise<void> {
    try {
      await this.#loadWebhooks({});
    } catch (caught) {
      this.error = errorMessage(caught);
    }
  }

  async #loadWebhookDeliveries(hookId: string): Promise<void> {
    this.webhookDeliveriesLoadingId = hookId;
    try {
      const deliveries = await requestJson(
        `${this.#hooksApi()}/${hookId}/deliveries`,
        z.array(webhookDeliverySchema),
        {},
      );
      this.webhookDeliveries = {
        ...this.webhookDeliveries,
        [hookId]: deliveries,
      };
    } finally {
      this.webhookDeliveriesLoadingId = null;
    }
  }

  async #refreshWebhookActivity(hookId: string): Promise<void> {
    void this.#refreshWebhooks();
    if (this.expandedWebhookId === hookId) {
      try {
        await this.#loadWebhookDeliveries(hookId);
      } catch (caught) {
        this.error = errorMessage(caught);
      }
    }
  }

  async #setBlob(blob: Blob) {
    this.blob = blob;
    const source = blob.content;
    const sequence = ++this.#highlightRequestSequence;
    this.highlighted = source ? escapeHtml(source) : "";
    if (!source || source.length > MAX_HIGHLIGHT_CHARACTERS) return;

    try {
      const { highlight } = await import("$lib/repository/syntax-highlight.js");
      if (
        sequence === this.#highlightRequestSequence &&
        this.blob?.oid === blob.oid
      ) {
        this.highlighted = highlight(blob.path, source);
      }
    } catch {
      // The escaped plaintext is already visible if highlighting cannot load.
    }
  }

  async #selectBlob(path: string): Promise<void> {
    this.#viewRequestController?.abort();
    const controller = new AbortController();
    this.#viewRequestController = controller;
    this.error = null;
    try {
      const blob = await requestJson(
        `${this.#api("/blob")}?${this.#query(path)}`,
        blobSchema,
        { signal: controller.signal },
      );
      if (
        this.#viewRequestController === controller &&
        this.repositoryPath === path
      ) {
        void this.#setBlob(blob);
        this.readme = null;
      }
    } catch (caught) {
      if (!(caught instanceof DOMException && caught.name === "AbortError")) {
        this.error = errorMessage(caught);
      }
    } finally {
      if (this.#viewRequestController === controller) {
        this.#viewRequestController = null;
      }
    }
  }

  #scheduleSupplementaryRefresh() {
    this.#supplementaryRefreshTimers.forEach((timer) =>
      window.clearTimeout(timer),
    );
    this.#supplementaryRefreshTimers = [];
    if (this.refs?.size_bytes != null && this.commitCount != null) return;

    const sequence = this.#repositoryRequestSequence;
    const revision = this.revision;
    for (const delay of [1_000, 5_000]) {
      const timer = window.setTimeout(async () => {
        if (
          sequence !== this.#repositoryRequestSequence ||
          revision !== this.revision
        ) {
          return;
        }
        const refsRequest =
          this.refs?.size_bytes == null
            ? requestJson(this.#api("/refs"), refsSchema)
            : Promise.resolve(null);
        const countRequest =
          this.commitCount == null
            ? requestJson(
                `${this.#api("/tree")}?${new URLSearchParams({ rev: revision })}`,
                treeSchema,
              )
            : Promise.resolve(null);
        const [refs, tree] = await Promise.all([
          refsRequest.catch(() => null),
          countRequest.catch(() => null),
        ]);
        if (
          sequence !== this.#repositoryRequestSequence ||
          revision !== this.revision
        ) {
          return;
        }
        if (refs) this.refs = refs;
        if (tree?.commit_count != null) this.commitCount = tree.commit_count;
      }, delay);
      this.#supplementaryRefreshTimers.push(timer);
    }
  }

  /**
   * Fetches language statistics for a revision at most once. Stats outlive view
   * changes, so this is a no-op whenever the sidebar already shows the numbers
   * for the revision being asked about.
   */
  async #loadStats(revision: string): Promise<void> {
    if (this.#statsRevision === revision) return;
    this.#statsRevision = revision;
    try {
      const stats = await requestJson(
        `${this.#api("/stats")}?${new URLSearchParams({ rev: revision })}`,
        z.array(languageStatSchema),
      );
      if (this.revision === revision) this.stats = stats;
    } catch {
      // Statistics are supplementary, so a failure leaves the previous numbers
      // in place and only clears the guard so a later view can retry.
      if (this.#statsRevision === revision) this.#statsRevision = "";
    }
  }

  async #loadOverview(init: RequestInit): Promise<void> {
    try {
      const preloaded = this.repositoryPath
        ? null
        : takePreloadedRepositoryOverview(
            this.namespace,
            this.name,
            this.revision,
          );
      if (preloaded) {
        const overview = await preloaded;
        if (init.signal?.aborted) return;
        this.repositoryTree = overview.tree;
        this.commitCount = overview.tree?.commit_count ?? null;
        this.stats = overview.stats;
        this.#statsRevision = this.revision;
        this.readme = overview.readme;
        this.emptyRepository = overview.emptyRepository;
        return;
      }

      const [tree] = await Promise.all([
        requestJson(`${this.#api("/tree")}?${this.#query()}`, treeSchema, init),
        this.#loadStats(this.revision),
      ]);
      this.repositoryTree = tree;
      this.commitCount = tree.commit_count;

      if (this.repositoryPath) {
        const parts = this.repositoryPath.split("/");
        const parentPaths = parts
          .slice(0, -1)
          .map((_, index) => parts.slice(0, index + 1).join("/"));
        const [blob, parentTrees] = await Promise.all([
          requestJson(
            `${this.#api("/blob")}?${this.#query(this.repositoryPath)}`,
            blobSchema,
            init,
          ),
          Promise.all(
            parentPaths.map((path) =>
              requestJson(
                `${this.#api("/tree")}?${this.#query(path)}`,
                treeSchema,
                init,
              ),
            ),
          ),
        ]);
        void this.#setBlob(blob);
        this.expandedPaths = new SvelteSet(parentPaths);
        this.expandedTrees = Object.fromEntries(
          parentPaths.map((path, index) => [path, parentTrees[index]!]),
        );
        return;
      }

      const readmeEntry = tree.entries.find(
        (entry) =>
          entry.kind === "blob" && /^readme(?:\.[^.]+)?$/i.test(entry.name),
      );
      if (readmeEntry) {
        this.readme = await requestJson(
          `${this.#api("/blob")}?${this.#query(readmeEntry.path)}`,
          blobSchema,
          init,
        );
      }
    } catch (caught) {
      if (caught instanceof ApiFailure && caught.status === 404) {
        this.emptyRepository = true;
        return;
      }
      throw caught;
    }
  }

  #readLocation(repository: Repository): void {
    const parameters = new URLSearchParams(window.location.search);
    const requestedView = parameters.get("view");
    const view = isRepositoryView(requestedView) ? requestedView : "overview";
    this.view =
      view === "settings" && !repository.can_manage ? "overview" : view;
    this.revision = parameters.get("rev") || repository.default_branch;
    this.repositoryPath = parameters.get("path") || "";
    this.commitOid = parameters.get("oid") || "";
    this.historyPage = Math.max(1, Number(parameters.get("page")) || 1);
    const issueNumber = Number(parameters.get("issue"));
    this.issueNumber =
      view === "issues" && issueNumber > 0 ? issueNumber : null;
  }

  #writeLocation(): void {
    const parameters = new URLSearchParams();
    if (this.view !== "overview") parameters.set("view", this.view);
    if (this.revision && this.revision !== this.repository?.default_branch) {
      parameters.set("rev", this.revision);
    }
    if (this.repositoryPath) parameters.set("path", this.repositoryPath);
    if (this.commitOid) parameters.set("oid", this.commitOid);
    if (this.historyPage > 1) parameters.set("page", String(this.historyPage));
    if (this.view === "issues" && this.issueNumber) {
      parameters.set("issue", String(this.issueNumber));
    }
    const search = parameters.toString();
    window.history.pushState(
      {},
      "",
      `/${this.namespace}/${this.name}${search ? `?${search}` : ""}`,
    );
  }

  #api(path = ""): string {
    return `/api/v1/repositories/${encodeURIComponent(this.namespace)}/${encodeURIComponent(this.name)}${path}`;
  }

  #hooksApi(): string {
    return `/api/v1/repos/${encodeURIComponent(this.namespace)}/${encodeURIComponent(this.name)}/hooks`;
  }

  #query(path = ""): string {
    const parameters = new URLSearchParams({ rev: this.revision });
    if (path) parameters.set("path", path);
    return parameters.toString();
  }
}
