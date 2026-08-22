<script lang="ts">
  import { Check, Pencil, Plus, X } from "lucide-svelte";

  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  const MAX_TOPICS = 25;

  let editing = $state(false);
  let entry = $state("");
  let entryField = $state<HTMLInputElement | null>(null);
  let suggestions = $state.raw<string[]>([]);

  // Each edit persists on its own, so an optimistic copy keeps the chips in
  // place while the request is in flight. Cleared once the newest save settles.
  let optimistic = $state.raw<string[] | null>(null);
  let queue: Promise<unknown> = Promise.resolve();

  const topics = $derived(optimistic ?? repository.topics);
  const available = $derived(
    suggestions.filter((topic) => !topics.includes(topic)),
  );
  const full = $derived(topics.length >= MAX_TOPICS);

  /** Mirrors the server's normalization so the badge preview matches what is stored. */
  function normalize(value: string): string {
    return value.trim().toLowerCase();
  }

  function commit(next: string[]): void {
    optimistic = next;
    queue = queue
      .then(() => repository.saveTopics(next))
      .catch(() => {
        // saveTopics surfaces the message; fall back to the server's list.
      })
      .finally(() => {
        // Only the most recent edit may hand rendering back to server state.
        if (optimistic === next) optimistic = null;
      });
  }

  function addTopic(value: string): void {
    const topic = normalize(value);
    entry = "";
    if (!topic || topics.includes(topic) || full) return;
    commit([...topics, topic].sort());
  }

  function removeTopic(topic: string): void {
    commit(topics.filter((existing) => existing !== topic));
  }

  function handleKeydown(event: KeyboardEvent): void {
    if (event.key === "Enter" || event.key === "," || event.key === " ") {
      event.preventDefault();
      addTopic(entry);
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      editing = false;
      entry = "";
      return;
    }
    if (event.key === "Backspace" && !entry && topics.length) {
      removeTopic(topics[topics.length - 1]);
    }
  }

  function startEditing(): void {
    entry = "";
    suggestions = [];
    editing = true;
  }

  function finishEditing(): void {
    addTopic(entry);
    editing = false;
    suggestions = [];
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
    {#if repository.repository?.can_manage}
      {#if editing}
        <Button
          variant="ghost"
          size="icon-xs"
          class="text-muted-foreground"
          aria-label="Done editing topics"
          onclick={finishEditing}
        >
          <Check class="size-3.5" />
        </Button>
      {:else}
        <Button
          variant="ghost"
          size="icon-xs"
          class="text-muted-foreground"
          aria-label={topics.length ? "Edit topics" : "Add topics"}
          onclick={startEditing}
        >
          {#if topics.length}
            <Pencil class="size-3.5" />
          {:else}
            <Plus class="size-3.5" />
          {/if}
        </Button>
      {/if}
    {/if}
  </div>

  {#if editing}
    <div class="mt-3">
      {#if topics.length}
        <div class="mb-2 flex flex-wrap gap-1.5">
          {#each topics as topic (topic)}
            <Badge variant="secondary" class="border-border pr-1">
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
                class="cursor-pointer border-dashed text-muted-foreground hover:bg-muted hover:text-foreground"
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
    </div>
  {:else if topics.length}
    <div class="mt-3 flex flex-wrap gap-1.5">
      {#each topics as topic (topic)}
        <Badge variant="secondary" class="border-border">{topic}</Badge>
      {/each}
    </div>
  {:else}
    <p class="mt-3 text-sm leading-6 text-muted-foreground">No topics yet.</p>
  {/if}
</div>
