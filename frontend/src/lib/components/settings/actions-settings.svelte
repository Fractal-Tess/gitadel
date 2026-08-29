<script lang="ts">
  import {
    CircleAlert,
    CircleCheck,
    LoaderCircle,
    RefreshCw,
    Server,
    Workflow,
  } from "lucide-svelte";
  import { onMount } from "svelte";

  import type { ActionRunner } from "$lib/api.js";
  import RunnerRegistrationDialog from "$lib/components/actions/runner-registration-dialog.svelte";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import type { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";

  type Target = { slug: string; label: string };
  type RunnerCard = { target: Target; runner: ActionRunner };
  type PendingRemoval = RunnerCard;
  type ConnectionTest = { healthy: boolean; message: string };

  let {
    state: account,
    namespace,
  }: {
    state: AccountSettingsState;
    namespace: Target;
  } = $props();

  const cards = $derived<RunnerCard[]>(
    (account.actionRunners[namespace.slug] ?? []).map((runner) => ({
      target: namespace,
      runner,
    })),
  );

  let createDialogOpen = $state(false);
  let runnerName = $state("gitadel-runner");
  let runnerLabels = $state("docker");
  const createLabels = $derived(
    runnerLabels
      .split(",")
      .map((label) => label.trim())
      .filter(Boolean),
  );
  let configureDialogOpen = $state(false);
  let configuring = $state<RunnerCard | null>(null);
  let connectionTesting = $state(false);
  let connectionTest = $state<ConnectionTest | null>(null);
  let removeDialogOpen = $state(false);
  let pendingRemoval = $state<PendingRemoval | null>(null);

  onMount(() => {
    void account.loadActionRunners([namespace.slug]);
  });

  function openCreate() {
    account.closeActionRegistration();
    runnerName = "gitadel-runner";
    runnerLabels = "docker";
    createDialogOpen = true;
  }

  async function register() {
    const name = runnerName.trim();
    if (!name || createLabels.length === 0) return;
    await account.issueActionRunner(
      namespace.slug,
      runnerName.trim(),
      createLabels,
    );
    if (account.actionRegistration) createDialogOpen = false;
  }

  function openConfigure(card: RunnerCard) {
    configuring = card;
    connectionTest = null;
    configureDialogOpen = true;
  }

  async function testConnection() {
    const card = configuring;
    if (!card || connectionTesting) return;
    connectionTesting = true;
    connectionTest = null;
    await account.loadActionRunners([namespace.slug]);
    const runner = (account.actionRunners[card.target.slug] ?? []).find(
      (candidate) => candidate.id === card.runner.id,
    );
    if (!runner) {
      connectionTest = {
        healthy: false,
        message:
          "Gitadel could not find this runner. It may have been removed.",
      };
    } else {
      configuring = { target: card.target, runner };
      if (runner.incompatibility) {
        connectionTest = { healthy: false, message: runner.incompatibility };
      } else if (runner.status !== "online") {
        connectionTest = {
          healthy: false,
          message:
            "Gitadel has not received a recent heartbeat. Start the runner daemon, then test again.",
        };
      } else if (runner.labels.length === 0) {
        connectionTest = {
          healthy: false,
          message: "The runner is connected but has no scheduling labels.",
        };
      } else {
        connectionTest = {
          healthy: true,
          message: `Connected to Forgejo Runner v${runner.version}. Its labels are ready for scheduling.`,
        };
      }
    }
    connectionTesting = false;
  }

  function requestRemove(card: RunnerCard) {
    pendingRemoval = card;
    configureDialogOpen = false;
    removeDialogOpen = true;
  }

  async function removeRunner() {
    if (!pendingRemoval) return;
    await account.removeActionRunner(
      pendingRemoval.target.slug,
      pendingRemoval.runner.id,
    );
    if (!account.actionErrors[pendingRemoval.target.slug]) {
      removeDialogOpen = false;
      pendingRemoval = null;
      configuring = null;
    }
  }

  function runnerStatus(runner: ActionRunner) {
    if (runner.incompatibility) return "Incompatible";
    return runner.status === "online" ? "Online" : "Offline";
  }

  function formatDate(value: string | null) {
    if (!value) return "Never connected";
    const date = new Date(value);
    return Number.isNaN(date.getTime())
      ? value
      : new Intl.DateTimeFormat(undefined, {
          dateStyle: "medium",
          timeStyle: "short",
        }).format(date);
  }
</script>

{#snippet runnerIcon()}
  <Server class="size-6 text-primary" />
{/snippet}

<section class="space-y-6" aria-labelledby="actions-settings-heading">
  <header>
    <h2
      id="actions-settings-heading"
      class="text-lg font-semibold tracking-tight"
    >
      Runners
    </h2>
    <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
      Register runners for {namespace.label}. Repositories in this namespace can
      use any runner listed here.
    </p>
  </header>

  {#if account.actionsLoading && Object.keys(account.actionRunners).length === 0}
    <p class="flex items-center gap-2 text-sm text-muted-foreground">
      <LoaderCircle class="size-4 animate-spin" />Loading runners…
    </p>
  {:else}
    {#if account.actionsLoadError}
      <div
        class="flex flex-wrap items-center justify-between gap-3 rounded-md border border-destructive/30 bg-destructive/5 p-3"
        role="alert"
      >
        <p class="text-sm text-destructive">{account.actionsLoadError}</p>
        <Button
          type="button"
          size="sm"
          variant="outline"
          onclick={() => void account.loadActionRunners([namespace.slug])}
          >Retry</Button
        >
      </div>
    {/if}

    <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
      <IntegrationAddCard
        title="Add runner"
        description={`Register a Forgejo Runner for ${namespace.label}.`}
        onclick={openCreate}
      />
      {#each cards as card (card.runner.id)}
        <IntegrationConnectionCard
          name={card.runner.name}
          provider="actions-runner"
          providerName="Forgejo Runner"
          icon={runnerIcon}
          subtitle={card.target.label}
          description={card.runner.labels.length > 0
            ? `Runs jobs labeled ${card.runner.labels.join(", ")}.`
            : "No scheduling labels configured."}
          enabled={card.runner.status === "online" &&
            !card.runner.incompatibility}
          statusLabel={runnerStatus(card.runner)}
          statusHealthy={card.runner.status === "online" &&
            !card.runner.incompatibility}
          detailLabel={`v${card.runner.version}`}
          busy={account.actionsWorking[card.target.slug]}
          error={account.actionErrors[card.target.slug]}
          onconfigure={() => openConfigure(card)}
          onremove={() => requestRemove(card)}
        />
      {/each}
    </div>
  {/if}
</section>

<Dialog.Root bind:open={createDialogOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>Add runner</Dialog.Title>
      <Dialog.Description>
        Create a one-time registration token for {namespace.label}.
      </Dialog.Description>
    </Dialog.Header>
    <form
      class="grid gap-5"
      onsubmit={(event) => {
        event.preventDefault();
        void register();
      }}
    >
      <div class="rounded-md border bg-card/20 p-3 text-sm">
        <span class="text-muted-foreground">Owner</span>
        <span class="ml-3 font-medium">{namespace.label}</span>
        <span class="ml-2 font-mono text-xs text-muted-foreground">
          {namespace.slug}
        </span>
      </div>
      <Field.Field>
        <Field.Label for="new-runner-name">Runner name</Field.Label>
        <Input
          id="new-runner-name"
          bind:value={runnerName}
          maxlength={100}
          required
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="new-runner-labels">Labels</Field.Label>
        <Input
          id="new-runner-labels"
          bind:value={runnerLabels}
          placeholder="docker"
          required
        />
        <Field.Description>
          Use 1–16 comma-separated exact labels. The runner configuration must
          define the same labels.
        </Field.Description>
      </Field.Field>
      {#if account.actionErrors[namespace.slug]}
        <p
          class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
          role="alert"
        >
          {account.actionErrors[namespace.slug]}
        </p>
      {/if}
      <Dialog.Footer>
        <Button
          type="button"
          variant="ghost"
          onclick={() => (createDialogOpen = false)}>Cancel</Button
        >
        <Button
          type="submit"
          disabled={account.actionsWorking[namespace.slug] ||
            !runnerName.trim() ||
            createLabels.length === 0 ||
            createLabels.length > 16}
        >
          {account.actionsWorking[namespace.slug]
            ? "Creating token…"
            : "Create registration token"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root bind:open={configureDialogOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title
        >Configure {configuring?.runner.name ?? "runner"}</Dialog.Title
      >
      <Dialog.Description>
        Review the registered owner and labels, then test the live runner
        connection.
      </Dialog.Description>
    </Dialog.Header>
    {#if configuring}
      <div class="grid gap-5">
        <dl class="grid gap-4 rounded-lg border bg-card/20 p-4 text-sm">
          <div class="grid gap-1 sm:grid-cols-[8rem_1fr] sm:gap-4">
            <dt class="text-muted-foreground">Owner</dt>
            <dd>
              <span class="font-medium">{configuring.target.label}</span>
              <span class="ml-2 font-mono text-xs text-muted-foreground">
                {configuring.target.slug}
              </span>
            </dd>
          </div>
          <div class="grid gap-1 sm:grid-cols-[8rem_1fr] sm:gap-4">
            <dt class="text-muted-foreground">Name</dt>
            <dd class="font-medium">{configuring.runner.name}</dd>
          </div>
          <div class="grid gap-1 sm:grid-cols-[8rem_1fr] sm:gap-4">
            <dt class="text-muted-foreground">Labels</dt>
            <dd>{configuring.runner.labels.join(", ") || "None"}</dd>
          </div>
          <div class="grid gap-1 sm:grid-cols-[8rem_1fr] sm:gap-4">
            <dt class="text-muted-foreground">Version</dt>
            <dd>Forgejo Runner v{configuring.runner.version}</dd>
          </div>
          <div class="grid gap-1 sm:grid-cols-[8rem_1fr] sm:gap-4">
            <dt class="text-muted-foreground">Connection</dt>
            <dd>{runnerStatus(configuring.runner)}</dd>
          </div>
          <div class="grid gap-1 sm:grid-cols-[8rem_1fr] sm:gap-4">
            <dt class="text-muted-foreground">Last seen</dt>
            <dd>{formatDate(configuring.runner.last_seen_at)}</dd>
          </div>
        </dl>
        <p class="text-xs leading-5 text-muted-foreground">
          The owner, name, and labels are fixed at registration. Remove and
          re-register the runner to change them.
        </p>
        {#if connectionTest}
          <div
            class={[
              "flex items-start gap-3 rounded-md border p-3 text-sm",
              connectionTest.healthy
                ? "border-emerald-500/30 bg-emerald-500/5 text-emerald-300"
                : "border-destructive/30 bg-destructive/5 text-destructive",
            ]}
            role="status"
          >
            {#if connectionTest.healthy}
              <CircleCheck class="mt-0.5 size-4 shrink-0" />
            {:else}
              <CircleAlert class="mt-0.5 size-4 shrink-0" />
            {/if}
            <p>{connectionTest.message}</p>
          </div>
        {/if}
        <Dialog.Footer class="sm:justify-between">
          <Button
            type="button"
            variant="ghost"
            class="text-muted-foreground hover:text-destructive"
            disabled={account.actionsWorking[configuring.target.slug]}
            onclick={() => requestRemove(configuring!)}>Remove runner</Button
          >
          <div class="flex gap-2">
            <Button
              type="button"
              variant="ghost"
              onclick={() => (configureDialogOpen = false)}>Close</Button
            >
            <Button
              type="button"
              variant="outline"
              class="gap-2"
              disabled={connectionTesting}
              onclick={() => void testConnection()}
            >
              {#if connectionTesting}
                <LoaderCircle class="size-4 animate-spin" />Testing…
              {:else}
                <RefreshCw class="size-4" />Test connection
              {/if}
            </Button>
          </div>
        </Dialog.Footer>
      </div>
    {/if}
  </Dialog.Content>
</Dialog.Root>

<RunnerRegistrationDialog
  registration={account.actionRegistration}
  onclose={() => account.closeActionRegistration()}
/>

<AlertDialog.Root bind:open={removeDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>
        Remove {pendingRemoval?.runner.name ?? "this runner"}?
      </AlertDialog.Title>
      <AlertDialog.Description>
        It will stop receiving jobs from the
        {pendingRemoval?.target.slug ?? "selected"} namespace. Running jobs may be
        cancelled.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={pendingRemoval
          ? account.actionsWorking[pendingRemoval.target.slug]
          : false}
        onclick={() => void removeRunner()}>Remove runner</AlertDialog.Action
      >
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
