<script lang="ts">
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Plus from "@lucide/svelte/icons/plus";
  import Tag from "@lucide/svelte/icons/tag";
  import Trash2 from "@lucide/svelte/icons/trash-2";

  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import LabelChip from "$lib/components/repository/label-chip.svelte";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let {
    open = $bindable(false),
    state: repository,
    mode = "manage",
    selected = $bindable([]),
  }: {
    open?: boolean;
    state: RepositoryPageState;
    /** "pick" toggles labels for an issue; "manage" only lists/creates/deletes. */
    mode?: "pick" | "manage";
    selected?: string[];
  } = $props();

  let creating = $state(false);
  let name = $state("");
  let color = $state("#3b82f6");
  let description = $state("");
  let failure = $state("");
  let deleteDialogOpen = $state(false);
  let pendingDeleteLabel = $state<{ id: string; name: string } | null>(null);

  const presets = [
    "#3b82f6",
    "#06b6d4",
    "#22c55e",
    "#eab308",
    "#f97316",
    "#ef4444",
    "#a855f7",
    "#ec4899",
  ];

  const selecting = $derived(mode === "pick");

  function toggle(id: string) {
    if (!selecting) return;
    selected = selected.includes(id)
      ? selected.filter((value) => value !== id)
      : [...selected, id];
  }

  async function submitLabel(event: SubmitEvent) {
    event.preventDefault();
    failure = "";
    creating = true;
    try {
      await repository.issues.createIssueLabel({
        name,
        color: color.replace("#", ""),
        description,
      });
      name = "";
      description = "";
    } catch (caught) {
      failure =
        caught instanceof Error
          ? caught.message
          : "Could not create the label.";
    } finally {
      creating = false;
    }
  }

  function requestRemoveLabel(id: string, labelName: string) {
    pendingDeleteLabel = { id, name: labelName };
    deleteDialogOpen = true;
  }

  function confirmRemoveLabel() {
    const target = pendingDeleteLabel;
    if (!target) return;
    deleteDialogOpen = false;
    pendingDeleteLabel = null;
    void repository.issues.deleteIssueLabel(target.id).catch(() => undefined);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title class="flex items-center gap-2">
        <Tag class="size-4" />Labels
      </Dialog.Title>
      <Dialog.Description>
        {selecting
          ? "Choose the labels that describe this issue."
          : "Organize issues with colored, deletable labels."}
      </Dialog.Description>
    </Dialog.Header>

    <div class="max-h-72 overflow-y-auto rounded-md border">
      <ul class="divide-y">
        {#each repository.issues.issueLabels as label (label.id)}
          <li class="flex items-center gap-3 p-2.5">
            {#if selecting}
              <input
                type="checkbox"
                class="accent-primary size-4"
                checked={selected.includes(label.id)}
                onchange={() => toggle(label.id)}
                aria-label={`Toggle ${label.name}`}
              />
            {/if}
            <button
              type="button"
              class="flex min-w-0 flex-1 items-center gap-3 text-left"
              onclick={() => toggle(label.id)}
              disabled={!selecting}
            >
              <LabelChip {label} />
              <span class="min-w-0 truncate text-xs text-muted-foreground"
                >{label.description || "No description"}</span
              >
            </button>
            {#if repository.repository?.can_manage}
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                class="text-muted-foreground hover:text-destructive"
                aria-label={`Delete ${label.name}`}
                disabled={repository.issues.labelPending}
                onclick={() => requestRemoveLabel(label.id, label.name)}
              >
                <Trash2 class="size-3" />
              </Button>
            {/if}
          </li>
        {:else}
          <li class="p-4 text-sm text-muted-foreground">No labels yet.</li>
        {/each}
      </ul>
    </div>

    {#if repository.repository?.can_manage}
      <form class="grid gap-3 border-t pt-4" onsubmit={submitLabel}>
        <div
          class="grid grid-cols-[auto_5.5rem_minmax(0,1fr)] items-center gap-2"
        >
          <input
            type="color"
            bind:value={color}
            aria-label="Label color"
            class="size-9 cursor-pointer rounded-md border bg-transparent p-1"
          />
          <Input
            bind:value={color}
            aria-label="Label color hex value"
            maxlength={7}
            class="font-mono text-xs uppercase"
          />
          <Input
            bind:value={name}
            maxlength={64}
            placeholder="Label name"
            required
          />
        </div>
        <div class="flex flex-wrap items-center gap-1.5">
          {#each presets as preset (preset)}
            <button
              type="button"
              aria-label={`Use color ${preset}`}
              class="size-5 rounded-full border transition-transform hover:scale-110 {color.toLowerCase() ===
              preset
                ? 'ring-ring ring-2 ring-offset-2 ring-offset-background'
                : ''}"
              style:background-color={preset}
              onclick={() => (color = preset)}
            ></button>
          {/each}
        </div>
        <Input
          bind:value={description}
          maxlength={255}
          placeholder="Description (optional)"
        />
        {#if failure}
          <p class="text-sm text-destructive">{failure}</p>
        {/if}
        <Button
          type="submit"
          size="sm"
          variant="outline"
          class="justify-self-start gap-2"
          disabled={creating || repository.issues.labelPending}
        >
          {#if creating || repository.issues.labelPending}
            <LoaderCircle class="size-3.5 animate-spin" />
          {:else}
            <Plus class="size-3.5" />
          {/if}
          Create label
        </Button>
      </form>
    {/if}

    <Dialog.Footer>
      <Dialog.Close>
        <Button type="button" variant="secondary">Done</Button>
      </Dialog.Close>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={deleteDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete label "{pendingDeleteLabel?.name ?? ""}"?</AlertDialog.Title>
      <AlertDialog.Description>This will remove it from every issue. This cannot be undone.</AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={confirmRemoveLabel}>Delete label</AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
