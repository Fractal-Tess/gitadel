<!--
  Chooses which repositories a namespace pins and in what order. The pinned
  list is edited in place; the rest are added from a searchable list below it.
  Each change is saved as it is made.
-->
<script lang="ts">
  import ArrowDown from "@lucide/svelte/icons/arrow-down";
  import ArrowUp from "@lucide/svelte/icons/arrow-up";
  import Plus from "@lucide/svelte/icons/plus";
  import X from "@lucide/svelte/icons/x";
  import { untrack } from "svelte";
  import { toast } from "svelte-sonner";
  import { z } from "zod";

  import { pinnedRepositoriesSchema } from "$lib/api/profile.js";
  import type { PinnedRepositories } from "$lib/api/profile.js";
  import { repositorySchema } from "$lib/api/repositories.js";
  import type { Repository } from "$lib/api/repositories.js";
  import { ApiFailure, jsonBody, requestJson } from "$lib/api/transport.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Input } from "$lib/components/ui/input/index.js";

  let {
    namespace,
    limit,
    current,
    open = $bindable(false),
    onsaved,
  }: {
    namespace: string;
    limit: number;
    current: { id: string }[];
    open: boolean;
    onsaved: (pins: PinnedRepositories) => void;
  } = $props();

  let repositories = $state.raw<Repository[]>([]);
  let selected = $state<string[]>([]);
  let query = $state("");
  let loading = $state(false);
  /** The pins as the server last accepted them. */
  let saved: string[] = [];
  let loadError = $state<string | null>(null);

  const byId = $derived(
    new Map(repositories.map((repository) => [repository.id, repository])),
  );
  const pinned = $derived(
    selected.flatMap((id) => {
      const repository = byId.get(id);
      return repository ? [repository] : [];
    }),
  );
  const available = $derived(
    repositories.filter(
      (repository) =>
        !selected.includes(repository.id) &&
        repository.name.toLowerCase().includes(query.trim().toLowerCase()),
    ),
  );
  const full = $derived(selected.length >= limit);

  // Each time the dialog opens, start from what is pinned now.
  $effect(() => {
    if (!open) return;
    // Saves update `current` through the parent, so reading it untracked keeps
    // each save from reloading the dialog underneath the person editing it.
    const ids = untrack(() => current.map((repository) => repository.id));
    selected = ids;
    saved = ids;
    query = "";
    loadError = null;
    loading = true;
    let cancelled = false;
    requestJson("/api/v1/repositories", z.array(repositorySchema)).then(
      (all) => {
        if (cancelled) return;
        repositories = all.filter(
          (repository) => repository.namespace === namespace,
        );
        loading = false;
      },
      (caught) => {
        if (cancelled) return;
        loadError =
          caught instanceof Error ? caught.message : "Could not load repositories.";
        loading = false;
      },
    );
    return () => {
      cancelled = true;
    };
  });

  let saveSequence = 0;

  /**
   * Every change saves at once. The list updates straight away, and a failed
   * save puts back what the server last accepted. Only the latest save's
   * answer is applied, so quick successive edits cannot land out of order.
   */
  async function update(next: string[]): Promise<void> {
    const previous = saved;
    selected = next;
    const sequence = ++saveSequence;
    try {
      const result = await requestJson(
        `/api/v1/namespaces/${encodeURIComponent(namespace)}/pins`,
        pinnedRepositoriesSchema,
        { method: "PUT", body: jsonBody({ repository_ids: next }) },
      );
      if (sequence !== saveSequence) return;
      saved = next;
      onsaved(result);
    } catch (caught) {
      if (sequence !== saveSequence) return;
      selected = previous;
      toast.error(
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not save the pinned repositories.",
      );
    }
  }

  function pin(id: string): void {
    if (!full && !selected.includes(id)) void update([...selected, id]);
  }

  function move(index: number, offset: number): void {
    const target = index + offset;
    if (target < 0 || target >= selected.length) return;
    const next = [...selected];
    [next[index], next[target]] = [next[target], next[index]];
    void update(next);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>Customize pins</Dialog.Title>
      <Dialog.Description>
        Feature up to {limit} repositories at the top of the page, in the order shown. Changes save as you make them.
      </Dialog.Description>
    </Dialog.Header>

    <div class="grid min-w-0 gap-4">
      <div class="grid gap-2">
        <p class="text-xs font-medium text-muted-foreground">
          Pinned {selected.length}/{limit}
        </p>
        {#if pinned.length === 0}
          <p class="rounded-lg border border-dashed p-3 text-sm text-muted-foreground">
            Nothing pinned yet.
          </p>
        {:else}
          <ol class="grid gap-1.5">
            {#each pinned as repository, index (repository.id)}
              <li
                class="flex items-center gap-1 rounded-lg border bg-card py-1 pr-1 pl-3 text-sm"
              >
                <span class="min-w-0 flex-1 truncate">{repository.name}</span>
                <Button
                  size="icon-sm"
                  variant="ghost"
                  aria-label={`Move ${repository.name} up`}
                  disabled={index === 0}
                  onclick={() => move(index, -1)}
                >
                  <ArrowUp class="size-3.5" />
                </Button>
                <Button
                  size="icon-sm"
                  variant="ghost"
                  aria-label={`Move ${repository.name} down`}
                  disabled={index === pinned.length - 1}
                  onclick={() => move(index, 1)}
                >
                  <ArrowDown class="size-3.5" />
                </Button>
                <Button
                  size="icon-sm"
                  variant="ghost"
                  aria-label={`Unpin ${repository.name}`}
                  onclick={() =>
                    void update(selected.filter((id) => id !== repository.id))}
                >
                  <X class="size-3.5" />
                </Button>
              </li>
            {/each}
          </ol>
        {/if}
      </div>

      <div class="grid gap-2">
        <Input
          type="search"
          placeholder="Search repositories"
          aria-label="Search repositories"
          bind:value={query}
          onkeydown={(event) => {
            // Enter pins the top match, so a name can be typed and taken.
            if (event.key !== "Enter") return;
            event.preventDefault();
            const first = available[0];
            if (first && !full) {
              pin(first.id);
              query = "";
            }
          }}
        />
        <div class="max-h-56 overflow-y-auto rounded-lg border">
          {#if loading}
            <p class="p-3 text-sm text-muted-foreground">Loading…</p>
          {:else if loadError}
            <p class="p-3 text-sm text-destructive">{loadError}</p>
          {:else if available.length === 0}
            <p class="p-3 text-sm text-muted-foreground">
              {query.trim() ? "No matching repositories." : "Every repository is pinned."}
            </p>
          {:else}
            <ul class="divide-y">
              {#each available as repository (repository.id)}
                <li>
                  <button
                    type="button"
                    class="flex w-full items-center gap-2 py-2 pr-2.5 pl-3 text-left text-sm outline-none hover:bg-accent/55 focus-visible:bg-accent disabled:pointer-events-none disabled:opacity-50"
                    aria-label={`Pin ${repository.name}`}
                    disabled={full}
                    onclick={() => pin(repository.id)}
                  >
                    <span class="min-w-0 flex-1 truncate">{repository.name}</span>
                    <Plus class="size-3.5 shrink-0 text-muted-foreground" />
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      </div>
    </div>

  </Dialog.Content>
</Dialog.Root>
