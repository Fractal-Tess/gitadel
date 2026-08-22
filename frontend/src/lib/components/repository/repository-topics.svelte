<script lang="ts">
  import { Pencil, Plus, X } from "lucide-svelte";

  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  const MAX_TOPICS = 25;

  let editing = $state(false);
  let draft = $state.raw<string[]>([]);
  let entry = $state("");
  let entryField = $state<HTMLInputElement | null>(null);
  let suggestions = $state.raw<string[]>([]);

  const available = $derived(
    suggestions.filter((topic) => !draft.includes(topic)),
  );
  const full = $derived(draft.length >= MAX_TOPICS);

  function startEditing(): void {
    draft = [...repository.topics];
    entry = "";
    suggestions = [];
    editing = true;
  }

  function cancelEditing(): void {
    editing = false;
    draft = [];
    entry = "";
    suggestions = [];
  }

  /** Mirrors the server's normalization so the badge preview matches what is stored. */
  function normalize(value: string): string {
    return value.trim().toLowerCase();
  }

  function addTopic(value: string): void {
    const topic = normalize(value);
    if (!topic || draft.includes(topic) || full) return;
    draft = [...draft, topic].sort();
    entry = "";
  }

  function removeTopic(topic: string): void {
    draft = draft.filter((existing) => existing !== topic);
  }

  function handleKeydown(event: KeyboardEvent): void {
    if (event.key === "Enter" || event.key === "," || event.key === " ") {
      event.preventDefault();
      addTopic(entry);
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      cancelEditing();
      return;
    }
    if (event.key === "Backspace" && !entry && draft.length) {
      draft = draft.slice(0, -1);
    }
  }

  async function save(): Promise<void> {
    const pending = normalize(entry);
    const next = pending && !draft.includes(pending) ? [...draft, pending].sort() : draft;
    if (
      next.length === repository.topics.length &&
      next.every((topic, index) => topic === repository.topics[index])
    ) {
      cancelEditing();
      return;
    }
    try {
      await repository.saveTopics(next);
      cancelEditing();
    } catch {
      // saveTopics surfaces the message; keep the draft editable.
    }
  }

  $effect(() => {
    if (editing) entryField?.focus();
  });

  // Suggestions come from the server so they can span every repository the
  // viewer may see, which client-side filtering could not do.
  $effect(() => {
    if (!editing) return;
    const query = normalize(entry);
    const controller = new AbortController();
    const timer = setTimeout(() => {
      void repository
        .suggestTopics(query, { signal: controller.signal })
        .then((topics) => {
          suggestions = topics;
        })
        .catch(() => {});
    }, 150);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  });
</script>

<div class="mt-5">
  <div class="flex min-h-6 items-center justify-between gap-2">
    <h2
      class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
    >
      Topics
    </h2>
    {#if repository.repository?.can_manage && !editing}
      <Button
        variant="ghost"
        size="icon-xs"
        class="text-muted-foreground"
        aria-label={repository.topics.length ? "Edit topics" : "Add topics"}
        onclick={startEditing}
      >
        {#if repository.topics.length}
          <Pencil class="size-3.5" />
        {:else}
          <Plus class="size-3.5" />
        {/if}
      </Button>
    {/if}
  </div>

  {#if editing}
    <div class="mt-3">
      {#if draft.length}
        <div class="mb-2 flex flex-wrap gap-1.5">
          {#each draft as topic (topic)}
            <Badge variant="secondary" class="pr-1">
              {topic}
              <button
                type="button"
                class="rounded-full p-0.5 text-muted-foreground hover:text-foreground"
                aria-label={`Remove ${topic}`}
                onclick={() => removeTopic(topic)}
              >
                <X class="size-3" />
              </button>
            </Badge>
          {/each}
        </div>
      {/if}

      <Input
        bind:value={entry}
        bind:ref={entryField}
        class="h-8 text-sm"
        maxlength={35}
        disabled={full}
        placeholder={full ? "Topic limit reached" : "Add a topic"}
        aria-label="Add a topic"
        onkeydown={handleKeydown}
      />

      {#if available.length && !full}
        <ul class="mt-2 flex flex-wrap gap-1.5">
          {#each available.slice(0, 8) as topic (topic)}
            <li>
              <Badge
                variant="outline"
                class="cursor-pointer hover:bg-muted"
                role="button"
                tabindex={0}
                onclick={() => addTopic(topic)}
                onkeydown={(event: KeyboardEvent) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    addTopic(topic);
                  }
                }}
              >
                <Plus class="size-3" />{topic}
              </Badge>
            </li>
          {/each}
        </ul>
      {/if}

      <div class="mt-2 flex items-center justify-end gap-2">
        <Button type="button" variant="ghost" size="sm" onclick={cancelEditing}>
          Cancel
        </Button>
        <Button size="sm" disabled={repository.topicsPending} onclick={() => void save()}>
          {repository.topicsPending ? "Saving…" : "Save"}
        </Button>
      </div>
    </div>
  {:else if repository.topics.length}
    <div class="mt-3 flex flex-wrap gap-1.5">
      {#each repository.topics as topic (topic)}
        <Badge variant="secondary">{topic}</Badge>
      {/each}
    </div>
  {:else}
    <p class="mt-3 text-sm leading-6 text-muted-foreground">No topics yet.</p>
  {/if}
</div>
