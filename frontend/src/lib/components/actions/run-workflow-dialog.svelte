<script lang="ts">
  import type { ActionWorkflow } from "$lib/api/actions.js";
  import Play from "@lucide/svelte/icons/play";

  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as NativeSelect from "$lib/components/ui/native-select/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import type { RepositoryActionsState } from "$lib/repository/state/actions-state.svelte.js";

  let { actions }: { actions: RepositoryActionsState } = $props();

  let open = $state(false);
  let workflowPath = $state("");
  let reference = $state("");
  let values = $state<Record<string, string>>({});

  const dispatchable = $derived(
    actions.workflows?.workflows.filter((workflow) => workflow.dispatchable) ??
      [],
  );
  const selected = $derived<ActionWorkflow | undefined>(
    dispatchable.find((workflow) => workflow.path === workflowPath),
  );
  const missing = $derived(
    selected?.inputs
      .filter(
        (input) =>
          input.required &&
          input.type !== "boolean" &&
          !(values[input.name] ?? "").trim(),
      )
      .map((input) => input.name) ?? [],
  );

  function shortReference(value: string): string {
    return value.replace(/^refs\/(heads|tags)\//, "");
  }

  function selectWorkflow(path: string): void {
    workflowPath = path;
    const workflow = dispatchable.find((candidate) => candidate.path === path);
    values = Object.fromEntries(
      (workflow?.inputs ?? []).map((input) => [
        input.name,
        input.default ?? (input.type === "boolean" ? "false" : ""),
      ]),
    );
  }

  async function openDialog(): Promise<void> {
    await actions.loadWorkflows();
    reference = shortReference(actions.workflows?.reference ?? "");
    selectWorkflow(dispatchable[0]?.path ?? "");
  }

  async function reloadForReference(): Promise<void> {
    const current = workflowPath;
    await actions.loadWorkflows(reference.trim() || undefined);
    selectWorkflow(
      dispatchable.some((workflow) => workflow.path === current)
        ? current
        : (dispatchable[0]?.path ?? ""),
    );
  }

  async function submit(): Promise<void> {
    if (!selected || missing.length) return;
    if (
      await actions.dispatchWorkflow(
        selected.path,
        reference.trim(),
        $state.snapshot(values),
      )
    )
      open = false;
  }
</script>

<Dialog.Root
  bind:open
  onOpenChange={(next) => {
    if (next) void openDialog();
  }}
>
  <Dialog.Trigger>
    {#snippet child({ props })}
      <Button {...props} size="sm" class="gap-2">
        <Play data-icon="inline-start" />Run workflow
      </Button>
    {/snippet}
  </Dialog.Trigger>
  <Dialog.Content class="sm:max-w-lg">
    <form
      class="grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <Dialog.Header>
        <Dialog.Title>Run workflow</Dialog.Title>
        <Dialog.Description>
          Start a workflow that declares <code>workflow_dispatch</code>.
        </Dialog.Description>
      </Dialog.Header>

      <Field.Field>
        <Field.Label for="dispatch-ref">Branch or tag</Field.Label>
        <Input
          id="dispatch-ref"
          class="font-mono"
          autocomplete="off"
          spellcheck={false}
          bind:value={reference}
          onchange={() => void reloadForReference()}
        />
      </Field.Field>

      {#if actions.workflowsError}
        <Alert.Root variant="destructive">
          <Alert.Title>Workflows unavailable</Alert.Title>
          <Alert.Description>{actions.workflowsError}</Alert.Description>
        </Alert.Root>
      {:else if actions.workflowsLoading && !actions.workflows}
        <p class="text-sm text-muted-foreground">Loading workflows…</p>
      {:else if dispatchable.length === 0}
        <p
          class="rounded-lg border border-dashed p-4 text-sm text-muted-foreground"
        >
          No workflow at this ref declares <code>workflow_dispatch</code>.
        </p>
      {:else}
        <Field.Field>
          <Field.Label for="dispatch-workflow">Workflow</Field.Label>
          <NativeSelect.Root
            id="dispatch-workflow"
            class="w-full"
            value={workflowPath}
            onchange={(event) => selectWorkflow(event.currentTarget.value)}
          >
            {#each dispatchable as workflow (workflow.path)}
              <NativeSelect.Option value={workflow.path}
                >{workflow.name}</NativeSelect.Option
              >
            {/each}
          </NativeSelect.Root>
          {#if selected}
            <Field.Description class="font-mono text-xs"
              >{selected.path}</Field.Description
            >
          {/if}
        </Field.Field>

        {#each selected?.inputs ?? [] as input (input.name)}
          {@const id = `dispatch-input-${input.name}`}
          <Field.Field>
            {#if input.type === "boolean"}
              <div class="flex items-center justify-between gap-4">
                <Field.Label for={id}>{input.name}</Field.Label>
                <Switch
                  {id}
                  checked={values[input.name] === "true"}
                  onCheckedChange={(checked) =>
                    (values[input.name] = checked ? "true" : "false")}
                />
              </div>
            {:else}
              <Field.Label for={id}>
                {input.name}{input.required ? " *" : ""}
              </Field.Label>
              {#if input.type === "choice"}
                <NativeSelect.Root
                  {id}
                  class="w-full"
                  bind:value={values[input.name]}
                >
                  {#if !input.required}
                    <NativeSelect.Option value="">—</NativeSelect.Option>
                  {/if}
                  {#each input.options as option (option)}
                    <NativeSelect.Option value={option}
                      >{option}</NativeSelect.Option
                    >
                  {/each}
                </NativeSelect.Root>
              {:else}
                <Input
                  {id}
                  type={input.type === "number" ? "number" : "text"}
                  autocomplete="off"
                  bind:value={values[input.name]}
                />
              {/if}
            {/if}
            {#if input.description}
              <Field.Description>{input.description}</Field.Description>
            {/if}
          </Field.Field>
        {/each}
      {/if}

      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (open = false)}
          >Cancel</Button
        >
        <Button
          type="submit"
          disabled={!selected || missing.length > 0 || actions.actionsPending}
        >
          {actions.actionsPending ? "Starting…" : "Run workflow"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
