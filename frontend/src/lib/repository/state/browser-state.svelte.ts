import { SvelteSet } from "svelte/reactivity";
import { z } from "zod";
import {
  blobSchema,
  commitSchema,
  diffSchema,
  languageStatSchema,
  type Blob,
  type Commit,
  type Diff,
  type History,
  type LanguageStat,
  type RepositoryRefs,
  type Tree,
  treeSchema,
} from "$lib/api/repositories.js";
import { requestJson } from "$lib/api/transport.js";
import { loadRepositoryHistory } from "$lib/repository/repository-data-cache.js";
import { takePreloadedRepositoryOverview } from "$lib/repository/repository-preload.js";
import { escapeHtml, languageLabel } from "$lib/repository/format.js";
import { highlight } from "$lib/repository/syntax-highlight.js";
import type { RepositoryFeatureContext } from "./shared.js";
import { repositoryApi, errorMessage } from "./shared.js";

type BrowserCallbacks = {
  setError(message: string): void;
  loadActionStatuses(oids: string[], init?: RequestInit): void;
};
const MAX_HIGHLIGHT_CHARACTERS = 200_000;

export class RepositoryBrowserState {
  readonly namespace: string;
  readonly name: string;
  readonly scope: RepositoryFeatureContext["scope"];
  readonly isScopeCurrent: RepositoryFeatureContext["isScopeCurrent"];
  readonly setError: BrowserCallbacks["setError"];
  readonly loadActionStatuses: BrowserCallbacks["loadActionStatuses"];
  refs = $state.raw<RepositoryRefs | null>(null);
  repositoryTree = $state.raw<Tree | null>(null);
  expandedTrees = $state.raw<Record<string, Tree>>({});
  expandedPaths = $state.raw<SvelteSet<string>>(new SvelteSet());
  loadingPaths = $state.raw<SvelteSet<string>>(new SvelteSet());
  blob = $state.raw<Blob | null>(null);
  submodule = $state.raw<Tree["entries"][number] | null>(null);
  history = $state.raw<History | null>(null);
  commit = $state.raw<Commit | null>(null);
  diff = $state.raw<Diff | null>(null);
  stats = $state.raw<LanguageStat[]>([]);
  commitCount = $state.raw<number | null>(null);
  readme = $state.raw<Blob | null>(null);
  highlighted = $state("");
  selectedPath = $state("");
  selectedLanguage = $derived(this.blob ? languageLabel(this.blob.path) : "");
  wrapLines = $state(false);
  totalLines = $derived(
    this.stats.reduce((sum, item) => sum + item.code + item.comments, 0),
  );
  #highlightSequence = 0;
  #statsRevision = "";
  #sidebarRevision = "";
  #blobController: AbortController | null = null;
  #destroyed = false;
  constructor(context: RepositoryFeatureContext, callbacks: BrowserCallbacks) {
    this.namespace = context.locator.namespace;
    this.name = context.locator.name;
    this.scope = context.scope;
    this.isScopeCurrent = context.isScopeCurrent;
    this.setError = callbacks.setError;
    this.loadActionStatuses = callbacks.loadActionStatuses;
  }
  destroy(): void {
    this.#destroyed = true;
    this.#highlightSequence += 1;
    this.#blobController?.abort();
    this.#blobController = null;
  }
  resetView(revision: string): void {
    this.#blobController?.abort();
    this.#blobController = null;
    this.repositoryTree = null;
    this.expandedTrees = {};
    this.expandedPaths = new SvelteSet();
    this.loadingPaths = new SvelteSet();
    this.selectedPath = "";
    this.blob = null;
    this.submodule = null;
    this.highlighted = "";
    this.#highlightSequence += 1;
    this.history = null;
    this.commit = null;
    this.diff = null;
    this.readme = null;
    if (this.#sidebarRevision !== revision) {
      this.#sidebarRevision = revision;
      this.stats = [];
      this.commitCount = null;
      this.#statsRevision = "";
    }
  }
  async loadOverview(
    revision: string,
    repositoryPath: string,
    init: RequestInit,
  ): Promise<boolean> {
    const preloaded = repositoryPath
      ? null
      : takePreloadedRepositoryOverview(
          this.namespace,
          this.name,
          this.scope,
          revision,
        );
    try {
      if (preloaded) {
        const overview = await preloaded;
        if (init.signal?.aborted || this.#destroyed || !this.isScopeCurrent())
          return false;
        this.repositoryTree = overview.tree;
        this.commitCount = overview.tree?.commit_count ?? null;
        this.stats = overview.stats;
        this.#statsRevision = revision;
        this.readme = overview.readme;
        return overview.emptyRepository;
      }
      const [tree] = await Promise.all([
        requestJson(
          `${repositoryApi(this, "/tree")}?${new URLSearchParams({ rev: revision })}`,
          treeSchema,
          init,
        ),
        this.loadStats(revision),
      ]);
      if (init.signal?.aborted || this.#destroyed || !this.isScopeCurrent())
        return false;
      this.repositoryTree = tree;
      this.commitCount = tree.commit_count;
      if (repositoryPath) {
        const parts = repositoryPath.split("/");
        const parentPaths = parts
          .slice(0, -1)
          .map((_, index) => parts.slice(0, index + 1).join("/"));
        const parentTrees = await Promise.all(
          parentPaths.map((path) =>
            requestJson(
              `${repositoryApi(this, "/tree")}?${this.query(revision, path)}`,
              treeSchema,
              init,
            ),
          ),
        );
        if (init.signal?.aborted || this.#destroyed || !this.isScopeCurrent())
          return false;
        const parent = parentTrees.at(-1) ?? tree;
        const entry = parent.entries.find(
          (entry) => entry.path === repositoryPath,
        );
        if (entry?.kind === "submodule") {
          this.selectSubmodule(entry);
        } else {
          const blob = await requestJson(
            `${repositoryApi(this, "/blob")}?${this.query(revision, repositoryPath)}`,
            blobSchema,
            init,
          );
          if (init.signal?.aborted || this.#destroyed || !this.isScopeCurrent())
            return false;
          void this.setBlob(blob);
        }
        this.expandedPaths = new SvelteSet(parentPaths);
        this.expandedTrees = Object.fromEntries(
          parentPaths.map((path, index) => [path, parentTrees[index]!]),
        );
        return false;
      }
      const readmeEntry = tree.entries.find(
        (entry) =>
          entry.kind === "blob" && /^readme(?:\.[^.]+)?$/i.test(entry.name),
      );
      if (readmeEntry) {
        const readme = await requestJson(
          `${repositoryApi(this, "/blob")}?${this.query(revision, readmeEntry.path)}`,
          blobSchema,
          init,
        );
        if (this.#destroyed || !this.isScopeCurrent()) return false;
        this.readme = readme;
      }
      return false;
    } catch (caught) {
      if (caught instanceof DOMException && caught.name === "AbortError")
        return false;
      throw caught;
    }
  }
  async loadHistory(
    revision: string,
    page: number,
    init: RequestInit,
  ): Promise<void> {
    const history = await loadRepositoryHistory(
      this.namespace,
      this.name,
      revision,
      this.scope,
      page,
    );
    if (init.signal?.aborted || this.#destroyed || !this.isScopeCurrent())
      return;
    this.history = history;
    this.loadActionStatuses(
      history.commits.map((commit) => commit.oid),
      init,
    );
  }
  async loadCommit(oid: string, init: RequestInit): Promise<void> {
    const [commit, diff] = await Promise.all([
      requestJson(
        repositoryApi(this, `/commits/${encodeURIComponent(oid)}`),
        commitSchema,
        init,
      ),
      requestJson(
        repositoryApi(this, `/commits/${encodeURIComponent(oid)}/diff`),
        diffSchema,
        init,
      ),
    ]);
    if (init.signal?.aborted || this.#destroyed || !this.isScopeCurrent())
      return;
    this.commit = commit;
    this.diff = diff;
    this.loadActionStatuses([oid], init);
  }
  async toggleDirectory(path: string, revision: string): Promise<void> {
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
    try {
      const tree = await requestJson(
        `${repositoryApi(this, "/tree")}?${this.query(revision, path)}`,
        treeSchema,
      );
      if (!this.#destroyed && this.isScopeCurrent())
        this.expandedTrees = { ...this.expandedTrees, [path]: tree };
    } catch (caught) {
      this.setError(errorMessage(caught));
    } finally {
      const nextLoading = new SvelteSet(this.loadingPaths);
      nextLoading.delete(path);
      this.loadingPaths = nextLoading;
    }
  }
  selectSubmodule(entry: Tree["entries"][number]): void {
    if (this.#destroyed || !this.isScopeCurrent()) return;
    this.#blobController?.abort();
    this.#blobController = null;
    this.#highlightSequence += 1;
    this.blob = null;
    this.highlighted = "";
    this.readme = null;
    this.selectedPath = entry.path;
    this.submodule = entry;
  }
  async selectBlob(path: string, revision: string): Promise<void> {
    this.#blobController?.abort();
    const controller = new AbortController();
    this.#blobController = controller;
    try {
      const blob = await requestJson(
        `${repositoryApi(this, "/blob")}?${this.query(revision, path)}`,
        blobSchema,
        { signal: controller.signal },
      );
      if (
        this.#blobController === controller &&
        !this.#destroyed &&
        this.isScopeCurrent()
      ) {
        void this.setBlob(blob);
        this.readme = null;
      }
    } catch (caught) {
      if (!(caught instanceof DOMException && caught.name === "AbortError"))
        this.setError(errorMessage(caught));
    } finally {
      if (this.#blobController === controller) this.#blobController = null;
    }
  }
  async loadStats(revision: string): Promise<void> {
    if (this.#statsRevision === revision) return;
    this.#statsRevision = revision;
    try {
      const stats = await requestJson(
        `${repositoryApi(this, "/stats")}?${new URLSearchParams({ rev: revision })}`,
        z.array(languageStatSchema),
      );
      if (
        !this.#destroyed &&
        this.isScopeCurrent() &&
        this.#statsRevision === revision
      )
        this.stats = stats;
    } catch {
      if (this.isScopeCurrent() && this.#statsRevision === revision)
        this.#statsRevision = "";
    }
  }
  async setBlob(blob: Blob): Promise<void> {
    if (!this.isScopeCurrent()) return;
    this.submodule = null;
    this.blob = blob;
    const source = blob.content;
    const sequence = ++this.#highlightSequence;
    this.highlighted = source ? escapeHtml(source) : "";
    if (!source || source.length > MAX_HIGHLIGHT_CHARACTERS) return;
    try {
      if (
        !this.#destroyed &&
        this.isScopeCurrent() &&
        sequence === this.#highlightSequence &&
        this.blob?.oid === blob.oid
      )
        this.highlighted = highlight(blob.path, source);
    } catch {
      /* escaped source remains visible */
    }
  }
  private query(revision: string, path = ""): string {
    const parameters = new URLSearchParams({ rev: revision });
    if (path) parameters.set("path", path);
    return parameters.toString();
  }
}
