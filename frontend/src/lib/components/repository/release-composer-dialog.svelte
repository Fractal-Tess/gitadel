<script lang="ts">
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  import type { Release } from "$lib/api/releases.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import MarkdownEditor from "$lib/components/repository/markdown-editor.svelte";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let {
    open = $bindable(false),
    state: repository,
    release = null,
  }: {
    open?: boolean;
    state: RepositoryPageState;
    /** When set, the dialog edits this release instead of publishing a new one. */
    release?: Release | null;
  } = $props();

  let targetRevision = $state("");
  let title = $state("");
  let body = $state("");
  let prerelease = $state(false);
  let files = $state<File[]>([]);
  let failure = $state("");

  const targetSuggestions = $derived([
    ...(repository.browser.refs?.tags.map((tag) => tag.name) ?? []),
    ...(repository.browser.refs?.branches.map((branch) => branch.name) ?? []),
  ]);

  // Prefill the draft each time the dialog opens.
  $effect(() => {
    if (!open) return;
    targetRevision = release?.target_revision ?? repository.revision;
    title = release?.title ?? "";
    body = release?.body ?? "";
    prerelease = release?.prerelease ?? false;
    files = [];
    failure = "";
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    failure = "";
    try {
      if (release) {
        await repository.releases.updateRelease(release.id, {
          target_revision: targetRevision,
          title,
          body,
          prerelease,
        });
      } else {
        await repository.releases.createRelease(
          {
            target_revision: targetRevision,
            title,
            body,
            prerelease,
          },
          files,
        );
      }
      open = false;
    } catch (caught) {
      failure =
        caught instanceof Error ? caught.message : "The request failed.";
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    class="ring-foreground/20 sm:max-w-2xl max-h-[85vh] overflow-y-auto"
  >
    <Dialog.Header>
      <Dialog.Title>
        {release ? `Edit release “${release.title}”` : "New release"}
      </Dialog.Title>
      <Dialog.Description>
        {release
          ? "Update the target, title, or release notes."
          : "Pair a commit with Markdown release notes and downloadable assets."}
      </Dialog.Description>
    </Dialog.Header>

    <form class="grid gap-4" onsubmit={submit}>
      <div class="grid gap-4 sm:grid-cols-2">
        <Field.Field>
          <Field.Label for="release-dialog-target">Release target</Field.Label>
          <Input
            id="release-dialog-target"
            bind:value={targetRevision}
            list="release-dialog-targets"
            maxlength={255}
            placeholder="Tag, branch, or commit SHA"
            required
          />
          <datalist id="release-dialog-targets">
            {#each targetSuggestions as target (target)}
              <option value={target}></option>
            {/each}
          </datalist>
          <Field.Description>
            Resolves to a commit now; no tag is required.
          </Field.Description>
        </Field.Field>
        <Field.Field>
          <Field.Label for="release-dialog-title">Release title</Field.Label>
          <Input
            id="release-dialog-title"
            bind:value={title}
            maxlength={255}
            placeholder="What changed?"
            required
          />
        </Field.Field>
      </div>

      <Field.Field>
        <Field.Label for="release-dialog-notes">Release notes</Field.Label>
        <MarkdownEditor
          state={repository}
          bind:value={body}
          id="release-dialog-notes"
          class="min-h-48"
          placeholder="Describe the changes in Markdown…"
        />
      </Field.Field>

      {#if !release}
        <Field.Field>
          <Field.Label for="release-dialog-assets">Assets</Field.Label>
          <Input
            id="release-dialog-assets"
            type="file"
            multiple
            onchange={(event) => {
              files = Array.from(event.currentTarget.files ?? []);
            }}
          />
          <Field.Description>
            Attach binaries, packages, checksums, or other release files.
          </Field.Description>
        </Field.Field>
      {/if}

      <label class="flex w-fit items-center gap-2 text-sm">
        <Checkbox bind:checked={prerelease} />
        Mark as a pre-release
      </label>

      {#if failure}
        <p class="text-sm text-destructive">{failure}</p>
      {/if}

      <Dialog.Footer class="mt-2 gap-2">
        <Button type="button" variant="ghost" onclick={() => (open = false)}
          >Cancel</Button
        >
        <Button
          type="submit"
          disabled={repository.releases.releasePending || repository.releases.releaseAssetPending}
        >
          {#if repository.releases.releasePending || repository.releases.releaseAssetPending}
            <LoaderCircle class="size-4 animate-spin" />
          {/if}
          {repository.releases.releaseAssetPending
            ? "Uploading assets…"
            : repository.releases.releasePending
              ? "Publishing…"
              : release
                ? "Save release"
                : "Publish release"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
