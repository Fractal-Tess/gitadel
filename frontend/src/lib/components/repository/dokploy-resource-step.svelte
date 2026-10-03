<script lang="ts">
  import {
    dokployStatusLabel,
    resourceKindLabel,
  } from "$lib/api/integrations.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import type { DokployConfigureState } from "$lib/repository/state/integrations-state.svelte.js";

  let { configure }: { configure: DokployConfigureState } = $props();
</script>

<div class="flex flex-wrap items-center justify-between gap-3">
  <div>
    <p class="sr-only">Step 2 of 3</p>
    <h2 class="mt-1 text-base font-semibold">Choose a resource</h2>
    <p class="mt-1 text-sm text-muted-foreground">
      Select an existing resource or create one inside {configure
        .selectedEnvironment?.name}.
    </p>
  </div>
</div>

{#if configure.catalogLoading && !configure.catalog}
  <div
    class="flex min-h-48 items-center justify-center rounded-xl border bg-card"
  >
    <Spinner class="size-5 animate-spin text-muted-foreground" />
  </div>
{:else if configure.catalog && configure.selectedEnvironment}
  {@const selectedEnvironment = configure.selectedEnvironment}
  <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
    <IntegrationAddCard
      compact
      title="New resource"
      description={`Create an Application or Docker Compose resource in ${selectedEnvironment.name}.`}
      onclick={() => configure.beginResourceKind()}
    />
    {#each selectedEnvironment.resources as resource (resource.id)}
      <IntegrationConnectionCard
        compact
        name={resource.name}
        provider="dokploy"
        providerName={resourceKindLabel(resource.kind)}
        subtitle={resource.server_name ?? "Dokploy server"}
        description="Connect this resource to the current repository."
        enabled={resource.status === "running" || resource.status === "done"}
        selected={configure.selectedResource?.id === resource.id}
        statusLabel={dokployStatusLabel(resource.status)}
        detailLabel="Choose"
        onselect={() => configure.beginExisting(resource)}
      />
    {/each}
  </div>
{/if}
