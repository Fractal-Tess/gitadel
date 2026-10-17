<script lang="ts">
  import { resolve } from "$app/paths";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import Rocket from "@lucide/svelte/icons/rocket";
  import Settings2 from "@lucide/svelte/icons/settings-2";

  import {
    resourceKindLabel,
    type RepositoryIntegration,
  } from "$lib/api/integrations.js";
  import DokployDeploymentStatus from "$lib/components/repository/dokploy-deployment-status.svelte";
  import DokploySetupResult from "$lib/components/repository/dokploy-setup-result.svelte";
  import RepositoryIntegrationConfigure from "$lib/components/repository/repository-integration-configure.svelte";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { DokployConfigureState } from "$lib/repository/state/integrations-state.svelte.js";

  let {
    state: repository,
    integration,
    connectionReady,
  }: {
    state: RepositoryPageState;
    integration: RepositoryIntegration;
    /** Whether the namespace connection behind it can deploy. */
    connectionReady: boolean;
  } = $props();

  const configure = $derived(
    new DokployConfigureState(repository, integration.id),
  );
  const uid = $props.id();
  const headingId = `${uid}-deploy-heading`;
  const linked = $derived(
    configure.detail && configure.link && !configure.showSetup
      ? { detail: configure.detail, link: configure.link }
      : null,
  );

  $effect(() => {
    if (connectionReady) void configure.load();
  });

  // Poll the latest deployment while the linked resource is shown.
  $effect(() => {
    if (!linked) return;
    void configure.deploymentWatch;
    return configure.watchDeployment();
  });
</script>

<article
  class="space-y-5 rounded-xl border bg-card/40 p-5 shadow-sm"
  aria-labelledby={headingId}
>
  <header class="flex items-start gap-4">
    <span
      class="grid size-12 shrink-0 place-items-center rounded-xl border bg-zinc-950 shadow-sm"
      aria-hidden="true"
    >
      <img
        class="size-7 object-contain"
        src="/integrations/dokploy.svg"
        alt=""
      />
    </span>
    <div class="min-w-0 flex-1">
      <h2 id={headingId} class="truncate font-semibold">
        {linked?.link.name ?? integration.name}
      </h2>
      <p class="mt-0.5 truncate text-xs text-muted-foreground">
        {integration.connection_name} ·
        {#if linked}
          {linked.link.project_name} / {linked.link.environment_name} /
          {linked.link.name}
        {:else}
          Not set up yet
        {/if}
      </p>
    </div>
    {#if linked}
      <div class="flex shrink-0 items-center gap-2">
        <label
          for={`${headingId}-push`}
          class="text-sm text-muted-foreground"
        >
          Deploy on push
        </label>
        <Switch
          id={`${headingId}-push`}
          checked={linked.detail.enabled}
          disabled={configure.working}
          onCheckedChange={(enabled) => void configure.setEnabled(enabled)}
        />
      </div>
    {/if}
  </header>

  {#if !connectionReady}
    <Alert.Root class="border-amber-500/40 bg-amber-500/5">
      <CircleAlert class="size-4 shrink-0 text-amber-500" />
      <Alert.Title>The connection is not finished</Alert.Title>
      <Alert.Description>
        <p>
          {integration.connection_name} needs a repository source before Dokploy
          can deploy this repository.
        </p>
        <Button
          class="mt-2"
          variant="outline"
          size="sm"
          href={resolve("/[namespace]/integrations", {
            namespace: repository.namespace,
          })}>Finish the connection</Button
        >
      </Alert.Description>
    </Alert.Root>
  {:else if linked}
    {@const link = linked.link}
    {#if configure.setupResult}
      <DokploySetupResult result={configure.setupResult} />
    {/if}
    <dl class="grid gap-3 text-sm sm:grid-cols-4">
      <div class="min-w-0">
        <dt class="text-xs text-muted-foreground">Project</dt>
        <dd class="truncate">{link.project_name}</dd>
      </div>
      <div class="min-w-0">
        <dt class="text-xs text-muted-foreground">Environment</dt>
        <dd class="truncate">{link.environment_name}</dd>
      </div>
      <div class="min-w-0">
        <dt class="text-xs text-muted-foreground">Resource</dt>
        <dd class="truncate">
          {link.name}
          <span class="text-muted-foreground">
            · {resourceKindLabel(link.kind)}</span
          >
        </dd>
      </div>
      <div class="min-w-0">
        <dt class="text-xs text-muted-foreground">Deploy on push</dt>
        <dd class="truncate">
          {linked.detail.enabled ? `On for ${link.branch}` : "Off"}
        </dd>
      </div>
    </dl>
    {#if !linked.detail.configured}
      <Alert.Root class="border-amber-500/40 bg-amber-500/5">
        <CircleAlert class="size-4 shrink-0 text-amber-500" />
        <Alert.Title>The connection is disabled</Alert.Title>
        <Alert.Description>
          {integration.connection_name} is disabled, so pushes are not delivered.
        </Alert.Description>
      </Alert.Root>
    {/if}
    <DokployDeploymentStatus {configure} />
    <div class="flex flex-wrap gap-2">
      <Button
        class="gap-2"
        disabled={configure.working}
        onclick={() => void configure.deployNow()}
      >
        {#if configure.working}
          <Spinner class="size-4 animate-spin" />
        {:else}
          <Rocket data-icon="inline-start" />
        {/if}
        Deploy now
      </Button>
      <Button
        variant="outline"
        class="gap-2"
        href={link.external_url}
        target="_blank"
        rel="noreferrer"
      >
        <ExternalLink data-icon="inline-start" />Open in Dokploy
      </Button>
      <Button
        variant="ghost"
        class="gap-2"
        onclick={() => repository.navigate("deploy", { provider: integration.id })}
      >
        <Settings2 data-icon="inline-start" />Manage
      </Button>
    </div>
    <p class="text-xs leading-5 text-muted-foreground">
      Environment variables and advanced settings are managed in Dokploy.
      <Button
        variant="link"
        class="h-auto p-0 text-xs"
        href={link.external_url}
        target="_blank"
        rel="noreferrer">Open in Dokploy</Button
      >
    </p>
    {#if configure.operationError}
      <Alert.Root variant="destructive">
        <CircleAlert class="mt-0.5 size-4 shrink-0" />
        <Alert.Title>Dokploy did not accept this</Alert.Title>
        <Alert.Description>{configure.operationError}</Alert.Description>
      </Alert.Root>
    {/if}
  {:else}
    <RepositoryIntegrationConfigure state={repository} {configure} embedded />
  {/if}
</article>
