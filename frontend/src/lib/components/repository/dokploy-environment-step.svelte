<script lang="ts">
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import type { DokployConfigureState } from "$lib/repository/state/integrations-state.svelte.js";

  let { configure }: { configure: DokployConfigureState } = $props();
</script>

<div class="flex flex-wrap items-center justify-between gap-3">
  <div>
    <p class="sr-only">Step 1 of 3</p>
    <h2 class="mt-1 text-base font-semibold">Choose an environment</h2>
    <p class="mt-1 text-sm text-muted-foreground">
      Environments group deployable resources and shared configuration in
      Dokploy.
    </p>
  </div>
  {#if configure.link}
    <Button
      variant="outline"
      size="sm"
      onclick={() => (configure.showSetup = false)}>Cancel</Button
    >
  {/if}
</div>

{#if configure.catalogLoading && !configure.catalog}
  <div
    class="flex min-h-48 items-center justify-center rounded-xl border bg-card"
  >
    <Spinner class="size-5 animate-spin text-muted-foreground" />
  </div>
{:else if configure.catalog}
  {#if configure.catalog.projects.length === 0}
    <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      <IntegrationAddCard
        compact
        title="Create a project and environment"
        description="Create the first Dokploy project without leaving Gitadel."
        onclick={() => configure.beginEnvironment()}
      />
    </div>
  {:else}
    <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      <IntegrationAddCard
        compact
        title="New environment"
        description="Create an environment in one of your Dokploy projects."
        onclick={() => configure.beginEnvironment()}
      />
      {#each configure.catalog.projects as project (project.id)}
        {#each project.environments as environment (environment.id)}
          <IntegrationConnectionCard
            compact
            name={project.name}
            provider="dokploy"
            providerName={environment.name}
            subtitle="Dokploy environment"
            description={environment.description ??
              "Groups resources and shared deployment configuration."}
            selected={configure.selectedProjectId === project.id &&
              configure.selectedEnvironmentId === environment.id}
            enabled
            statusLabel={`${environment.resources.length} ${environment.resources.length === 1 ? "resource" : "resources"}`}
            detailLabel="Choose"
            onselect={() =>
              configure.chooseEnvironment(project, environment.id)}
          />
        {/each}
      {/each}
    </div>
  {/if}
{/if}
