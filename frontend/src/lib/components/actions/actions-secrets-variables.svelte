<script lang="ts">
  import KeyRound from "@lucide/svelte/icons/key-round";
  import Pencil from "@lucide/svelte/icons/pencil";
  import Plus from "@lucide/svelte/icons/plus";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import Variable from "@lucide/svelte/icons/variable";

  import { actionValueNameError } from "$lib/api/actions.js";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import {
    ActionsSecretsVariablesState,
    type ActionValueKind,
  } from "$lib/settings/actions/secrets-variables-state.svelte.js";

  let {
    base,
    description,
  }: {
    /** Actions API prefix of the scope, without a trailing slash. */
    base: string;
    /** Explains which workflows receive these values. */
    description: string;
  } = $props();

  const values = $derived(new ActionsSecretsVariablesState(base));

  $effect(() => {
    void values.load();
  });

  let editorOpen = $state(false);
  let editorKind = $state<ActionValueKind>("secrets");
  let editorExisting = $state(false);
  let editorName = $state("");
  let editorValue = $state("");
  let editorTouched = $state(false);
  const nameError = $derived(
    editorTouched ? actionValueNameError(editorName) : null,
  );

  let deleteOpen = $state(false);
  let pendingDelete = $state<{ kind: ActionValueKind; name: string } | null>(
    null,
  );

  const dateFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
  });

  function openEditor(
    kind: ActionValueKind,
    name = "",
    value = "",
  ): void {
    editorKind = kind;
    editorExisting = name !== "";
    editorName = name;
    editorValue = value;
    editorTouched = false;
    editorOpen = true;
  }

  async function saveEditor(): Promise<void> {
    editorTouched = true;
    if (actionValueNameError(editorName)) return;
    if (await values.save(editorKind, editorName, editorValue)) {
      editorOpen = false;
      editorValue = "";
    }
  }

  function requestDelete(kind: ActionValueKind, name: string): void {
    pendingDelete = { kind, name };
    deleteOpen = true;
  }

  async function confirmDelete(): Promise<void> {
    if (!pendingDelete) return;
    await values.remove(pendingDelete.kind, pendingDelete.name);
    deleteOpen = false;
    pendingDelete = null;
  }
</script>

<div
  class="divide-y divide-border overflow-hidden rounded-xl bg-card/20 ring-1 ring-foreground/15"
>
  {#if values.loadError}
    <div class="p-5">
      <Alert.Root variant="destructive">
        <Alert.Title>Secrets and variables unavailable</Alert.Title>
        <Alert.Description>{values.loadError}</Alert.Description>
      </Alert.Root>
    </div>
  {/if}

  {#snippet section(
    kind: ActionValueKind,
    title: string,
    summary: string,
    rows: { name: string; detail: string; value?: string }[],
  )}
    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby={`actions-${kind}-heading`}
    >
      <header class="flex items-start gap-3">
        {#if kind === "secrets"}
          <KeyRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        {:else}
          <Variable class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        {/if}
        <div>
          <h2 id={`actions-${kind}-heading`} class="font-semibold">{title}</h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            {summary}
            {description}
          </p>
        </div>
      </header>

      <div class="grid max-w-2xl content-start gap-3">
        {#if values.loading && rows.length === 0}
          <p class="text-sm text-muted-foreground">Loading…</p>
        {:else if rows.length === 0}
          <p
            class="rounded-lg border border-dashed p-4 text-sm text-muted-foreground"
          >
            {kind === "secrets" ? "No secrets yet." : "No variables yet."}
          </p>
        {:else}
          <ul class="overflow-hidden rounded-lg border">
            {#each rows as row (row.name)}
              <li
                class="flex items-center justify-between gap-3 border-b px-4 py-3 last:border-b-0"
              >
                <div class="min-w-0">
                  <p class="truncate font-mono text-sm font-medium">
                    {row.name}
                  </p>
                  {#if row.value !== undefined}
                    <p class="mt-1 truncate font-mono text-xs text-muted-foreground">
                      {row.value}
                    </p>
                  {/if}
                  <p class="mt-1 text-xs text-muted-foreground">{row.detail}</p>
                </div>
                <div class="flex shrink-0 gap-1">
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={`Update ${row.name}`}
                    disabled={values.pending}
                    onclick={() => openEditor(kind, row.name, row.value ?? "")}
                  >
                    <Pencil />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={`Delete ${row.name}`}
                    disabled={values.pending}
                    onclick={() => requestDelete(kind, row.name)}
                  >
                    <Trash2 />
                  </Button>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
        <div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onclick={() => openEditor(kind)}
          >
            <Plus data-icon="inline-start" />{kind === "secrets"
              ? "Add secret"
              : "Add variable"}
          </Button>
        </div>
      </div>
    </section>
  {/snippet}

  {@render section(
    "secrets",
    "Secrets",
    "Encrypted values exposed to jobs as secrets.NAME. They cannot be read back.",
    values.secrets.map((secret) => ({
      name: secret.name,
      detail: `Updated ${dateFormatter.format(new Date(secret.updated_at))}`,
    })),
  )}
  {@render section(
    "variables",
    "Variables",
    "Plain configuration exposed to jobs as vars.NAME.",
    values.variables.map((variable) => ({
      name: variable.name,
      value: variable.value,
      detail: `Updated ${dateFormatter.format(new Date(variable.updated_at))}`,
    })),
  )}
</div>

<Dialog.Root bind:open={editorOpen}>
  <Dialog.Content class="sm:max-w-lg">
    <form
      class="grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void saveEditor();
      }}
    >
      <Dialog.Header>
        <Dialog.Title>
          {editorExisting ? "Update" : "Add"}
          {editorKind === "secrets" ? "secret" : "variable"}
        </Dialog.Title>
        <Dialog.Description>
          {editorKind === "secrets"
            ? "The value is encrypted at rest and never shown again. Saving replaces any previous value."
            : "Variables are stored in plain text and visible to repository managers."}
        </Dialog.Description>
      </Dialog.Header>
      <Field.Field data-invalid={nameError ? true : undefined}>
        <Field.Label for="actions-value-name">Name</Field.Label>
        <Input
          id="actions-value-name"
          class="font-mono uppercase"
          autocomplete="off"
          spellcheck={false}
          readonly={editorExisting}
          aria-invalid={nameError ? true : undefined}
          bind:value={editorName}
          onblur={() => (editorTouched = true)}
        />
        {#if nameError}
          <Field.Error>{nameError}</Field.Error>
        {:else}
          <Field.Description>
            Letters, digits, and underscores. Stored in upper case.
          </Field.Description>
        {/if}
      </Field.Field>
      <Field.Field>
        <Field.Label for="actions-value-value">Value</Field.Label>
        <Textarea
          id="actions-value-value"
          class="min-h-28 font-mono text-xs"
          autocomplete="off"
          spellcheck={false}
          bind:value={editorValue}
        />
      </Field.Field>
      <Dialog.Footer>
        <Button
          type="button"
          variant="outline"
          onclick={() => (editorOpen = false)}>Cancel</Button
        >
        <Button type="submit" disabled={values.pending}>
          {values.pending ? "Saving…" : "Save"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={deleteOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>
        Delete {pendingDelete?.kind === "secrets" ? "secret" : "variable"}
        {pendingDelete?.name}?
      </AlertDialog.Title>
      <AlertDialog.Description>
        Workflows that reference it will receive an empty value.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        disabled={values.pending}
        onclick={() => void confirmDelete()}>Delete</AlertDialog.Action
      >
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
