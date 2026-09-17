import { SvelteDate } from "svelte/reactivity";
import { toast } from "svelte-sonner";
import {
  topicsSchema,
  repositorySchema,
  iconCandidatesSchema,
  type IconCandidates,
  type Repository,
} from "$lib/api/repositories.js";
import { jsonBody, requestEmpty, requestJson } from "$lib/api/transport.js";
import { blobToBase64 } from "$lib/avatar-crop.js";
import type { RepositoryFeatureContext } from "./shared.js";
import { errorMessage, repositoryApi } from "./shared.js";

type SettingsCallbacks = {
  setError(message: string): void;
  getRepository(): Repository | null;
  setRepository(repository: Repository): void;
  onDefaultBranchChanged(): Promise<void>;
  invalidatePreload(): void;
  invalidateData(datasets?: readonly string[]): void;
  isAuthenticated(): boolean;
  redirectToLogin(returnTo: string): void;
};
export class RepositorySettingsState {
  readonly namespace: string;
  readonly name: string;
  readonly scope: RepositoryFeatureContext["scope"];
  readonly isScopeCurrent: RepositoryFeatureContext["isScopeCurrent"];
  readonly setError: SettingsCallbacks["setError"];
  readonly getRepository: SettingsCallbacks["getRepository"];
  readonly setRepository: SettingsCallbacks["setRepository"];
  readonly onDefaultBranchChanged: SettingsCallbacks["onDefaultBranchChanged"];
  readonly invalidatePreload: SettingsCallbacks["invalidatePreload"];
  readonly invalidateData: SettingsCallbacks["invalidateData"];
  readonly isAuthenticated: SettingsCallbacks["isAuthenticated"];
  readonly redirectToLogin: SettingsCallbacks["redirectToLogin"];
  topics = $state.raw<string[]>([]);
  ownedNamespaces = $state.raw<string[]>([]);
  repositoryControlPending = $state(false);
  lifecyclePending = $state(false);
  favoritePending = $state(false);
  iconPending = $state(false);
  iconCandidates = $state.raw<IconCandidates | null>(null);
  iconCandidatesLoading = $state(false);
  iconCandidatesError = $state<string | null>(null);
  iconSelectionPending = $state(false);
  #iconRequestSequence = 0;
  constructor(context: RepositoryFeatureContext, callbacks: SettingsCallbacks) {
    this.namespace = context.locator.namespace;
    this.name = context.locator.name;
    this.scope = context.scope;
    this.isScopeCurrent = context.isScopeCurrent;
    this.setError = callbacks.setError;
    this.getRepository = callbacks.getRepository;
    this.setRepository = callbacks.setRepository;
    this.onDefaultBranchChanged = callbacks.onDefaultBranchChanged;
    this.invalidatePreload = callbacks.invalidatePreload;
    this.invalidateData = callbacks.invalidateData;
    this.isAuthenticated = callbacks.isAuthenticated;
    this.redirectToLogin = callbacks.redirectToLogin;
  }
  async saveTopics(topics: string[]): Promise<void> {
    try {
      const saved = await requestJson(
        repositoryApi(this, "/topics"),
        topicsSchema,
        { method: "PUT", body: jsonBody({ topics }) },
      );
      this.invalidatePreload();
      this.topics = saved.topics;
    } catch (caught) {
      toast.error(errorMessage(caught));
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
  async toggleFavorite(): Promise<void> {
    const repository = this.getRepository();
    if (!repository) return;
    if (!this.isAuthenticated()) {
      this.redirectToLogin(`/${this.namespace}/${this.name}`);
      return;
    }
    const favorited = !repository.favorited;
    this.favoritePending = true;
    try {
      await requestEmpty(repositoryApi(this, "/favorite"), {
        method: favorited ? "PUT" : "DELETE",
      });
      this.invalidatePreload();
      this.setRepository({ ...repository, favorited });
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      this.favoritePending = false;
    }
  }
  #applyIconCandidates(candidates: IconCandidates): void {
    this.iconCandidates = candidates;
    const repository = this.getRepository();
    if (
      repository &&
      (repository.icon_source !== candidates.mode ||
        repository.icon_updated_at !== candidates.icon_updated_at)
    ) {
      this.setRepository({
        ...repository,
        icon_source: candidates.mode,
        icon_updated_at: candidates.icon_updated_at,
      });
    }
  }
  async loadIconCandidates(init: RequestInit = {}): Promise<void> {
    if (this.iconPending || this.iconSelectionPending || !this.isScopeCurrent())
      return;
    const sequence = ++this.#iconRequestSequence;
    this.iconCandidatesLoading = true;
    this.iconCandidatesError = null;
    try {
      const candidates = await requestJson(
        repositoryApi(this, "/icon/candidates"),
        iconCandidatesSchema,
        init,
      );
      if (
        sequence === this.#iconRequestSequence &&
        !init.signal?.aborted &&
        this.isScopeCurrent()
      ) {
        this.#applyIconCandidates(candidates);
      }
    } catch (caught) {
      if (
        sequence === this.#iconRequestSequence &&
        this.isScopeCurrent() &&
        !(caught instanceof DOMException && caught.name === "AbortError")
      ) {
        this.iconCandidatesError = errorMessage(caught);
        toast.error(this.iconCandidatesError);
      }
    } finally {
      if (sequence === this.#iconRequestSequence)
        this.iconCandidatesLoading = false;
    }
  }
  async saveIconSelection(
    mode: "automatic" | "selected" | "none",
    path?: string,
    commitOid?: string,
  ): Promise<void> {
    if (mode === "selected" && !path) return;
    if (this.iconPending || this.iconSelectionPending || !this.isScopeCurrent())
      return;
    const sequence = ++this.#iconRequestSequence;
    this.iconSelectionPending = true;
    this.iconCandidatesLoading = false;
    this.iconCandidatesError = null;
    try {
      const candidates = await requestJson(
        repositoryApi(this, "/icon/selection"),
        iconCandidatesSchema,
        {
          method: "PUT",
          body: jsonBody({
            mode,
            ...(path ? { path } : {}),
            ...(commitOid ? { commit_oid: commitOid } : {}),
          }),
        },
      );
      if (sequence !== this.#iconRequestSequence || !this.isScopeCurrent())
        return;
      this.#applyIconCandidates(candidates);
      this.invalidatePreload();
      toast.success(
        mode === "none"
          ? "Repository icon removed."
          : "Repository icon updated.",
      );
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      this.iconSelectionPending = false;
    }
  }
  // The icon endpoints return no body, so the timestamp is advanced locally to
  // bust the image cache rather than costing a second round trip.
  async updateIcon(image: Blob): Promise<void> {
    if (this.iconPending || this.iconSelectionPending || !this.isScopeCurrent())
      return;
    const sequence = ++this.#iconRequestSequence;
    this.iconPending = true;
    this.iconCandidatesLoading = false;
    this.iconCandidatesError = null;
    try {
      await requestEmpty(repositoryApi(this, "/icon"), {
        method: "PUT",
        body: jsonBody({ image_base64: await blobToBase64(image) }),
      });
      if (sequence !== this.#iconRequestSequence || !this.isScopeCurrent())
        return;
      const updatedAt = new SvelteDate().toISOString();
      if (this.iconCandidates) {
        this.iconCandidates = {
          ...this.iconCandidates,
          mode: "uploaded",
          selected_path: null,
          selected_missing: false,
          icon_updated_at: updatedAt,
        };
      }
      const repository = this.getRepository();
      if (repository) {
        this.setRepository({
          ...repository,
          icon_updated_at: updatedAt,
          icon_source: "uploaded",
        });
      }
      this.invalidatePreload();
      toast.success("Repository icon updated.");
    } catch (caught) {
      toast.error(errorMessage(caught));
      throw caught;
    } finally {
      this.iconPending = false;
    }
  }
  async updateRepositoryControl(values: {
    description?: string | null;
    default_branch?: string | null;
    name?: string;
    namespace?: string;
  }): Promise<void> {
    this.repositoryControlPending = true;
    try {
      const repository = await requestJson(
        repositoryApi(this, "/control"),
        repositorySchema,
        { method: "PATCH", body: jsonBody(values) },
      );
      const current = this.getRepository();
      const moved =
        repository.namespace !== this.namespace ||
        repository.name !== this.name;
      this.invalidatePreload();
      this.invalidateData();
      this.setRepository(repository);
      if (moved) {
        window.location.assign(
          `/${encodeURIComponent(repository.namespace)}/${encodeURIComponent(repository.name)}?view=settings`,
        );
        return;
      }
      toast.success("Repository settings saved.");
      if (
        values.default_branch &&
        current?.default_branch !== values.default_branch
      ) {
        try {
          await this.onDefaultBranchChanged();
        } catch (caught) {
          this.setError(errorMessage(caught));
        }
      }
    } catch (caught) {
      toast.error(errorMessage(caught));
      throw caught;
    } finally {
      this.repositoryControlPending = false;
    }
  }
  async setArchived(archived: boolean): Promise<void> {
    await this.lifecycleRequest(
      "/archive",
      archived ? "POST" : "DELETE",
      archived
        ? "Repository archived. Cloning remains available; pushes are blocked."
        : "Repository unarchived.",
    );
  }
  async softDelete(): Promise<void> {
    await this.lifecycleRequest(
      "/delete",
      "POST",
      "Repository deleted. You can restore it during the recovery period.",
    );
    window.location.assign("/");
  }
  private async lifecycleRequest(
    path: string,
    method: string,
    notice: string,
  ): Promise<void> {
    this.lifecyclePending = true;
    try {
      await requestEmpty(repositoryApi(this, path), { method });
      this.invalidatePreload();
      this.invalidateData();
      const repository = this.getRepository();
      if (repository && path === "/archive")
        this.setRepository({
          ...repository,
          archived_at:
            method === "POST" ? new SvelteDate().toISOString() : null,
        });
      toast.success(notice);
    } catch (caught) {
      toast.error(errorMessage(caught));
      throw caught;
    } finally {
      this.lifecyclePending = false;
    }
  }
}
