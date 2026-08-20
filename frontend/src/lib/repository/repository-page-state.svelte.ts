import { z } from "zod";

import {
  ApiFailure,
  authStatusSchema,
  blobSchema,
  commitSchema,
  diffSchema,
  historySchema,
  languageStatSchema,
  refsSchema,
  repositorySchema,
  requestEmpty,
  requestJson,
  treeSchema,
  type AuthStatus,
  type Blob,
  type Commit,
  type Diff,
  type History,
  type LanguageStat,
  type Repository,
  type RepositoryRefs,
  type Tree,
} from "$lib/api.js";
import { highlight } from "$lib/repository/format.js";

export type RepositoryView = "overview" | "history" | "commit" | "tags";
export type CopyTarget = "http" | "ssh";

function errorMessage(caught: unknown): string {
  if (caught instanceof ApiFailure || caught instanceof Error) return caught.message;
  return "The request failed.";
}

function isRepositoryView(value: string | null): value is RepositoryView {
  return ["overview", "history", "commit", "tags"].includes(value ?? "");
}

export class RepositoryPageState {
  readonly namespace: string;
  readonly name: string;

  repository = $state.raw<Repository | null>(null);
  refs = $state.raw<RepositoryRefs | null>(null);
  repositoryTree = $state.raw<Tree | null>(null);
  expandedTrees = $state.raw<Record<string, Tree>>({});
  expandedPaths = $state.raw<Set<string>>(new Set());
  loadingPaths = $state.raw<Set<string>>(new Set());
  selectedPath = $state("");
  blob = $state.raw<Blob | null>(null);
  history = $state.raw<History | null>(null);
  commit = $state.raw<Commit | null>(null);
  diff = $state.raw<Diff | null>(null);
  stats = $state.raw<LanguageStat[]>([]);
  readme = $state.raw<Blob | null>(null);
  authStatus = $state.raw<AuthStatus | null>(null);
  view = $state<RepositoryView>("overview");
  revision = $state("");
  repositoryPath = $state("");
  commitOid = $state("");
  historyPage = $state(1);
  error = $state<string | null>(null);
  copied = $state<CopyTarget | null>(null);
  favoritePending = $state(false);
  wrapLines = $state(false);
  emptyRepository = $state(false);
  loading = $state(true);

  totalCode = $derived(this.stats.reduce((sum, item) => sum + item.code, 0));
  highlighted = $derived(
    this.blob?.content ? highlight(this.blob.path, this.blob.content) : "",
  );

  #repositoryRequestSequence = 0;
  #viewRequestController: AbortController | null = null;

  constructor(namespace: string, name: string) {
    this.namespace = namespace;
    this.name = name;
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
      const [repository, refs, authStatus] = await Promise.all([
        requestJson(
          `/api/v1/repositories/${encodeURIComponent(this.namespace)}/${encodeURIComponent(this.name)}`,
          repositorySchema,
        ),
        requestJson(
          `/api/v1/repositories/${encodeURIComponent(this.namespace)}/${encodeURIComponent(this.name)}/refs`,
          refsSchema,
        ),
        requestJson("/api/v1/auth/status", authStatusSchema),
      ]);
      if (sequence !== this.#repositoryRequestSequence) return;
      this.repository = repository;
      this.refs = refs;
      this.authStatus = authStatus;
      this.#readLocation(repository.default_branch);
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
    this.expandedPaths = new Set();
    this.loadingPaths = new Set();
    this.selectedPath = this.repositoryPath;
    this.blob = null;
    this.history = null;
    this.commit = null;
    this.diff = null;
    this.readme = null;
    this.stats = [];

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
      }
    } catch (caught) {
      if (!(caught instanceof DOMException && caught.name === "AbortError")) {
        this.error = errorMessage(caught);
      }
    } finally {
      if (this.#viewRequestController === controller) this.#viewRequestController = null;
    }
  }

  navigate(
    nextView: RepositoryView,
    options: { path?: string; oid?: string; page?: number; rev?: string } = {},
  ): void {
    this.view = nextView;
    this.repositoryPath = options.path ?? "";
    this.commitOid = options.oid ?? "";
    this.historyPage = options.page ?? 1;
    this.revision = options.rev ?? this.revision;
    this.#writeLocation();
    void this.loadView();
  }

  restoreLocation(): void {
    if (!this.repository) return;
    this.#readLocation(this.repository.default_branch);
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
    this.navigate("overview", { path: entry.path });
  }

  async toggleDirectory(path: string): Promise<void> {
    const expanded = new Set(this.expandedPaths);
    if (expanded.has(path)) {
      expanded.delete(path);
      this.expandedPaths = expanded;
      return;
    }
    expanded.add(path);
    this.expandedPaths = expanded;
    if (this.expandedTrees[path] || this.loadingPaths.has(path)) return;

    const loading = new Set(this.loadingPaths);
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
      const nextLoading = new Set(this.loadingPaths);
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
      this.repository = { ...this.repository, favorited };
    } catch (caught) {
      this.error = errorMessage(caught);
    } finally {
      this.favoritePending = false;
    }
  }

  async copyCloneUrl(target: CopyTarget): Promise<void> {
    const value = target === "http" ? this.httpCloneUrl : this.repository?.ssh_clone_url;
    if (!value) return;
    await navigator.clipboard.writeText(value);
    this.copied = target;
    window.setTimeout(() => {
      if (this.copied === target) this.copied = null;
    }, 1600);
  }

  async #loadOverview(init: RequestInit): Promise<void> {
    try {
      const [tree, stats] = await Promise.all([
        requestJson(`${this.#api("/tree")}?${this.#query()}`, treeSchema, init),
        requestJson(
          `${this.#api("/stats")}?${this.#query()}`,
          z.array(languageStatSchema),
          init,
        ),
      ]);
      this.repositoryTree = tree;
      this.stats = stats;

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
        this.blob = blob;
        this.expandedPaths = new Set(parentPaths);
        this.expandedTrees = Object.fromEntries(
          parentPaths.map((path, index) => [path, parentTrees[index]!]),
        );
        return;
      }

      const readmeEntry = tree.entries.find(
        (entry) => entry.kind === "blob" && /^readme(?:\.[^.]+)?$/i.test(entry.name),
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

  #readLocation(defaultBranch: string): void {
    const parameters = new URLSearchParams(window.location.search);
    const requestedView = parameters.get("view");
    this.view = isRepositoryView(requestedView) ? requestedView : "overview";
    this.revision = parameters.get("rev") || defaultBranch;
    this.repositoryPath = parameters.get("path") || "";
    this.commitOid = parameters.get("oid") || "";
    this.historyPage = Math.max(1, Number(parameters.get("page")) || 1);
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

  #query(path = ""): string {
    const parameters = new URLSearchParams({ rev: this.revision });
    if (path) parameters.set("path", path);
    return parameters.toString();
  }
}
