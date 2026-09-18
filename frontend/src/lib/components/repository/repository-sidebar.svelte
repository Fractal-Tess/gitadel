<script lang="ts">
  import ArrowRight from "@lucide/svelte/icons/arrow-right";
  import BarChart3 from "@lucide/svelte/icons/bar-chart-3";
  import Check from "@lucide/svelte/icons/check";
  import Copy from "@lucide/svelte/icons/copy";
  import Package from "@lucide/svelte/icons/package";
  import Pencil from "@lucide/svelte/icons/pencil";

  import RepositoryToolbar from "$lib/components/repository/repository-toolbar.svelte";
  import RepositoryTopics from "$lib/components/repository/repository-topics.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import * as Tooltip from "$lib/components/ui/tooltip/index.js";
  import { languageColor } from "$lib/repository/language-colors.js";
  import { repositoryApi } from "$lib/repository/state/shared.js";
  import type {
    CopyTarget,
    RepositoryPageState,
  } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  let gitBytes = $derived(
    repository.browser.refs?.size_bytes != null &&
      repository.browser.refs.lfs_size_bytes != null
      ? repository.browser.refs.size_bytes -
          repository.browser.refs.lfs_size_bytes
      : null,
  );

  // LFS only earns its own cell once something is actually stored there;
  // otherwise the repository size is a single number and splitting it three
  // ways would just repeat it.
  let lfsBytes = $derived(repository.browser.refs?.lfs_size_bytes ?? null);
  let hasLfs = $derived((lfsBytes ?? 0) > 0);

  let editingDescription = $state(false);
  let descriptionDraft = $state("");
  let descriptionField = $state<HTMLTextAreaElement | null>(null);

  function startEditingDescription(): void {
    descriptionDraft = repository.repository?.description ?? "";
    editingDescription = true;
  }

  function cancelEditingDescription(): void {
    editingDescription = false;
    descriptionDraft = "";
  }

  async function saveDescription(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const next = descriptionDraft.trim();
    if (next === (repository.repository?.description ?? "")) {
      cancelEditingDescription();
      return;
    }
    try {
      await repository.settings.updateRepositoryControl({
        description: next || null,
      });
      cancelEditingDescription();
    } catch {
      // updateRepositoryControl surfaces the message; keep the draft editable.
    }
  }

  $effect(() => {
    if (editingDescription) descriptionField?.focus();
  });

  const cloneKinds: Array<{ id: CopyTarget; label: string }> = [
    { id: "http", label: "HTTP" },
    { id: "ssh", label: "SSH" },
  ];

  const compactUnits = [
    { threshold: 1_000, suffix: "k" },
    { threshold: 1_000_000, suffix: "m" },
    { threshold: 1_000_000_000, suffix: "b" },
  ] as const;
  const compactDecimal = new Intl.NumberFormat("en", {
    maximumFractionDigits: 1,
  });

  function compactCount(value: number): string {
    let unitIndex = -1;
    for (let index = 0; index < compactUnits.length; index += 1) {
      if (value < compactUnits[index].threshold) break;
      unitIndex = index;
    }
    if (unitIndex < 0) return value.toLocaleString("en");

    let unit = compactUnits[unitIndex];
    let rounded = Math.ceil((value / unit.threshold) * 10) / 10;
    if (rounded >= 1_000 && unitIndex < compactUnits.length - 1) {
      unit = compactUnits[unitIndex + 1];
      rounded = Math.ceil((value / unit.threshold) * 10) / 10;
    }
    return `${compactDecimal.format(rounded)}${unit.suffix}`;
  }

  function sourceArchiveUrl(format: "zip" | "tar.gz"): string {
    const current = repository.repository;
    if (!current) return "";
    const endpoint = repositoryApi(current, "/source");
    return `${endpoint}?${new URLSearchParams({
      rev: repository.revision,
      format,
    })}`;
  }

  function formatReleaseDate(value: string) {
    return new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
      year: "numeric",
    }).format(new Date(value));
  }

  function formatRepositorySize(bytes: number | null | undefined) {
    if (bytes == null) return "—";
    if (bytes === 0) return "0 KB";
    if (bytes < 1_024) return "<1 KB";
    if (bytes < 1_024 ** 2) return `${compactDecimal.format(bytes / 1_024)} KB`;
    if (bytes < 1_024 ** 3)
      return `${compactDecimal.format(bytes / 1_024 ** 2)} MB`;
    return `${compactDecimal.format(bytes / 1_024 ** 3)} GB`;
  }
</script>

<aside
  class="flex h-fit min-w-0 flex-col border-t bg-card/20 xl:h-full xl:min-h-0 xl:border-l xl:border-t-0"
>
  <RepositoryToolbar state={repository} />

  <!-- The toolbar is pinned so it keeps forming the divider under the app
       header; only the metadata below it scrolls. -->
  <div class="min-h-0 flex-1 divide-y xl:overflow-y-auto xl:overscroll-contain">
    <section class="p-4">
      <div class="flex items-center justify-between gap-2">
        <h2
          class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
        >
          Clone
        </h2>
        {#if repository.repository?.mirrored}
          <span
            class="rounded-full border border-sky-500/30 bg-sky-500/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-sky-600 dark:text-sky-400"
          >
            Mirror
          </span>
        {/if}
      </div>
      <div class="mt-3 flex flex-col gap-2">
        {#each cloneKinds as kind (kind.id)}
          <!-- The whole row is the copy target: the icon is only an
               affordance, so nothing here may be a nested interactive. -->
          <button
            type="button"
            class="flex w-full cursor-pointer items-center overflow-hidden rounded-md border bg-background text-left hover:border-input hover:bg-muted/40 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            onclick={() => void repository.copyCloneUrl(kind.id)}
            aria-label={`Copy ${kind.label} clone URL`}
          >
            <span
              class="grid w-11 shrink-0 place-items-center self-stretch border-r text-[10px] font-medium text-muted-foreground"
            >
              {kind.label}
            </span>
            <code
              class="min-w-0 flex-1 truncate px-2 py-2 text-xs text-muted-foreground"
            >
              {kind.id === "http"
                ? repository.httpCloneUrl
                : repository.repository?.ssh_clone_url}
            </code>
            <span
              class="grid w-7 shrink-0 place-items-center self-stretch border-l text-muted-foreground"
            >
              {#if repository.copied === kind.id}
                <Check class="size-3.5 text-emerald-500" />
              {:else}
                <Copy class="size-3.5" />
              {/if}
            </span>
          </button>
        {/each}
        {#if repository.repository}
          <div
            class="flex w-full items-stretch overflow-hidden rounded-md border bg-background text-xs"
          >
            <span
              class="grid w-11 shrink-0 place-items-center border-r text-[10px] font-medium text-muted-foreground"
            >
              Source
            </span>
            <a
              data-sveltekit-reload
              class="flex flex-1 cursor-pointer items-center justify-center px-2 py-2 font-medium hover:bg-muted/40"
              href={sourceArchiveUrl("zip")}
            >
              ZIP
            </a>
            <a
              data-sveltekit-reload
              class="flex flex-1 cursor-pointer items-center justify-center border-l px-2 py-2 font-medium hover:bg-muted/40"
              href={sourceArchiveUrl("tar.gz")}
            >
              TAR.GZ
            </a>
          </div>
        {/if}
      </div>
    </section>

    <section class="p-4">
      <div class="flex min-h-6 items-center justify-between gap-2">
        <h2
          class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
        >
          Description
        </h2>
        {#if repository.repository?.can_manage && !editingDescription}
          <Button
            variant="ghost"
            size="icon-xs"
            class="text-muted-foreground"
            aria-label="Edit description"
            onclick={startEditingDescription}
          >
            <Pencil class="size-3.5" />
          </Button>
        {/if}
      </div>

      {#if editingDescription}
        <form class="mt-3" onsubmit={saveDescription}>
          <Textarea
            bind:value={descriptionDraft}
            bind:ref={descriptionField}
            class="min-h-20 text-sm"
            maxlength={512}
            placeholder="Describe this repository"
            aria-label="Repository description"
            onkeydown={(event) => {
              if (event.key === "Escape") cancelEditingDescription();
            }}
          />
          <div class="mt-2 flex items-center justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onclick={cancelEditingDescription}
            >
              Cancel
            </Button>
            <Button
              type="submit"
              size="sm"
              disabled={repository.settings.repositoryControlPending}
            >
              {repository.settings.repositoryControlPending
                ? "Saving…"
                : "Save"}
            </Button>
          </div>
        </form>
      {:else if repository.repository?.description}
        <p class="mt-3 text-sm leading-6">
          {repository.repository.description}
        </p>
      {:else}
        <p class="mt-3 text-sm leading-6 text-muted-foreground">
          No description provided.
        </p>
      {/if}

      <RepositoryTopics state={repository} />

      {#if hasLfs}
        <dl
          class="mt-5 grid grid-cols-3 divide-x overflow-hidden rounded-md border text-center"
        >
          <div class="px-2 py-2.5">
            <dt
              class="text-[10px] font-medium uppercase tracking-wide text-muted-foreground"
              title="Packed git objects, excluding LFS."
            >
              Git
            </dt>
            <dd class="mt-1 text-xs tabular-nums">
              {formatRepositorySize(gitBytes)}
            </dd>
          </div>
          <div class="px-2 py-2.5">
            <dt
              class="text-[10px] font-medium uppercase tracking-wide text-muted-foreground"
              title="All LFS objects stored for this repository, across branches and history."
            >
              LFS
            </dt>
            <dd class="mt-1 text-xs tabular-nums">
              {formatRepositorySize(lfsBytes)}
            </dd>
          </div>
          <div class="px-2 py-2.5">
            <dt
              class="text-[10px] font-medium uppercase tracking-wide text-muted-foreground"
              title="Git objects and LFS objects combined."
            >
              Total
            </dt>
            <dd class="mt-1 text-xs font-medium tabular-nums">
              {formatRepositorySize(repository.browser.refs?.size_bytes)}
            </dd>
          </div>
        </dl>
      {/if}

      <dl class="mt-4 flex flex-col gap-3 text-xs">
        {#if !hasLfs}
          <div class="flex justify-between gap-4">
            <dt class="text-muted-foreground">Size</dt>
            <dd class="tabular-nums">
              {formatRepositorySize(repository.browser.refs?.size_bytes)}
            </dd>
          </div>
        {/if}
        <div class="flex justify-between gap-4">
          <dt class="text-muted-foreground">Commits</dt>
          <dd>
            {repository.browser.commitCount?.toLocaleString() ?? "—"}
          </dd>
        </div>
        {#if (repository.browser.refs?.branches.length ?? 0) > 1}
          <div class="flex justify-between gap-4">
            <dt class="text-muted-foreground">Branches</dt>
            <dd>{repository.browser.refs?.branches.length}</dd>
          </div>
        {/if}
        {#if (repository.browser.refs?.tags.length ?? 0) > 0}
          <div class="flex justify-between gap-4">
            <dt class="text-muted-foreground">Tags</dt>
            <dd>{repository.browser.refs?.tags.length}</dd>
          </div>
        {/if}
      </dl>
    </section>

    {#if repository.releases.releasesLoading || repository.releases.releasesLoadFailed || repository.releases.releases.length}
      <section class="p-4">
        <div class="flex items-center justify-between gap-3">
          <h2
            class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
          >
            <Package class="size-3.5" />Releases
          </h2>
          {#if repository.releases.releases.length}
            <button
              type="button"
              class="flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground"
              onclick={() => repository.navigate("releases")}
            >
              {repository.releases.releases.length} total<ArrowRight
                class="size-3"
              />
            </button>
          {/if}
        </div>
        {#if repository.releases.releases[0]}
          {@const latest =
            repository.releases.releases.find((release) => release.latest) ??
            repository.releases.releases[0]}
          <button
            type="button"
            class="mt-3 block w-full text-left"
            onclick={() => repository.navigate("releases")}
          >
            <span class="block truncate text-sm font-medium hover:underline"
              >{latest.title}</span
            >
            <span
              class="mt-1 flex items-center justify-between gap-3 text-xs text-muted-foreground"
            >
              <code class="truncate">{latest.target_revision}</code>
              <span class="shrink-0"
                >{formatReleaseDate(latest.published_at)}</span
              >
            </span>
          </button>
        {:else if repository.releases.releasesLoading}
          <p class="mt-3 text-xs text-muted-foreground">Loading releases…</p>
        {:else if repository.releases.releasesLoadFailed}
          <Alert.Root class="mt-3 p-2 text-xs" variant="destructive">
            <Alert.Title>Releases unavailable</Alert.Title>
            <Alert.Description>
              <Button
                variant="link"
                class="h-auto p-0 text-xs"
                onclick={() => void repository.releases.refreshReleases()}
                >Releases unavailable. Try again.</Button
              >
            </Alert.Description>
          </Alert.Root>
        {/if}
      </section>
    {/if}

    <section class="p-4">
      <div class="flex items-center justify-between gap-3">
        <h2
          class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
        >
          <BarChart3 class="size-3.5" />Statistics
        </h2>
        <span
          class="text-[11px] tabular-nums text-muted-foreground"
          title={`${repository.browser.totalLines.toLocaleString()} non-blank lines`}
        >
          {compactCount(repository.browser.totalLines)}
        </span>
      </div>
      {#if repository.browser.stats.length}
        <div class="mt-4 flex h-1.5 overflow-hidden rounded-full bg-muted">
          {#each repository.browser.stats as item (item.language)}
            <span
              style:width={`${repository.browser.totalLines ? ((item.code + item.comments) / repository.browser.totalLines) * 100 : 0}%`}
              style:background={languageColor(item.language)}
            ></span>
          {/each}
        </div>
        <ul class="mt-4 flex flex-col gap-1">
          {#each repository.browser.stats as item (item.language)}
            <li>
              <Tooltip.Root>
                <Tooltip.Trigger
                  class="flex w-full cursor-help items-center justify-between gap-3 rounded-md px-2 py-1.5 text-xs hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  aria-label={`${item.language} statistics`}
                >
                  <span class="flex min-w-0 items-center gap-2 font-medium">
                    <span
                      class="size-2 shrink-0 rounded-full"
                      style:background={languageColor(item.language)}
                    ></span>
                    <span class="truncate">{item.language}</span>
                  </span>
                  <span class="shrink-0 tabular-nums">
                    {compactCount(item.code + item.comments)}
                  </span>
                </Tooltip.Trigger>
                <Tooltip.Content
                  side="left"
                  align="center"
                  sideOffset={8}
                  class="block w-48"
                >
                  <p class="font-medium">{item.language}</p>
                  <dl class="mt-1.5 grid grid-cols-[1fr_auto] gap-x-4 gap-y-1">
                    <dt>Files</dt>
                    <dd class="text-right tabular-nums">
                      {item.files.toLocaleString()}
                    </dd>
                    <dt>Code</dt>
                    <dd class="text-right tabular-nums">
                      {item.code.toLocaleString()}
                    </dd>
                    <dt>Comments</dt>
                    <dd class="text-right tabular-nums">
                      {item.comments.toLocaleString()}
                    </dd>
                    <dt>Blank</dt>
                    <dd class="text-right tabular-nums">
                      {item.blanks.toLocaleString()}
                    </dd>
                    <dt>Total</dt>
                    <dd class="text-right tabular-nums">
                      {(
                        item.code +
                        item.comments +
                        item.blanks
                      ).toLocaleString()}
                    </dd>
                  </dl>
                </Tooltip.Content>
              </Tooltip.Root>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="mt-3 text-xs text-muted-foreground">
          No recognized source files.
        </p>
      {/if}
    </section>
  </div>
</aside>
