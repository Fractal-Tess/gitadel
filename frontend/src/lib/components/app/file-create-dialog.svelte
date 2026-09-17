<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import FilePlus2 from "@lucide/svelte/icons/file-plus-2";
  import { z } from "zod";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import { toast } from "svelte-sonner";

  import { refsSchema, repositorySchema } from "$lib/api/repositories.js";
  import { jsonBody, requestJson } from "$lib/api/transport.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  const createFileResponseSchema = z.object({
    path: z.string(),
    branch: z.string(),
    commit: z.string(),
  });

  const shell = useShellState();
  let mode = $state<"file" | "directory">("file");
  let path = $state("");
  let filename = $state(".gitkeep");
  let content = $state("");
  let message = $state("");
  let branch = $state("");
  let expectedCommit = $state<string | null>(null);
  let loading = $state(false);
  let saving = $state(false);
  let confirmed = $state(false);
  let wasOpen = false;

  const repository = $derived(shell.activeRepository);
  const targetPath = $derived(
    mode === "directory"
      ? [path.trim(), filename.trim()].filter(Boolean).join("/")
      : path.trim(),
  );

  $effect(() => {
    const open = shell.fileCreateOpen;
    if (!open) {
      wasOpen = false;
      return;
    }
    if (wasOpen) return;
    wasOpen = true;
    mode = shell.fileCreateMode;
    shell.fileCreateMode = "file";
    path = "";
    filename = ".gitkeep";
    content =
      mode === "directory" ? "This file keeps the directory in Git.\n" : "";
    message = mode === "directory" ? "Create directory" : "Create file";
    branch = "";
    expectedCommit = null;
    confirmed = false;
    void loadBranch();
  });

  async function loadBranch(): Promise<void> {
    const current = repository;
    if (!current) return;
    loading = true;
    try {
      const api = `/api/v1/repositories/${encodeURIComponent(current.namespace)}/${encodeURIComponent(current.name)}`;
      const [detail, refs] = await Promise.all([
        requestJson(api, repositorySchema),
        requestJson(`${api}/refs`, refsSchema),
      ]);
      if (
        shell.activeRepository?.namespace !== current.namespace ||
        shell.activeRepository?.name !== current.name
      ) {
        return;
      }
      branch = detail.default_branch ?? refs.branches[0]?.name ?? "main";
      expectedCommit =
        refs.branches.find((candidate) => candidate.name === branch)
          ?.commit_oid ?? null;
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : "Could not load branches.",
      );
    } finally {
      loading = false;
    }
  }

  async function createFile(): Promise<void> {
    const current = repository;
    if (!current || !targetPath || !branch || !message.trim() || !confirmed)
      return;
    saving = true;
    try {
      await requestJson(
        `/api/v1/repositories/${encodeURIComponent(current.namespace)}/${encodeURIComponent(current.name)}/files`,
        createFileResponseSchema,
        {
          method: "POST",
          body: jsonBody({
            branch,
            expected_commit: expectedCommit,
            path: targetPath,
            content,
            message: message.trim(),
          }),
        },
      );
      shell.fileCreateOpen = false;
      toast.success(`${targetPath} committed to ${branch}.`);
      await goto(
        `${resolve("/[namespace]/[name]", {
          namespace: current.namespace,
          name: current.name,
        })}?view=overview&path=${encodeURIComponent(targetPath)}&rev=${encodeURIComponent(branch)}`,
      );
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : "Could not create file.",
      );
    } finally {
      saving = false;
    }
  }
</script>

<Dialog.Root bind:open={shell.fileCreateOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title class="flex items-center gap-2">
        {#if mode === "directory"}<FolderPlus class="size-4" />New directory{:else}<FilePlus2
            class="size-4"
          />New file{/if}
      </Dialog.Title>
      <Dialog.Description>
        {#if mode === "directory"}
          Git cannot store an empty directory. This creates the tracked file you
          name below; review its content and commit before saving.
        {:else}
          Add a tracked file, then review and commit it to the selected branch.
        {/if}
      </Dialog.Description>
    </Dialog.Header>

    {#if loading}
      <p class="py-8 text-center text-sm text-muted-foreground">
        Loading branch…
      </p>
    {:else}
      <form
        class="grid gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void createFile();
        }}
      >
        {#if mode === "directory"}
          <Field.Field>
            <Field.Label for="create-file-path">Directory path</Field.Label>
            <Input
              id="create-file-path"
              bind:value={path}
              placeholder="docs/guides"
              required
            />
          </Field.Field>
          <Field.Field>
            <Field.Label for="create-file-name">Tracked filename</Field.Label>
            <Input
              id="create-file-name"
              bind:value={filename}
              placeholder=".gitkeep"
              required
            />
            <Field.Description
              >This is the explicit file Git will track inside the directory.</Field.Description
            >
          </Field.Field>
        {:else}
          <Field.Field>
            <Field.Label for="create-file-path">File path</Field.Label>
            <Input
              id="create-file-path"
              bind:value={path}
              placeholder="docs/README.md"
              required
            />
          </Field.Field>
        {/if}
        <Field.Field>
          <Field.Label for="create-file-content">Content</Field.Label>
          <Textarea
            id="create-file-content"
            bind:value={content}
            class="min-h-40 font-mono text-sm"
            placeholder={mode === "directory"
              ? "This file keeps the directory in Git."
              : "File contents"}
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="create-file-message">Commit message</Field.Label>
          <Input
            id="create-file-message"
            bind:value={message}
            maxlength={255}
            required
          />
        </Field.Field>
        <div class="rounded-lg border bg-muted/20 p-3 text-sm">
          <p class="font-medium">Commit to <code>{branch || "…"}</code></p>
          <p class="mt-1 text-xs leading-5 text-muted-foreground">
            The branch is checked again when saving. If it changed, nothing is
            committed and you can review the latest state.
          </p>
        </div>
        <label class="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            bind:checked={confirmed}
            class="mt-1 size-4 rounded border-input accent-primary"
          />
          <span
            >I reviewed the path, content, and commit message and want to create
            this tracked file.</span
          >
        </label>
        <Dialog.Footer>
          <Button
            type="button"
            variant="outline"
            onclick={() => (shell.fileCreateOpen = false)}>Cancel</Button
          >
          <Button
            type="submit"
            disabled={saving || !branch || !targetPath || !confirmed}
            >{saving ? "Committing…" : "Commit file"}</Button
          >
        </Dialog.Footer>
      </form>
    {/if}
  </Dialog.Content>
</Dialog.Root>
