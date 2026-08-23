<script lang="ts">
  import { LoaderCircle, Paperclip } from "lucide-svelte";

  import type { IssueAttachment } from "$lib/api.js";
  import * as Tabs from "$lib/components/ui/tabs/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { trustedHtml } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let {
    state: repository,
    value = $bindable(""),
    id,
    placeholder = "",
    class: className = "min-h-64",
    maxlength = 1_000_000,
    attachments = false,
    onattachment,
  }: {
    state: RepositoryPageState;
    value?: string;
    id?: string;
    placeholder?: string;
    class?: string;
    maxlength?: number;
    attachments?: boolean;
    onattachment?: (attachment: IssueAttachment) => void;
  } = $props();

  let mode = $state<"write" | "preview">("write");
  let previewHtml = $state("");
  let previewPending = $state(false);
  let pendingUploads = $state(0);
  let textarea = $state<HTMLTextAreaElement | null>(null);

  async function switchMode(next: string) {
    if (next === mode) return;
    if (next !== "preview") {
      mode = "write";
      return;
    }
    previewPending = true;
    try {
      previewHtml = await repository.previewMarkdown(value);
      mode = "preview";
    } catch {
      // The page-level error explains the failure; the draft stays untouched.
    } finally {
      previewPending = false;
    }
  }

  function handleFiles(files: File[]) {
    if (!attachments) return;
    for (const file of files) void uploadFile(file);
  }

  async function uploadFile(file: File) {
    pendingUploads += 1;
    try {
      const attachment = await repository.uploadIssueAttachment(file);
      insertAtCursor(attachmentMarkdown(attachment));
      onattachment?.(attachment);
    } catch {
      // The page-level error explains the failure; the draft stays untouched.
    } finally {
      pendingUploads -= 1;
    }
  }

  function attachmentMarkdown(attachment: IssueAttachment) {
    const label = attachment.name.replaceAll(/[[\]]/g, "");
    const url = attachment.url;
    return attachment.content_type.startsWith("image/")
      ? `![${label}](${url})`
      : `[${label}](${url})`;
  }

  function insertAtCursor(snippet: string) {
    const node = textarea;
    const start = node?.selectionStart ?? value.length;
    const end = node?.selectionEnd ?? value.length;
    value = `${value.slice(0, start)}${snippet}${value.slice(end)}`;
    requestAnimationFrame(() => {
      node?.focus();
      node?.setSelectionRange(start + snippet.length, start + snippet.length);
    });
  }

  function onPaste(event: ClipboardEvent) {
    handleFiles(Array.from(event.clipboardData?.files ?? []));
  }

  function onDrop(event: DragEvent) {
    if (!attachments) return;
    event.preventDefault();
    handleFiles(Array.from(event.dataTransfer?.files ?? []));
  }

  function onDragOver(event: DragEvent) {
    if (attachments) event.preventDefault();
  }
</script>

<div class="overflow-hidden rounded-md border">
  <div
    class="flex flex-wrap items-center justify-between gap-2 border-b bg-muted/15 px-2 py-1"
  >
    <Tabs.Root value={mode} onValueChange={switchMode}>
      <Tabs.List class="h-7 gap-1 bg-transparent p-0">
        <Tabs.Trigger
          value="write"
          class="h-7 rounded-sm px-2.5 text-xs data-[state=active]:bg-background data-[state=active]:shadow-none"
        >
          Write
        </Tabs.Trigger>
        <Tabs.Trigger
          value="preview"
          class="h-7 rounded-sm px-2.5 text-xs data-[state=active]:bg-background data-[state=active]:shadow-none"
        >
          {#if previewPending}
            <LoaderCircle class="size-3 animate-spin" />
          {/if}
          Preview
        </Tabs.Trigger>
      </Tabs.List>
    </Tabs.Root>
    <span
      class="flex items-center gap-1.5 pr-1 text-xs text-muted-foreground"
      aria-live="polite"
    >
      {#if pendingUploads > 0}
        <LoaderCircle class="size-3 animate-spin" />
        Uploading {pendingUploads}
        {pendingUploads === 1 ? "file" : "files"}…
      {:else if attachments}
        <Paperclip class="size-3" />
        Paste or drop files to attach
      {:else}
        Markdown supported
      {/if}
    </span>
  </div>
  {#if mode === "write"}
    <Textarea
      bind:ref={textarea}
      bind:value
      {id}
      {placeholder}
      {maxlength}
      class="rounded-none border-0 font-mono text-sm focus-visible:ring-0 {className}"
      onpaste={onPaste}
      ondrop={onDrop}
      ondragover={onDragOver}
    />
  {:else}
    <div class="p-4 {className}">
      {#if previewHtml}
        <div
          class="prose prose-invert max-w-none text-sm prose-code:before:content-none prose-code:after:content-none"
          {@attach trustedHtml(previewHtml)}
        ></div>
      {:else}
        <p class="text-sm italic text-muted-foreground">
          Nothing to preview yet.
        </p>
      {/if}
    </div>
  {/if}
</div>
