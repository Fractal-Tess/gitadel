<script lang="ts">
  import AppWindow from "@lucide/svelte/icons/app-window";
  import Box from "@lucide/svelte/icons/box";
  import Rocket from "@lucide/svelte/icons/rocket";
  import Unlink from "@lucide/svelte/icons/unlink";
  import SiDocker from "@icons-pack/svelte-simple-icons/icons/SiDocker";

  import { resourceKindLabel } from "$lib/api/integrations.js";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import {
    DOKPLOY_SERVER,
    type DokployConfigureState,
  } from "$lib/repository/state/integrations-state.svelte.js";

  let { configure }: { configure: DokployConfigureState } = $props();
</script>

<div class="flex flex-wrap items-center justify-between gap-3">
  <div>
    <p class="sr-only">Step 3 of 3</p>
    <h2 class="mt-1 text-base font-semibold">
      {#if configure.setupStep === "kind"}
        Choose a resource type
      {:else if configure.setupStep === "link"}
        Connect {configure.selectedResource?.name}
      {:else}
        Create {configure.selectedKind === "application"
          ? "an Application"
          : "a Docker Compose resource"}
      {/if}
    </h2>
    <p class="mt-1 text-sm text-muted-foreground">
      {#if configure.setupStep === "kind"}
        Gitadel creates the resource and source link. Finish its build
        configuration in Dokploy.
      {:else if configure.setupStep === "link"}
        Confirm the source path Gitadel should attach when this resource has no
        source yet.
      {:else}
        Gitadel creates the resource and points it at this repository. Deploy
        on push stays off until you have checked its build settings in Dokploy.
      {/if}
    </p>
  </div>
</div>

{#if configure.catalogLoading && !configure.catalog}
  <div
    class="flex min-h-48 items-center justify-center rounded-xl border bg-card"
  >
    <Spinner class="size-5 animate-spin text-muted-foreground" />
  </div>
{:else if configure.setupStep === "kind"}
  <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
    <IntegrationConnectionCard
      compact
      name="Application"
      provider="dokploy"
      providerName="Single service"
      subtitle="Application resource"
      description="Use Dockerfile, Railpack, Nixpacks, Paketo, Heroku Buildpacks, or Static configuration in Dokploy."
      enabled
      statusLabel="New"
      detailLabel="Choose"
      onselect={() => configure.beginCreate("application")}
    >
      {#snippet icon()}
        <AppWindow class="size-6 text-primary" />
      {/snippet}
    </IntegrationConnectionCard>
    <IntegrationConnectionCard
      compact
      name="Docker Compose"
      provider="dokploy"
      providerName="Multi-container service"
      subtitle="Compose resource"
      description="Use a Compose file from this repository; configure containers and runtime settings in Dokploy."
      enabled
      statusLabel="New"
      detailLabel="Choose"
      onselect={() => configure.beginCreate("compose")}
    >
      {#snippet icon()}
        <SiDocker class="size-7 text-primary" />
      {/snippet}
    </IntegrationConnectionCard>
  </div>
{:else if configure.setupStep === "link" && configure.selectedResource}
  {@const selectedResource = configure.selectedResource}
  <form
    class="max-w-2xl space-y-5 rounded-xl border bg-card p-5 shadow-sm"
    onsubmit={(event) => {
      event.preventDefault();
      void configure.linkExisting();
    }}
  >
    <div class="flex items-start gap-4">
      <span
        class="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
      >
        <Box class="size-5" />
      </span>
      <div>
        <p class="font-semibold">{selectedResource.name}</p>
        <p class="text-sm text-muted-foreground">
          {resourceKindLabel(selectedResource.kind)} in {configure
            .selectedEnvironment?.name}
        </p>
      </div>
    </div>
    <div class="grid gap-4 sm:grid-cols-2">
      <label class="grid gap-2 text-sm font-medium">
        Branch
        <Input bind:value={configure.branch} required />
      </label>
      {#if selectedResource.kind === "application"}
        <label class="grid gap-2 text-sm font-medium">
          Build path
          <Input bind:value={configure.repositoryPath} required />
        </label>
      {:else}
        <label class="grid gap-2 text-sm font-medium">
          Compose file
          <Input bind:value={configure.composePath} required />
        </label>
      {/if}
    </div>
    <p class="text-xs leading-5 text-muted-foreground">
      If this resource already uses this repository, Gitadel preserves its
      current source settings. A resource using another source will be rejected.
    </p>
    <div class="flex justify-end">
      <Button type="submit" class="gap-2" disabled={configure.working}>
        {#if configure.working}<Spinner
            class="size-4 animate-spin"
          />{:else}<Unlink data-icon="inline-start" class="rotate-45" />{/if}
        Connect resource
      </Button>
    </div>
  </form>
{:else if configure.catalog && configure.setupStep === "create" && configure.selectedEnvironment}
  {@const catalog = configure.catalog}
  <form
    class="max-w-2xl space-y-5 rounded-xl border bg-card p-5 shadow-sm"
    onsubmit={(event) => {
      event.preventDefault();
      void configure.createResource();
    }}
  >
    <div class="grid gap-4 sm:grid-cols-2">
      <label class="grid gap-2 text-sm font-medium">
        Name
        <Input bind:value={configure.resourceName} required />
      </label>
      <label class="grid gap-2 text-sm font-medium">
        Branch
        <Input bind:value={configure.branch} required />
      </label>
      {#if configure.selectedKind === "compose"}
        <label class="grid gap-2 text-sm font-medium">
          Compose file
          <Input bind:value={configure.composePath} required />
        </label>
      {:else}
        <label class="grid gap-2 text-sm font-medium">
          Build path
          <Input bind:value={configure.repositoryPath} required />
          <span class="text-xs font-normal text-muted-foreground">
            The directory Dokploy builds from. Use a subdirectory such as
            /web for a monorepo.
          </span>
        </label>
      {/if}
      {#if catalog.servers.length > 0}
        <Field.Field>
          <Field.Label for="dokploy-deployment-server">
            Deployment server
          </Field.Label>
          <Select.Root type="single" bind:value={configure.serverId}>
            <Select.Trigger id="dokploy-deployment-server" class="w-full">
              {configure.serverId === DOKPLOY_SERVER
                ? "Dokploy server"
                : (catalog.servers.find(
                    (server) => server.id === configure.serverId,
                  )?.name ?? "Select a server")}
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
    </div>
    <div
      class="rounded-lg border bg-muted/30 p-3 text-xs leading-5 text-muted-foreground"
    >
      <p class="font-medium text-foreground">After this step</p>
      <ol class="mt-1 list-decimal space-y-0.5 pl-4">
        <li>Open the resource in Dokploy and choose its build type, domain, and variables.</li>
        <li>Deploy it once from here or from Dokploy.</li>
        <li>Turn on Deploy on push so every push to {configure.branch || "the branch"} redeploys it.</li>
      </ol>
    </div>
    <div class="flex justify-end">
      <Button type="submit" class="gap-2" disabled={configure.working}>
        {#if configure.working}<Spinner
            class="size-4 animate-spin"
          />{:else}<Rocket data-icon="inline-start" />{/if}
        Create resource
      </Button>
    </div>
  </form>
{/if}
