<script lang="ts">
  import { resolve } from "$app/paths";
  import {
    CalendarDays,
    Download,
    FileArchive,
    GitCommitHorizontal,
    Package,
    Pencil,
    Plus,
    Trash2,
    Upload,
  } from "lucide-svelte";

  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import ReleaseComposerDialog from "$lib/components/repository/release-composer-dialog.svelte";
  import { formatSize, trustedHtml } from "$lib/repository/format.js";
  import type { Release } from "$lib/api.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  let composerOpen = $state(false);
  let composerRelease = $state<Release | null>(null);
  let assetFiles = $state<Record<string, File[]>>({});

  function beginCreate() {
    composerRelease = null;
    composerOpen = true;
  }

  function beginEdit(release: Release) {
    composerRelease = release;
    composerOpen = true;
  }

  async function removeRelease(release: Release) {
    if (
      !globalThis.confirm(
        `Delete the release “${release.title}”? Its target commit and Git refs will not be changed.`,
      )
    )
      return;
    try {
      await repository.deleteRelease(release.id);
    } catch {
      // The page-level error explains the failure.
    }
  }

  async function uploadAssets(releaseId: string) {
    const selected = assetFiles[releaseId] ?? [];
    if (!selected.length) return;
    try {
      await repository.uploadReleaseAssets(releaseId, selected);
      assetFiles = { ...assetFiles, [releaseId]: [] };
    } catch {
      // The page-level error explains the failure.
    }
  }

  function revisionUrl(oid: string) {
    const path = resolve("/[namespace]/[name]", {
      namespace: repository.namespace,
      name: repository.name,
    });
    return `${path}?${new URLSearchParams({ rev: oid })}`;
  }

  function publishedDate(value: string) {
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(new Date(value));
  }
</script>

<div class="mx-auto max-w-5xl">
  <header
    class="flex flex-wrap items-center justify-between gap-4 border-b pb-4"
  >
    <div>
      <h1 class="text-xl font-semibold tracking-tight">Releases</h1>
      <p class="mt-1 text-sm text-muted-foreground">
        Published snapshots, release notes, and downloadable assets.
      </p>
    </div>
    {#if repository.repository?.can_manage}
      <Button class="gap-2" onclick={beginCreate}>
        <Plus class="size-4" />New release
      </Button>
    {/if}
  </header>

  {#if repository.releasesLoading && !repository.releasesLoaded}
    <p class="py-16 text-center text-sm text-muted-foreground">
      Loading releases…
    </p>
  {:else if repository.releasesLoadFailed && !repository.releasesLoaded}
    <div class="py-16 text-center text-sm text-muted-foreground">
      <p>Releases could not be loaded.</p>
      <Button
        variant="link"
        class="mt-2"
        onclick={() => void repository.refreshReleases()}>Try again</Button
      >
    </div>
  {:else if repository.releases.length}
    <div class="divide-y">
      {#each repository.releases as release (release.id)}
        <article class="grid gap-5 py-7 md:grid-cols-[10rem_minmax(0,1fr)]">
          <div class="space-y-2 text-xs text-muted-foreground">
            <div class="flex items-center gap-2 font-mono text-foreground">
              <GitCommitHorizontal class="size-3.5" />
              <Button
                variant="link"
                class="h-auto min-w-0 truncate p-0 font-mono text-xs"
                href={revisionUrl(release.target_oid)}
              >
                {release.target_revision}
              </Button>
            </div>
            <code class="block text-[11px]"
              >{release.target_oid.slice(0, 12)}</code
            >
            <p class="flex items-start gap-2 leading-5">
              <CalendarDays class="mt-0.5 size-3.5 shrink-0" />
              <span>{publishedDate(release.published_at)}</span>
            </p>
          </div>

          <div class="min-w-0">
            <div class="flex flex-wrap items-start justify-between gap-3">
              <div class="min-w-0">
                <div class="flex flex-wrap items-center gap-2">
                  <h2 class="text-lg font-semibold tracking-tight">
                    {release.title}
                  </h2>
                  {#if release.latest}
                    <Badge>Latest</Badge>
                  {:else if release.prerelease}
                    <Badge variant="secondary">Pre-release</Badge>
                  {/if}
                </div>
                <p class="mt-1 text-xs text-muted-foreground">
                  Published by <span class="font-medium text-foreground"
                    >{release.author}</span
                  >
                </p>
              </div>
              {#if repository.repository?.can_manage}
                <div class="flex items-center gap-1">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Edit ${release.title}`}
                    onclick={() => beginEdit(release)}
                  >
                    <Pencil class="size-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    class="text-muted-foreground hover:text-destructive"
                    aria-label={`Delete ${release.title}`}
                    onclick={() => void removeRelease(release)}
                  >
                    <Trash2 class="size-3.5" />
                  </Button>
                </div>
              {/if}
            </div>

            {#if release.rendered_body}
              <div
                class="prose prose-invert mt-5 max-w-none text-sm prose-code:before:content-none prose-code:after:content-none"
                {@attach trustedHtml(release.rendered_body)}
              ></div>
            {:else}
              <p class="mt-5 text-sm italic text-muted-foreground">
                No release notes provided.
              </p>
            {/if}

            <div class="mt-6 overflow-hidden rounded-md border">
              <div
                class="flex items-center gap-2 border-b bg-muted/20 px-4 py-3 text-sm font-medium"
              >
                <Package class="size-4 text-muted-foreground" />
                Assets
                <span class="text-xs font-normal text-muted-foreground"
                  >{release.assets.length}</span
                >
              </div>
              <ul class="divide-y">
                {#each release.assets as asset (asset.id)}
                  <li class="flex items-center gap-3 px-4 py-3 text-sm">
                    <FileArchive
                      class="size-4 shrink-0 text-muted-foreground"
                    />
                    <Button
                      variant="link"
                      class="h-auto min-w-0 flex-1 justify-start truncate p-0 font-medium"
                      href={asset.download_url}
                    >
                      {asset.name}
                    </Button>
                    <span class="shrink-0 text-xs text-muted-foreground"
                      >{formatSize(asset.size_bytes)}</span
                    >
                    <span
                      class="hidden shrink-0 items-center gap-1 text-xs text-muted-foreground sm:flex"
                    >
                      <Download class="size-3" />{asset.download_count}
                    </span>
                    {#if repository.repository?.can_manage}
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        class="text-muted-foreground hover:text-destructive"
                        aria-label={`Delete ${asset.name}`}
                        disabled={repository.releaseAssetPending}
                        onclick={() =>
                          void repository.deleteReleaseAsset(
                            release.id,
                            asset.id,
                          )}
                      >
                        <Trash2 class="size-3" />
                      </Button>
                    {/if}
                  </li>
                {:else}
                  <li class="px-4 py-4 text-sm text-muted-foreground">
                    No uploaded assets.
                  </li>
                {/each}
              </ul>
              {#if repository.repository?.can_manage}
                <div
                  class="flex flex-wrap items-center gap-2 border-t bg-muted/10 p-3"
                >
                  <Input
                    type="file"
                    multiple
                    class="min-w-0 flex-1"
                    onchange={(event) => {
                      assetFiles = {
                        ...assetFiles,
                        [release.id]: Array.from(
                          event.currentTarget.files ?? [],
                        ),
                      };
                    }}
                  />
                  <Button
                    variant="outline"
                    size="sm"
                    class="gap-2"
                    disabled={repository.releaseAssetPending ||
                      !assetFiles[release.id]?.length}
                    onclick={() => void uploadAssets(release.id)}
                  >
                    <Upload class="size-3.5" />Upload
                  </Button>
                </div>
              {/if}
            </div>
          </div>
        </article>
      {/each}
    </div>
  {:else}
    <div class="py-20 text-center">
      <Package class="mx-auto size-9 text-muted-foreground" />
      <h2 class="mt-4 font-medium">No releases published</h2>
      <p class="mx-auto mt-2 max-w-md text-sm leading-6 text-muted-foreground">
        Releases pair a commit snapshot with rendered notes and downloadable
        assets.
      </p>
      {#if repository.repository?.can_manage}
        <Button class="mt-5 gap-2" onclick={beginCreate}>
          <Plus class="size-4" />Publish the first release
        </Button>
      {/if}
    </div>
  {/if}
</div>

<ReleaseComposerDialog
  bind:open={composerOpen}
  state={repository}
  release={composerRelease}
/>
