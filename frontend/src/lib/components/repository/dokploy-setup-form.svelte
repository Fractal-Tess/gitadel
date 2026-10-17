<script lang="ts">
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import Rocket from "@lucide/svelte/icons/rocket";

  import {
    DOKPLOY_SERVER,
    NEW_DOKPLOY_PROJECT,
    OTHER_BUILD,
  } from "$lib/api/integrations.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import type { DokployConfigureState } from "$lib/repository/state/integrations-state.svelte.js";

  let { configure }: { configure: DokployConfigureState } = $props();

  const form = $derived(configure.form);
  const catalog = $derived(configure.catalog);
  const isCompose = $derived(
    configure.selectedChoice?.suggestion?.kind === "compose",
  );
  const detected = $derived(configure.choices.length > 1);
  const branchLabel = $derived(form.branch.trim() || "the branch");
  const environmentNames = $derived(
    configure.formProject?.environments.map((environment) => environment.name) ??
      [],
  );
</script>

<form
  class="max-w-2xl space-y-6 rounded-xl border bg-card p-5 shadow-sm"
  aria-labelledby="dokploy-setup-heading"
  onsubmit={(event) => {
    event.preventDefault();
    void configure.submitSetup();
  }}
>
  <div>
    <h2 id="dokploy-setup-heading" class="text-base font-semibold">
      Set up deployment
    </h2>
    <p class="mt-1 text-sm leading-6 text-muted-foreground">
      Gitadel creates the resource in Dokploy, points it at this repository, and
      can start the first deployment. Check the choices below, then set it up.
    </p>
  </div>

  <fieldset class="grid gap-2" disabled={configure.setupWorking}>
    <legend class="mb-2 text-sm font-medium">Build</legend>
    {#if !detected}
      <p class="text-xs leading-5 text-muted-foreground">
        {configure.suggestionsError ??
          `No Dockerfile or Compose file was found on ${branchLabel}.`}
        Gitadel will create an application and you choose how it builds in Dokploy.
      </p>
    {/if}
    {#each configure.choices as choice (choice.id)}
      <label
        class="flex cursor-pointer gap-3 rounded-lg border p-3 transition-colors hover:bg-muted/30 has-checked:border-primary/50 has-checked:bg-primary/5 has-focus-visible:ring-2 has-focus-visible:ring-ring"
      >
        <input
          type="radio"
          name="dokploy-build"
          class="mt-1 size-4 shrink-0 accent-primary"
          value={choice.id}
          checked={form.build === choice.id}
          onchange={() => configure.chooseBuild(choice.id)}
        />
        <span class="min-w-0">
          <span class="block text-sm font-medium break-words">{choice.label}</span>
          <span class="mt-0.5 block text-xs leading-5 text-muted-foreground"
            >{choice.detail}</span
          >
        </span>
      </label>
    {/each}
    {#if form.build === OTHER_BUILD}
      <Field.Field class="mt-2">
        <Field.Label for="dokploy-setup-build-path">Build path</Field.Label>
        <Input
          id="dokploy-setup-build-path"
          bind:value={configure.form.buildPath}
          placeholder="/"
        />
        <Field.Description>
          The directory Dokploy builds from. Use a subdirectory such as /web for
          a monorepo.
        </Field.Description>
      </Field.Field>
    {/if}
  </fieldset>

  <fieldset class="grid gap-4 sm:grid-cols-2" disabled={configure.setupWorking}>
    <legend class="sr-only">Resource</legend>
    <Field.Field>
      <Field.Label for="dokploy-setup-name">Name</Field.Label>
      <Input
        id="dokploy-setup-name"
        bind:value={configure.form.name}
        maxlength={80}
        required
      />
    </Field.Field>
    <Field.Field>
      <Field.Label for="dokploy-setup-branch">Branch</Field.Label>
      <Input id="dokploy-setup-branch" bind:value={configure.form.branch} required />
    </Field.Field>

    <Field.Field>
      <Field.Label for="dokploy-setup-project">Project</Field.Label>
      <Select.Root
        type="single"
        bind:value={configure.form.project}
        disabled={configure.setupWorking}
      >
        <Select.Trigger id="dokploy-setup-project" class="w-full">
          {form.project === NEW_DOKPLOY_PROJECT
            ? `New project: ${form.projectName.trim() || "unnamed"}`
            : (configure.formProject?.name ?? "Select a project")}
        </Select.Trigger>
        <Select.Content>
          <Select.Group>
            <Select.Item value={NEW_DOKPLOY_PROJECT}>
              New project: {form.projectName.trim() || "unnamed"}
            </Select.Item>
            {#each catalog?.projects ?? [] as project (project.id)}
              <Select.Item value={project.id}>{project.name}</Select.Item>
            {/each}
          </Select.Group>
        </Select.Content>
      </Select.Root>
    </Field.Field>
    {#if form.project === NEW_DOKPLOY_PROJECT}
      <Field.Field>
        <Field.Label for="dokploy-setup-project-name">
          New project name
        </Field.Label>
        <Input
          id="dokploy-setup-project-name"
          bind:value={configure.form.projectName}
          maxlength={80}
          required
        />
      </Field.Field>
    {/if}
    <Field.Field>
      <Field.Label for="dokploy-setup-environment">Environment</Field.Label>
      <Input
        id="dokploy-setup-environment"
        bind:value={configure.form.environment}
        list="dokploy-setup-environments"
        required
      />
      <datalist id="dokploy-setup-environments">
        {#each environmentNames as environmentName (environmentName)}
          <option value={environmentName}></option>
        {/each}
      </datalist>
      <Field.Description>
        "production" uses the project's default environment. A new name creates
        that environment.
      </Field.Description>
    </Field.Field>
    {#if catalog && catalog.servers.length > 0}
      <Field.Field>
        <Field.Label for="dokploy-setup-server">Server</Field.Label>
        <Select.Root
          type="single"
          bind:value={configure.form.server}
          disabled={configure.setupWorking}
        >
          <Select.Trigger id="dokploy-setup-server" class="w-full">
            {form.server === DOKPLOY_SERVER
              ? "Dokploy server"
              : (catalog.servers.find((server) => server.id === form.server)
                  ?.name ?? "Select a server")}
          </Select.Trigger>
          <Select.Content>
            <Select.Group>
              <Select.Item value={DOKPLOY_SERVER}>Dokploy server</Select.Item>
              {#each catalog.servers as server (server.id)}
                <Select.Item value={server.id}>{server.name}</Select.Item>
              {/each}
            </Select.Group>
          </Select.Content>
        </Select.Root>
      </Field.Field>
    {/if}
  </fieldset>

  {#if !isCompose}
    <fieldset class="grid gap-4 border-t pt-5" disabled={configure.setupWorking}>
      <legend class="sr-only">Domain</legend>
      <div>
        <p class="text-sm font-medium">Domain (optional)</p>
        <p class="mt-0.5 text-xs leading-5 text-muted-foreground">
          Leave the host empty to add a domain later in Dokploy.
        </p>
      </div>
      <div class="grid gap-4 sm:grid-cols-[minmax(0,1fr)_8rem]">
        <Field.Field>
          <Field.Label for="dokploy-setup-host">Host</Field.Label>
          <Input
            id="dokploy-setup-host"
            bind:value={configure.form.domainHost}
            placeholder="app.example.com"
            autocomplete="off"
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="dokploy-setup-port">Port</Field.Label>
          <Input
            id="dokploy-setup-port"
            type="number"
            min={1}
            max={65535}
            bind:value={configure.form.domainPort}
          />
        </Field.Field>
      </div>
      <Field.Field orientation="horizontal">
        <Switch
          id="dokploy-setup-https"
          bind:checked={configure.form.domainHttps}
          disabled={configure.setupWorking}
        />
        <Field.Content>
          <Field.Label for="dokploy-setup-https">HTTPS</Field.Label>
          <Field.Description>
            Dokploy requests a Let's Encrypt certificate. The host's DNS must
            already point at your Dokploy server.
          </Field.Description>
        </Field.Content>
      </Field.Field>
    </fieldset>
  {/if}

  <fieldset class="grid gap-3 border-t pt-5" disabled={configure.setupWorking}>
    <legend class="sr-only">After setup</legend>
    <Field.Field orientation="horizontal">
      <Checkbox
        id="dokploy-setup-deploy"
        bind:checked={configure.form.deploy}
        disabled={configure.setupWorking}
      />
      <Field.Label for="dokploy-setup-deploy">Deploy now</Field.Label>
    </Field.Field>
    <Field.Field orientation="horizontal">
      <Checkbox
        id="dokploy-setup-deploy-on-push"
        bind:checked={configure.form.deployOnPush}
        disabled={configure.setupWorking}
      />
      <Field.Label for="dokploy-setup-deploy-on-push">
        Deploy on every push to {branchLabel}
      </Field.Label>
    </Field.Field>
  </fieldset>

  {#if configure.setupError}
    <Alert.Root variant="destructive">
      <CircleAlert class="size-4 shrink-0" />
      <Alert.Title>Setup did not finish</Alert.Title>
      <Alert.Description>{configure.setupError}</Alert.Description>
    </Alert.Root>
  {/if}

  <div
    class="flex flex-col-reverse gap-3 border-t pt-5 sm:flex-row sm:items-center sm:justify-between"
  >
    <Button
      type="button"
      variant="link"
      class="h-auto justify-start px-0"
      disabled={configure.setupWorking}
      onclick={() => configure.openLinkWizard()}
    >
      Link an existing Dokploy resource instead
    </Button>
    <div class="flex flex-col items-stretch gap-2 sm:items-end">
      <Button
        type="submit"
        class="gap-2"
        disabled={configure.setupWorking || configure.formProblem !== null}
      >
        {#if configure.setupWorking}
          <Spinner class="size-4 animate-spin" />
        {:else}
          <Rocket data-icon="inline-start" />
        {/if}
        Set up deployment
      </Button>
      <p class="text-xs text-muted-foreground" aria-live="polite">
        {#if configure.setupWorking}
          Setting up in Dokploy…
        {:else if configure.formProblem}
          {configure.formProblem}
        {/if}
      </p>
    </div>
  </div>
</form>
