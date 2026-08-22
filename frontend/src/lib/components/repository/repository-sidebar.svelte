<script lang="ts">
  import { BarChart3, Check, Copy, Pencil } from "lucide-svelte";

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
</script>

<aside class="order-first flex min-w-0 flex-col xl:order-none">
  <section
    class="order-last border-t p-4 xl:order-first xl:border-t-0 xl:border-b"
  >
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
          <div
            class="flex items-center overflow-hidden rounded-md border bg-background"
          >
            <code
              class="min-w-0 flex-1 truncate px-3 py-2 text-xs text-muted-foreground"
            >
              {kind.id === "http"
                ? repository.httpCloneUrl
                : repository.repository?.ssh_clone_url}
            </code>
            <Button
              variant="ghost"
              size="icon-sm"
              class="shrink-0 rounded-none border-l text-muted-foreground"
              onclick={() => void repository.copyCloneUrl(kind.id)}
              aria-label={`Copy ${kind.label} clone URL`}
            >
              {#if repository.copied === kind.id}
                <Check class="size-3.5 text-emerald-500" />
              {:else}
                <Copy class="size-3.5" />
              {/if}
            </Button>
          </div>
        </div>
      {/each}
    </div>
  </section>

  <section class="order-first p-4 xl:order-2">
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
      <p class="mt-3 text-sm leading-6">{repository.repository.description}</p>
    {:else}
      <p class="mt-3 text-sm leading-6 text-muted-foreground">
        No description provided.
      </p>
    {/if}

    <RepositoryTopics state={repository} />

    <dl class="mt-5 space-y-3 border-t pt-4 text-xs">
      <div class="flex justify-between gap-4">
        <dt class="text-muted-foreground">Default branch</dt>
        <dd class="font-mono">{repository.repository?.default_branch}</dd>
      </div>
      <div class="flex justify-between gap-4">
        <dt class="text-muted-foreground">Object format</dt>
        <dd>{repository.repository?.object_format.toUpperCase()}</dd>
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

  <section class="order-2 border-t p-4 xl:order-last">
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
</aside>
