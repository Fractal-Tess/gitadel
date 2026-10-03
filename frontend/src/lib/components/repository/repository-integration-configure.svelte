<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import Rocket from "@lucide/svelte/icons/rocket";

  import { resourceKindLabel } from "$lib/api/integrations.js";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import DokployConfigureStep from "$lib/components/repository/dokploy-configure-step.svelte";
  import DokployEnvironmentDialog from "$lib/components/repository/dokploy-environment-dialog.svelte";
  import DokployEnvironmentStep from "$lib/components/repository/dokploy-environment-step.svelte";
  import DokployResourceStep from "$lib/components/repository/dokploy-resource-step.svelte";
  import DokploySetupBreadcrumb from "$lib/components/repository/dokploy-setup-breadcrumb.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { DokployConfigureState } from "$lib/repository/state/integrations-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  const configure = $derived(new DokployConfigureState(repository));

  $effect(() => {
    void configure.load();
  });

  function openExternal(url: string) {
    window.open(url, "_blank", "noopener,noreferrer");
  }
</script>

<div class="space-y-6">
  <header class="flex items-center gap-4">
    <Button
      variant="ghost"
      size="icon"
      aria-label="Back to integrations"
      onclick={() =>
        repository.navigate("settings", { settingsTab: "integrations" })}
    >
      <ArrowLeft class="size-4" />
    </Button>
    <div class="min-w-0">
      <h1 class="truncate text-lg font-semibold">Dokploy deployment</h1>
      <p class="text-sm text-muted-foreground">
        Connect this repository to one Application or Docker Compose resource.
      </p>
    </div>
  </header>

  {#if configure.loading}
    <div
      class="flex min-h-48 items-center justify-center rounded-xl border bg-card"
    >
      <Spinner class="size-5 animate-spin text-muted-foreground" />
    </div>
  {:else if configure.loadError}
    <Alert.Root variant="destructive">
      <Alert.Title>Integration unavailable</Alert.Title>
      <Alert.Description>{configure.loadError}</Alert.Description>
    </Alert.Root>
  {:else if configure.detail && configure.link && !configure.showSetup}
    {@const detail = configure.detail}
    {@const link = configure.link}
    <div class="grid gap-4 xl:grid-cols-2">
      <IntegrationConnectionCard
        name={link.name}
        provider="dokploy"
        providerName={detail.connection_name}
        subtitle={`${link.project_name} / ${link.environment_name}`}
        description={`Deploys ${repository.name} from ${link.branch}. Build, domains, variables, and runtime settings stay in Dokploy.`}
        enabled={detail.enabled}
        statusLabel={detail.enabled ? "Push delivery on" : "Push delivery off"}
        detailLabel={resourceKindLabel(link.kind)}
        busy={configure.working}
        onenabledchange={(enabled) => void configure.setEnabled(enabled)}
        onconfigure={() => openExternal(link.external_url)}
        configureLabel="Open in Dokploy"
        configureExternal
        onremove={() => (configure.unlinkDialogOpen = true)}
      />

      <article
        class="flex min-h-52 flex-col rounded-xl border bg-card p-5 shadow-sm"
      >
        <div class="flex items-start gap-4">
          <span
            class="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
          >
            <Rocket class="size-5" />
          </span>
          <div class="min-w-0 flex-1">
            <p class="font-semibold">Deployment delivery</p>
            <p class="mt-1 text-sm leading-5 text-muted-foreground">
              Gitadel sends matching push events. Dokploy decides how the linked
              resource builds and runs.
            </p>
          </div>
        </div>
        <div
          class="mt-5 flex items-center justify-between rounded-lg border bg-muted/30 px-3 py-2.5"
        >
          <div>
            <p class="text-sm font-medium">Deploy on push</p>
            <p class="text-xs text-muted-foreground">Branch: {link.branch}</p>
          </div>
          <Switch
            checked={detail.enabled}
            disabled={configure.working}
            aria-label="Deploy on push"
            onCheckedChange={(enabled) => void configure.setEnabled(enabled)}
          />
        </div>
        <div class="mt-auto grid gap-2 pt-5 sm:grid-cols-2">
          <Button
            class="gap-2"
            disabled={configure.working}
            onclick={() => void configure.deployNow()}
          >
            {#if configure.working}<Spinner
                class="size-4 animate-spin"
              />{:else}<Rocket data-icon="inline-start" />{/if}
            Deploy now
          </Button>
          <Button
            variant="outline"
            disabled={configure.working}
            onclick={() => configure.changeResource()}>Change resource</Button
          >
        </div>
      </article>
    </div>
  {:else if configure.detail}
    <section id="dokploy-setup" class="space-y-5">
      <DokploySetupBreadcrumb {configure} />
      <!-- Each setup step settles in under the fixed step bar. -->
      {#key configure.setupStep}
        <div class="motion-view space-y-5">
          {#if configure.setupStep === "environments"}
            <DokployEnvironmentStep {configure} />
          {:else if configure.setupStep === "resources"}
            <DokployResourceStep {configure} />
          {:else}
            <DokployConfigureStep {configure} />
          {/if}
        </div>
      {/key}
    </section>
  {/if}

  {#if configure.operationError}
    <Alert.Root variant="destructive">
      <CircleAlert class="mt-0.5 size-4 shrink-0" />
      <Alert.Title>Deployment operation failed</Alert.Title>
      <Alert.Description>{configure.operationError}</Alert.Description>
    </Alert.Root>
  {/if}
</div>
<DokployEnvironmentDialog {configure} />

<AlertDialog.Root bind:open={configure.unlinkDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Unlink {configure.link?.name}?</AlertDialog.Title>
      <AlertDialog.Description>
        Gitadel will stop sending push events and disable auto-deploy on this
        resource. The resource and its configuration remain in Dokploy.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={configure.working}
        >Cancel</AlertDialog.Cancel
      >
      <AlertDialog.Action
        disabled={configure.working}
        variant="destructive"
        onclick={(event) => {
          event.preventDefault();
          void configure.unlinkResource();
        }}
      >
        {#if configure.working}<Spinner class="size-4 animate-spin" />{/if}
        Unlink resource
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
