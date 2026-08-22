<script lang="ts">
  import { BarChart3, Check, Copy, Pencil } from "lucide-svelte";

  import RepositoryToolbar from "$lib/components/repository/repository-toolbar.svelte";
  import RepositoryTopics from "$lib/components/repository/repository-topics.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { languageColor } from "$lib/repository/language-colors.js";
  import type {
    CopyTarget,
    RepositoryPageState,
  } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

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
      await repository.updateRepositoryControl({ description: next || null });
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

  function formatRepositorySize(bytes: number | undefined) {
    if (bytes === undefined) return "—";
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
      <h2
        class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
      >
        Clone
      </h2>
      <div class="mt-3 space-y-2">
        {#each cloneKinds as kind (kind.id)}
          <div>
            <p class="mb-1 text-[10px] font-medium text-muted-foreground">
              {kind.label}
            </p>
            <!-- The whole row is the copy target: the icon is only an
                 affordance, so nothing here may be a nested interactive. -->
            <button
              type="button"
              class="flex w-full items-center overflow-hidden rounded-md border bg-background text-left hover:border-input hover:bg-muted/40 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              onclick={() => void repository.copyCloneUrl(kind.id)}
              aria-label={`Copy ${kind.label} clone URL`}
            >
              <code
                class="min-w-0 flex-1 truncate px-3 py-2 text-xs text-muted-foreground"
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
          </div>
        {/each}
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
              disabled={repository.repositoryControlPending}
            >
              {repository.repositoryControlPending ? "Saving…" : "Save"}
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

      <dl class="mt-5 space-y-3 border-t pt-4 text-xs">
        <div class="flex justify-between gap-4">
          <dt class="text-muted-foreground">Repository size</dt>
          <dd class="tabular-nums">
            {formatRepositorySize(repository.refs?.size_bytes)}
          </dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-muted-foreground">Object format</dt>
          <dd>{repository.repository?.object_format.toUpperCase()}</dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-muted-foreground">Commits</dt>
          <dd>
            {repository.commitCount?.toLocaleString() ?? "—"}
          </dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-muted-foreground">Branches</dt>
          <dd>{repository.refs?.branches.length ?? 0}</dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-muted-foreground">Tags</dt>
          <dd>{repository.refs?.tags.length ?? 0}</dd>
        </div>
      </dl>
    </section>

    <section class="p-4">
      <div class="flex items-center justify-between gap-3">
        <h2
          class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
        >
          <BarChart3 class="size-3.5" />Statistics
        </h2>
        <span
          class="text-[11px] tabular-nums text-muted-foreground"
          title={`${repository.totalLines.toLocaleString()} non-blank lines`}
        >
          {compactCount(repository.totalLines)}
        </span>
      </div>
      {#if repository.stats.length}
        <div class="mt-4 flex h-1.5 overflow-hidden rounded-full bg-muted">
          {#each repository.stats as item (item.language)}
            <span
              style:width={`${repository.totalLines ? ((item.code + item.comments) / repository.totalLines) * 100 : 0}%`}
              style:background={languageColor(item.language)}
            ></span>
          {/each}
        </div>
        <ul class="mt-4 space-y-3">
          {#each repository.stats as item (item.language)}
            <li>
              <div class="flex items-center justify-between gap-3 text-xs">
                <span class="flex min-w-0 items-center gap-2 font-medium">
                  <span
                    class="size-2 shrink-0 rounded-full"
                    style:background={languageColor(item.language)}
                  ></span>
                  <span class="truncate">{item.language}</span>
                </span>
                <span
                  class="shrink-0 tabular-nums"
                  title={`${(item.code + item.comments).toLocaleString()} non-blank lines`}
                >
                  {compactCount(item.code + item.comments)}
                </span>
              </div>
              <div
                class="mt-1 flex flex-wrap gap-x-2 pl-4 text-[10px] text-muted-foreground"
              >
                <span>{item.files} file{item.files === 1 ? "" : "s"}</span>
                <span>{compactCount(item.code)} code</span>
                <span>{compactCount(item.comments)} comments</span>
              </div>
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
