<script lang="ts">
  import Box from "@lucide/svelte/icons/box";
  import Layers3 from "@lucide/svelte/icons/layers-3";
  import Settings2 from "@lucide/svelte/icons/settings-2";

  import * as Breadcrumb from "$lib/components/ui/breadcrumb/index.js";
  import { Separator } from "$lib/components/ui/separator/index.js";
  import type { DokployConfigureState } from "$lib/repository/state/integrations-state.svelte.js";

  let { configure }: { configure: DokployConfigureState } = $props();
</script>

<Breadcrumb.Root>
  <Breadcrumb.List
    class="grid w-full grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto_minmax(0,1fr)] gap-0 overflow-hidden rounded-lg border bg-card"
  >
    <Breadcrumb.Item class="relative min-w-0 self-stretch">
      <Layers3
        class="pointer-events-none absolute left-3 top-1/2 z-10 size-4 -translate-y-1/2 {configure.setupStep ===
        'environments'
          ? 'text-primary'
          : 'text-muted-foreground'}"
        aria-hidden="true"
      />
      {#if configure.setupStep === "environments"}
        <Breadcrumb.Page
          class="grid h-full w-full min-w-0 gap-0.5 bg-primary/10 py-3 pr-3 pl-10 text-primary ring-1 ring-inset ring-primary/30"
        >
          <span
            class="truncate text-xs font-medium uppercase tracking-wide text-primary/80"
            >Environment</span
          >
          <span class="truncate">Choose environment</span>
        </Breadcrumb.Page>
      {:else}
        <Breadcrumb.Link
          href="#dokploy-setup"
          class="grid h-full w-full min-w-0 gap-0.5 py-3 pr-3 pl-10 hover:bg-muted/50"
          onclick={configure.showEnvironmentStep}
        >
          <span
            class="truncate text-xs font-medium uppercase tracking-wide text-muted-foreground"
            >Environment</span
          >
          <span class="truncate text-foreground">
            {configure.selectedProject?.name} / {configure.selectedEnvironment
              ?.name}
          </span>
        </Breadcrumb.Link>
      {/if}
    </Breadcrumb.Item>
    <Breadcrumb.Separator class="self-stretch">
      <Separator orientation="vertical" />
    </Breadcrumb.Separator>
    <Breadcrumb.Item class="relative min-w-0 self-stretch">
      <Box
        class="pointer-events-none absolute left-3 top-1/2 z-10 size-4 -translate-y-1/2 {configure.setupStep ===
        'resources'
          ? 'text-primary'
          : 'text-muted-foreground'}"
        aria-hidden="true"
      />
      {#if configure.setupStep === "resources"}
        <Breadcrumb.Page
          class="grid h-full w-full min-w-0 gap-0.5 bg-primary/10 py-3 pr-3 pl-10 text-primary ring-1 ring-inset ring-primary/30"
        >
          <span
            class="truncate text-xs font-medium uppercase tracking-wide text-primary/80"
            >Resource</span
          >
          <span class="truncate">Choose resource</span>
        </Breadcrumb.Page>
      {:else if configure.selectedEnvironment}
        <Breadcrumb.Link
          href="#dokploy-setup"
          class="grid h-full w-full min-w-0 gap-0.5 py-3 pr-3 pl-10 hover:bg-muted/50"
          onclick={configure.showResourceStep}
        >
          <span
            class="truncate text-xs font-medium uppercase tracking-wide text-muted-foreground"
            >Resource</span
          >
          <span class="truncate text-foreground">
            {configure.selectedResource?.name ??
              (configure.retainedConfigureStep === "kind" ||
              configure.retainedConfigureStep === "create"
                ? "New resource"
                : "Resource")}
          </span>
        </Breadcrumb.Link>
      {:else}
        <span class="grid h-full min-w-0 gap-0.5 py-3 pr-3 pl-10">
          <span class="truncate text-xs font-medium uppercase tracking-wide"
            >Resource</span
          >
          <span class="truncate">Choose next</span>
        </span>
      {/if}
    </Breadcrumb.Item>
    <Breadcrumb.Separator class="self-stretch">
      <Separator orientation="vertical" />
    </Breadcrumb.Separator>
    <Breadcrumb.Item class="relative min-w-0 self-stretch">
      <Settings2
        class="pointer-events-none absolute left-3 top-1/2 z-10 size-4 -translate-y-1/2 {configure.setupStep !==
          'environments' && configure.setupStep !== 'resources'
          ? 'text-primary'
          : 'text-muted-foreground'}"
        aria-hidden="true"
      />
      {#if configure.setupStep !== "environments" && configure.setupStep !== "resources"}
        <Breadcrumb.Page
          class="grid h-full w-full min-w-0 gap-0.5 bg-primary/10 py-3 pr-3 pl-10 text-primary ring-1 ring-inset ring-primary/30"
        >
          <span
            class="truncate text-xs font-medium uppercase tracking-wide text-primary/80"
            >Configure</span
          >
          <span class="truncate">
            {configure.setupStep === "kind"
              ? "Choose resource type"
              : configure.setupStep === "create"
                ? "Create resource"
                : "Connect resource"}
          </span>
        </Breadcrumb.Page>
      {:else if configure.retainedConfigureStep}
        <Breadcrumb.Link
          href="#dokploy-setup"
          class="grid h-full w-full min-w-0 gap-0.5 py-3 pr-3 pl-10 hover:bg-muted/50"
          onclick={configure.showConfigureStep}
        >
          <span
            class="truncate text-xs font-medium uppercase tracking-wide text-muted-foreground"
            >Configure</span
          >
          <span class="truncate text-foreground">
            {configure.retainedConfigureStep === "kind"
              ? "Choose resource type"
              : configure.retainedConfigureStep === "create"
                ? "Create resource"
                : "Connect resource"}
          </span>
        </Breadcrumb.Link>
      {:else}
        <span class="grid h-full min-w-0 gap-0.5 py-3 pr-3 pl-10">
          <span class="truncate text-xs font-medium uppercase tracking-wide"
            >Configure</span
          >
          <span class="truncate">Final step</span>
        </span>
      {/if}
    </Breadcrumb.Item>
  </Breadcrumb.List>
</Breadcrumb.Root>
