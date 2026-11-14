<!--
  Edits one text file in the browser and commits it to the branch it was
  opened on. The draft lives in the page state, so leaving the file by any
  route asks before unsaved changes are thrown away.
-->
<script lang="ts">
  import GitCommitHorizontal from "@lucide/svelte/icons/git-commit-horizontal";
  import Pencil from "@lucide/svelte/icons/pencil";
  import { toast } from "svelte-sonner";

  import { errorMessage } from "$lib/api/transport.js";
  import MaterialFileIcon from "$lib/components/repository/material-file-icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { highlight } from "$lib/repository/syntax-highlight.js";

  let { state: pageState }: { state: RepositoryPageState } = $props();

  const edit = $derived(pageState.editing!);
  const fileName = $derived(edit.path.split("/").at(-1) ?? edit.path);
  const changed = $derived(pageState.hasUnsavedEdit);
  const lineCount = $derived(pageState.draft.split("\n").length);
  // Tab indents the way the file already does: tabs if any line starts with
  // one, otherwise two spaces.
  const indent = $derived(/^\t/m.test(edit.original) ? "\t" : "  ");
  // Match the viewer's size limit so highlighting cannot block large edits.
  const highlighted = $derived.by(() => {
    if (pageState.draft.length > 200_000) return null;
    try {
      return highlight(edit.path, pageState.draft);
    } catch {
      return null;
    }
  });

  let commitOpen = $state(false);
  let message = $state("");
  let committing = $state(false);
  let textarea = $state<HTMLTextAreaElement | null>(null);
  let highlightLayer: HTMLPreElement;

  function syncScroll(event: Event): void {
    const target = event.currentTarget as HTMLTextAreaElement;
    highlightLayer.scrollLeft = target.scrollLeft;
    highlightLayer.scrollTop = target.scrollTop;
  }

  $effect(() => {
    textarea?.focus({ preventScroll: true });
  });

  // Closing or reloading the tab would lose the draft, so the browser asks.
  $effect(() => {
    if (!changed) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  });

  function onKeydown(event: KeyboardEvent): void {
    const target = event.currentTarget as HTMLTextAreaElement;
    if ((event.metaKey || event.ctrlKey) && event.key === "s") {
      event.preventDefault();
      if (changed) openCommit();
      return;
    }
    if (event.key !== "Tab" || event.altKey || event.metaKey || event.ctrlKey)
      return;
    event.preventDefault();
    const { selectionStart: start, selectionEnd: end, value } = target;
    if (event.shiftKey) {
      // Outdent the current line.
      const lineStart = value.lastIndexOf("\n", start - 1) + 1;
      const removable = value.startsWith(indent, lineStart)
        ? indent.length
        : value.startsWith("\t", lineStart)
          ? 1
          : 0;
      if (!removable) return;
      target.setRangeText("", lineStart, lineStart + removable, "preserve");
    } else {
      target.setRangeText(indent, start, end, "end");
    }
    pageState.draft = target.value;
  }

  function openCommit(): void {
    message = `Update ${fileName}`;
    commitOpen = true;
  }

  async function commit(): Promise<void> {
    if (!message.trim()) return;
    committing = true;
    try {
      await pageState.commitEdit(message.trim());
      commitOpen = false;
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      committing = false;
    }
  }
</script>

<header
  class="flex min-h-12 shrink-0 flex-wrap items-center justify-between gap-3 border-b px-5 py-2 text-sm font-semibold"
>
  <span class="flex min-w-0 items-center gap-2">
    <MaterialFileIcon name={edit.path} class="size-4 shrink-0" />
    <span class="truncate">{edit.path}</span>
    <span
      class="inline-flex shrink-0 items-center gap-1 rounded border border-amber-500/30 bg-amber-500/10 px-1.5 py-0.5 text-[10px] font-medium text-amber-700 dark:text-amber-300"
    >
      <Pencil class="size-2.5" />Editing on {edit.branch}
    </span>
    {#if changed}
      <span class="shrink-0 text-xs font-normal text-muted-foreground">
        Unsaved changes
      </span>
    {/if}
  </span>
  <div class="flex items-center gap-1.5">
    <Button
      variant="ghost"
      size="sm"
      class="text-muted-foreground"
      onclick={() => pageState.stopEditing()}>Cancel</Button
    >
    <Button size="sm" disabled={!changed} onclick={openCommit}>
      <GitCommitHorizontal data-icon="inline-start" />Commit changes
    </Button>
  </div>
</header>

<div class="flex min-h-0 flex-1 bg-background/35 xl:overflow-auto">
  <!-- Line numbers follow the draft's line count; wrapping is off so each
       number stays level with its line. -->
  <div
    class="shrink-0 select-none border-r py-5 pr-3 pl-4 text-right font-mono text-xs leading-5 text-muted-foreground/60"
    aria-hidden="true"
  >
    {#each { length: lineCount }, index (index)}
      <div>{index + 1}</div>
    {/each}
  </div>
  <div class="relative min-w-0 flex-1 self-start">
    <!-- The textarea owns editing, selection, IME and accessibility. This
         noninteractive copy supplies only the syntax colours underneath it. -->
    <pre
      bind:this={highlightLayer}
      class="editor-text pointer-events-none absolute inset-0 m-0 overflow-hidden p-5 text-foreground"
      aria-hidden="true"
      data-editor-highlight>{@html (highlighted ?? "") + "\n"}</pre>
    <textarea
      bind:this={textarea}
      bind:value={pageState.draft}
      class="editor-text relative block min-h-80 w-full resize-none overflow-x-auto overflow-y-hidden bg-transparent p-5 caret-foreground outline-none"
      class:highlighted={highlighted !== null}
      rows={lineCount}
      wrap="off"
      spellcheck="false"
      autocapitalize="off"
      autocomplete="off"
      aria-label={`Contents of ${edit.path}`}
      onkeydown={onKeydown}
      onscroll={syncScroll}></textarea>
  </div>
</div>

<Dialog.Root bind:open={commitOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>Commit changes</Dialog.Title>
      <Dialog.Description>
        Adds a commit to <span class="font-mono">{edit.branch}</span> that
        updates
        <span class="font-mono">{edit.path}</span>.
      </Dialog.Description>
    </Dialog.Header>
    <form
      class="grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void commit();
      }}
    >
      <Field.Field>
        <Field.Label for="edit-commit-message">Commit message</Field.Label>
        <Input
          id="edit-commit-message"
          bind:value={message}
          maxlength={255}
          required
          disabled={committing}
        />
        <Field.Description>
          If someone else pushed to {edit.branch} since you opened the file, the commit
          is refused so their work is not overwritten.
        </Field.Description>
      </Field.Field>
      <Dialog.Footer>
        <Dialog.Close>
          {#snippet child({ props })}
            <Button {...props} type="button" variant="outline">Cancel</Button>
          {/snippet}
        </Dialog.Close>
        <Button type="submit" disabled={committing || !message.trim()}>
          {committing ? "Committing…" : "Commit"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<style>
  .editor-text {
    font-family: var(--font-mono);
    font-size: 0.75rem;
    font-weight: 400;
    line-height: 1.25rem;
    letter-spacing: normal;
    white-space: pre;
    tab-size: 4;
    font-variant-ligatures: none;
  }

  textarea.highlighted {
    color: transparent;
  }

  textarea.highlighted::selection {
    background: color-mix(in srgb, var(--primary) 25%, transparent);
    color: transparent;
  }

  @media (forced-colors: active) {
    textarea.highlighted {
      color: CanvasText;
    }
    textarea.highlighted::selection {
      color: HighlightText;
      background: Highlight;
    }
    pre {
      visibility: hidden;
    }
  }
</style>
