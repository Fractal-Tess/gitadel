<!--
  Edits one text file in the browser and commits it to the branch it was
  opened on. The draft lives in the page state, so leaving the file by any
  route asks before unsaved changes are thrown away.
-->
<script lang="ts">
  import GitCommitHorizontal from "@lucide/svelte/icons/git-commit-horizontal";
  import Pencil from "@lucide/svelte/icons/pencil";
  import { toast } from "svelte-sonner";

  import MaterialFileIcon from "$lib/components/repository/material-file-icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { errorMessage } from "$lib/repository/state/shared.js";

  let { state: pageState }: { state: RepositoryPageState } = $props();

  const edit = $derived(pageState.editing!);
  const fileName = $derived(edit.path.split("/").at(-1) ?? edit.path);
  const changed = $derived(pageState.hasUnsavedEdit);
  const lineCount = $derived(pageState.draft.split("\n").length);
  // Tab indents the way the file already does: tabs if any line starts with
  // one, otherwise two spaces.
  const indent = $derived(/^\t/m.test(edit.original) ? "\t" : "  ");

  let commitOpen = $state(false);
  let message = $state("");
  let committing = $state(false);
  let textarea = $state<HTMLTextAreaElement | null>(null);

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
  <textarea
    bind:this={textarea}
    bind:value={pageState.draft}
    class="min-h-80 min-w-0 flex-1 resize-none overflow-x-auto overflow-y-hidden bg-transparent p-5 font-mono text-xs leading-5 whitespace-pre outline-none"
    rows={lineCount}
    wrap="off"
    spellcheck="false"
    autocapitalize="off"
    autocomplete="off"
    aria-label={`Contents of ${edit.path}`}
    onkeydown={onKeydown}
  ></textarea>
</div>

<Dialog.Root bind:open={commitOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>Commit changes</Dialog.Title>
      <Dialog.Description>
        Adds a commit to <span class="font-mono">{edit.branch}</span> that updates
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
          If someone else pushed to {edit.branch} since you opened the file, the
          commit is refused so their work is not overwritten.
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
