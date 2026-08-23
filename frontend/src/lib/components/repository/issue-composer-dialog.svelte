<script lang="ts">
  import { LoaderCircle, Tag } from "lucide-svelte";

  import type { Issue, IssueAttachment } from "$lib/api.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import AssigneeCombobox from "$lib/components/repository/assignee-combobox.svelte";
  import IssueLabelDialog from "$lib/components/repository/issue-label-dialog.svelte";
  import MarkdownEditor from "$lib/components/repository/markdown-editor.svelte";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let {
    open = $bindable(false),
    state: repository,
    issue = null,
  }: {
    open?: boolean;
    state: RepositoryPageState;
    /** When set, the dialog edits this issue instead of composing a new one. */
    issue?: Issue | null;
  } = $props();

  let title = $state("");
  let body = $state("");
  let assignee = $state("");
  let selectedLabelIds = $state<string[]>([]);
  let pendingAttachmentIds = $state<string[]>([]);
  let labelsOpen = $state(false);
  let failure = $state("");

  const canManage = $derived(
    issue?.can_manage ?? repository.repository?.can_manage ?? false,
  );

  // Prefill the draft each time the dialog opens.
  $effect(() => {
    if (!open) return;
    title = issue?.title ?? "";
    body = issue?.body ?? "";
    assignee = issue?.assignee?.username ?? "";
    selectedLabelIds = issue ? issue.labels.map((label) => label.id) : [];
    pendingAttachmentIds = [];
    failure = "";
  });

  $effect(() => {
    if (
      open &&
      canManage &&
      repository.authStatus?.authenticated &&
      !repository.assignableUsers.length
    ) {
      void repository.loadAssignableUsers().catch(() => undefined);
    }
  });

  function onAttachment(attachment: IssueAttachment) {
    pendingAttachmentIds = [...pendingAttachmentIds, attachment.id];
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    failure = "";
    try {
      if (issue) {
        await repository.updateIssue(issue.number, {
          title,
          body,
          ...(issue.can_manage && {
            label_ids: selectedLabelIds,
            assignee,
          }),
          ...(pendingAttachmentIds.length && {
            attachment_ids: pendingAttachmentIds,
          }),
        });
      } else {
        await repository.createIssue({
          title,
          body,
          label_ids: canManage ? selectedLabelIds : [],
          ...(canManage && assignee.trim() && { assignee: assignee.trim() }),
          ...(pendingAttachmentIds.length && {
            attachment_ids: pendingAttachmentIds,
          }),
        });
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
      <Dialog.Title
        >{issue ? `Edit issue #${issue.number}` : "New issue"}</Dialog.Title
      >
      <Dialog.Description>
        {issue
          ? "Update the title, description, labels, or assignee."
          : "Describe the problem or idea in Markdown; attachments are welcome."}
      </Dialog.Description>
    </Dialog.Header>

    <form class="grid gap-4" onsubmit={submit}>
      <Field.Field>
        <Field.Label for="issue-dialog-title">Title</Field.Label>
        <Input
          id="issue-dialog-title"
          bind:value={title}
          maxlength={255}
          placeholder="Describe the problem or idea"
          required
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="issue-dialog-body">Description</Field.Label>
        <MarkdownEditor
          state={repository}
          bind:value={body}
          id="issue-dialog-body"
          class="min-h-52"
          placeholder="Add context, steps to reproduce, or acceptance criteria in Markdown…"
          attachments
          onattachment={onAttachment}
        />
      </Field.Field>

      {#if canManage}
        <div class="flex flex-wrap items-end gap-4">
          <Field.Field class="min-w-48 flex-1">
            <Field.Label>Assignee</Field.Label>
            <AssigneeCombobox
              users={repository.assignableUsers}
              bind:value={assignee}
            />
          </Field.Field>
          <Field.Field>
            <Field.Label>Labels</Field.Label>
            <Button
              type="button"
              variant="outline"
              class="gap-2"
              onclick={() => (labelsOpen = true)}
            >
              <Tag class="size-3.5" />
              {selectedLabelIds.length
                ? `${selectedLabelIds.length} selected`
                : "Choose labels"}
            </Button>
          </Field.Field>
        </div>
      {/if}

      {#if failure}
        <p class="text-sm text-destructive">{failure}</p>
      {/if}

      <Dialog.Footer class="mt-2 gap-2">
        <Button type="button" variant="ghost" onclick={() => (open = false)}
          >Cancel</Button
        >
        <Button type="submit" disabled={repository.issuePending}>
          {#if repository.issuePending}
            <LoaderCircle class="size-4 animate-spin" />
          {/if}
          {repository.issuePending
            ? "Saving…"
            : issue
              ? "Save changes"
              : "Open issue"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<IssueLabelDialog
  bind:open={labelsOpen}
  state={repository}
  mode={canManage ? "pick" : "manage"}
  bind:selected={selectedLabelIds}
/>
