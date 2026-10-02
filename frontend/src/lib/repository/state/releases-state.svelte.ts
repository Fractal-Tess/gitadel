import { toast } from "svelte-sonner";
import {
  releaseAssetSchema,
  releaseSchema,
  type Release,
} from "$lib/api/releases.js";
import {
  errorMessage,
  jsonBody,
  requestEmpty,
  requestJson,
} from "$lib/api/transport.js";
import { loadRepositoryReleases } from "$lib/repository/repository-data-cache.js";
import { appendPage, loadPages } from "$lib/repository/pagination.js";
import type { RepositoryFeatureContext } from "./shared.js";
import { repositoryApi } from "./shared.js";

type ReleasesCallbacks = {
  setError(message: string): void;
  isScopeCurrent(): boolean;
};
export class RepositoryReleasesState {
  readonly namespace: string;
  readonly name: string;
  readonly scope: RepositoryFeatureContext["scope"];
  readonly setError: ReleasesCallbacks["setError"];
  readonly isScopeCurrent: ReleasesCallbacks["isScopeCurrent"];
  releases = $state.raw<Release[]>([]);
  releasesLoading = $state(false);
  releasesLoaded = $state(false);
  releasesLoadFailed = $state(false);
  /** Whether the server holds releases beyond the ones loaded. */
  releasesHaveMore = $state(false);
  releasesLoadingMore = $state(false);
  releasePending = $state(false);
  releaseAssetPending = $state(false);
  #destroyed = false;
  #releasePages = 1;
  // Advanced by every full load, so a page requested before it is dropped.
  #releasesGeneration = 0;
  constructor(context: RepositoryFeatureContext, callbacks: ReleasesCallbacks) {
    this.namespace = context.locator.namespace;
    this.name = context.locator.name;
    this.scope = context.scope;
    this.setError = callbacks.setError;
    this.isScopeCurrent = callbacks.isScopeCurrent;
  }
  destroy(): void {
    this.#destroyed = true;
  }
  async refreshReleases(): Promise<void> {
    try {
      await this.loadReleases({});
    } catch (caught) {
      this.setError(errorMessage(caught));
    }
  }
  async loadReleases(init: RequestInit): Promise<void> {
    if (this.releasesLoading || (this.releasesLoaded && init.signal)) return;
    this.releasesLoading = true;
    this.releasesLoadFailed = false;
    try {
      // A reload covers every page already on screen, so the list does not
      // shrink back to its first page after an edit.
      const loaded = await loadPages(
        (page) =>
          loadRepositoryReleases(
            this.namespace,
            this.name,
            this.scope,
            page,
            !init.signal,
          ),
        this.#releasePages,
      );
      if (init.signal?.aborted || this.#destroyed || !this.isScopeCurrent())
        return;
      this.#releasesGeneration += 1;
      this.#releasePages = Math.max(1, loaded.pages);
      this.releases = loaded.items;
      this.releasesHaveMore = loaded.hasMore;
      this.releasesLoaded = true;
    } catch (caught) {
      this.releasesLoadFailed = true;
      throw caught;
    } finally {
      this.releasesLoading = false;
    }
  }
  async loadMoreReleases(): Promise<void> {
    if (this.releasesLoadingMore || !this.releasesHaveMore) return;
    const generation = this.#releasesGeneration;
    this.releasesLoadingMore = true;
    try {
      const page = await loadRepositoryReleases(
        this.namespace,
        this.name,
        this.scope,
        this.#releasePages + 1,
        true,
      );
      if (
        this.#destroyed ||
        !this.isScopeCurrent() ||
        generation !== this.#releasesGeneration
      )
        return;
      const loaded = appendPage(
        { items: this.releases, pages: this.#releasePages, hasMore: true },
        page,
      );
      this.#releasePages = loaded.pages;
      this.releases = loaded.items;
      this.releasesHaveMore = loaded.hasMore;
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      this.releasesLoadingMore = false;
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
  ): Promise<Release> {
    this.releasePending = true;
    try {
      let release = await requestJson(
        repositoryApi(this, "/releases"),
        releaseSchema,
        { method: "POST", body: jsonBody(values) },
      );
      if (!this.isScopeCurrent()) return release;
      this.releases = [release, ...this.releases];
      this.releasesLoaded = true;
      if (files.length) {
        this.releaseAssetPending = true;
        try {
          for (const file of files) {
            const asset = await requestJson(
              `${repositoryApi(this, `/releases/${release.id}/assets`)}?${new URLSearchParams({ name: file.name })}`,
              releaseAssetSchema,
              {
                method: "PUT",
                headers: {
                  "content-type": file.type || "application/octet-stream",
                },
                body: file,
              },
            );
            if (!this.isScopeCurrent()) return release;
            release = { ...release, assets: [...release.assets, asset] };
          }
        } catch (caught) {
          await this.loadReleases({}).catch(() => undefined);
          this.setError(
            `Release “${release.title}” was published, but some assets could not be uploaded: ${errorMessage(caught)} Add the remaining files from the published release.`,
          );
          return release;
        }
      }
      await this.loadReleases({}).catch((caught) => {
        this.setError(errorMessage(caught));
      });
      toast.success(`Release “${release.title}” published.`);
      return release;
    } catch (caught) {
      toast.error(errorMessage(caught));
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
  ): Promise<Release> {
    this.releasePending = true;
    try {
      const release = await requestJson(
        repositoryApi(this, `/releases/${id}`),
        releaseSchema,
        { method: "PATCH", body: jsonBody(values) },
      );
      if (!this.isScopeCurrent()) return release;
      await this.loadReleases({}).catch((caught) => {
        this.setError(errorMessage(caught));
      });
      toast.success("Release updated.");
      return release;
    } catch (caught) {
      toast.error(errorMessage(caught));
      throw caught;
    } finally {
      this.releasePending = false;
    }
  }
  async uploadReleaseAssets(releaseId: string, files: File[]): Promise<void> {
    this.releaseAssetPending = true;
    try {
      for (const file of files)
        await requestJson(
          `${repositoryApi(this, `/releases/${releaseId}/assets`)}?${new URLSearchParams({ name: file.name })}`,
          releaseAssetSchema,
          {
            method: "PUT",
            headers: {
              "content-type": file.type || "application/octet-stream",
            },
            body: file,
          },
        );
      if (!this.isScopeCurrent()) return;
      await this.loadReleases({}).catch((caught) => {
        this.setError(errorMessage(caught));
      });
      toast.success(
        `${files.length} release asset${files.length === 1 ? "" : "s"} uploaded.`,
      );
    } catch (caught) {
      toast.error(errorMessage(caught));
      throw caught;
    } finally {
      this.releaseAssetPending = false;
    }
  }
  async deleteRelease(id: string): Promise<void> {
    this.releasePending = true;
    try {
      await requestEmpty(repositoryApi(this, `/releases/${id}`), {
        method: "DELETE",
      });
      if (!this.isScopeCurrent()) return;
      await this.loadReleases({}).catch((caught) => {
        this.setError(errorMessage(caught));
      });
      toast.success("Release deleted. Its Git target was not changed.");
    } catch (caught) {
      toast.error(errorMessage(caught));
      throw caught;
    } finally {
      this.releasePending = false;
    }
  }
  async deleteReleaseAsset(releaseId: string, assetId: string): Promise<void> {
    this.releaseAssetPending = true;
    try {
      await requestEmpty(
        repositoryApi(this, `/releases/${releaseId}/assets/${assetId}`),
        { method: "DELETE" },
      );
      if (!this.isScopeCurrent()) return;
      await this.loadReleases({}).catch((caught) => {
        this.setError(errorMessage(caught));
      });
      toast.success("Release asset deleted.");
    } catch (caught) {
      toast.error(errorMessage(caught));
      throw caught;
    } finally {
      this.releaseAssetPending = false;
    }
  }
}
