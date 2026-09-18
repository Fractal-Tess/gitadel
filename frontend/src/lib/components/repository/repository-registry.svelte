<script lang="ts">
  import { resolve } from "$app/paths";
  import Check from "@lucide/svelte/icons/check";
  import Copy from "@lucide/svelte/icons/copy";
  import Package from "@lucide/svelte/icons/package";
  import { toast } from "svelte-sonner";

  import type { RegistryImage, RegistryReference } from "$lib/api/registry.js";
  import { copyText } from "$lib/clipboard.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import { formatDate, formatSize } from "$lib/repository/format.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();
  let copiedCommand = $state<string | null>(null);

  const images = $derived.by(() =>
    [...(repository.registry?.images ?? [])].sort((left, right) =>
      left.name.localeCompare(right.name),
    ),
  );

  function referenceDate(reference: RegistryReference): number {
    return reference.updated_at ? Date.parse(reference.updated_at) : 0;
  }

  function referenceLabel(reference: RegistryReference): string {
    return reference.tag ?? reference.digest;
  }

  function pullCommand(
    image: RegistryImage,
    reference: RegistryReference,
  ): string {
    return reference.tag
      ? `docker pull ${image.name}:${reference.tag}`
      : `docker pull ${image.name}@${reference.digest}`;
  }

  async function copyPullCommand(command: string): Promise<void> {
    try {
      await copyText(command);
      copiedCommand = command;
      toast.success("Docker pull command copied");
      window.setTimeout(() => {
        if (copiedCommand === command) copiedCommand = null;
      }, 1600);
    } catch {
      toast.error("Could not copy the Docker pull command.");
    }
  }

  function loginHref(): string {
    return `${resolve("/login")}?returnTo=${encodeURIComponent(`/${repository.namespace}/${repository.name}?view=registry`)}`;
  }
</script>

<div class="mx-auto max-w-5xl">
  <header
    class="flex flex-wrap items-start justify-between gap-4 border-b pb-4"
  >
    <div>
      <h1 class="text-xl font-semibold tracking-tight">Container registry</h1>
      <p class="mt-1 text-sm text-muted-foreground">
        Browse images published for this repository and copy pull commands.
      </p>
    </div>
    {#if repository.registry}
      <code
        class="rounded-md border bg-muted/30 px-2.5 py-1.5 text-xs text-muted-foreground"
      >
        {repository.registry.registry_host}
      </code>
    {/if}
  </header>

  {#if repository.registryLoading && !repository.registry}
    <p class="py-16 text-center text-sm text-muted-foreground">
      Loading container images…
    </p>
  {:else if repository.registryError && !repository.registry}
    <Alert.Root class="mt-6" variant="destructive">
      <Alert.Title>Container registry unavailable</Alert.Title>
      <Alert.Description>{repository.registryError}</Alert.Description>
      <Button
        variant="link"
        class="mt-2 px-0"
        onclick={() => void repository.loadView()}>Try again</Button
      >
    </Alert.Root>
  {:else if images.length}
    <div class="mt-6 flex flex-col gap-4">
      {#each images as image (image.name)}
        <Card.Root>
          <Card.Header class="gap-1">
            <Card.Title class="break-all font-mono text-base">
              {image.name}
            </Card.Title>
            <Card.Description class="mt-2 flex flex-wrap gap-x-4 gap-y-1">
              <span>{formatSize(image.size_bytes)} stored</span>
              <span>
                Latest push: {image.updated_at
                  ? formatDate(Date.parse(image.updated_at) / 1000)
                  : "Unavailable"}
              </span>
            </Card.Description>
          </Card.Header>
          <Card.Content class="pt-0">
            <div class="overflow-hidden rounded-md border">
              <div
                class="border-b bg-muted/20 px-3 py-2 text-xs font-medium text-muted-foreground"
              >
                References
              </div>
              <ul class="divide-y">
                {#each [...image.references].sort((left, right) => referenceDate(right) - referenceDate(left)) as reference (`${reference.tag ?? "digest"}:${reference.digest}`)}
                  {@const command = pullCommand(image, reference)}
                  <li
                    class="flex min-w-0 flex-wrap items-center gap-2 px-3 py-2.5"
                  >
                    <div class="min-w-0 flex-1">
                      <code
                        class="block truncate text-xs font-medium"
                        title={referenceLabel(reference)}
                      >
                        {referenceLabel(reference)}
                      </code>
                      <code
                        class="mt-1 block truncate text-[11px] text-muted-foreground"
                        title={command}
                      >
                        {command}
                      </code>
                    </div>
                    <span
                      class="hidden shrink-0 text-xs text-muted-foreground sm:inline"
                    >
                      {reference.updated_at
                        ? formatDate(Date.parse(reference.updated_at) / 1000)
                        : "Date unavailable"}
                    </span>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Copy pull command for ${referenceLabel(reference)}`}
                      title={command}
                      onclick={() => void copyPullCommand(command)}
                    >
                      {#if copiedCommand === command}
                        <Check class="size-3.5 text-emerald-500" />
                      {:else}
                        <Copy class="size-3.5" />
                      {/if}
                    </Button>
                  </li>
                {/each}
              </ul>
            </div>
          </Card.Content>
        </Card.Root>
      {/each}
    </div>
  {:else}
    <div class="py-20 text-center">
      <Package class="mx-auto size-9 text-muted-foreground" />
      <h2 class="mt-4 font-medium">No container images yet</h2>
      {#if repository.repository?.can_write}
        <p
          class="mx-auto mt-2 max-w-lg text-sm leading-6 text-muted-foreground"
        >
          Use your username and an
          <a
            class="underline underline-offset-4"
            href={resolve("/-/account/[view]", { view: "api-tokens" })}
            >API token</a
          >
          with read and write scopes to log in. Enter the token as the password. Replace
          <code>local-image:latest</code> below with your local image.
        </p>
        <div class="mx-auto mt-5 grid max-w-xl gap-2 text-left">
          <code
            class="overflow-x-auto rounded-md border bg-muted/30 px-3 py-2 text-xs"
          >
            docker login {repository.registry?.registry_host}
          </code>
          <code
            class="overflow-x-auto rounded-md border bg-muted/30 px-3 py-2 text-xs"
          >
            docker tag local-image:latest {repository.registry
              ?.image_prefix}:latest
          </code>
          <code
            class="overflow-x-auto rounded-md border bg-muted/30 px-3 py-2 text-xs"
          >
            docker push {repository.registry?.image_prefix}:latest
          </code>
        </div>
      {:else}
        <p
          class="mx-auto mt-2 max-w-md text-sm leading-6 text-muted-foreground"
        >
          This repository has not published any container images. Images will
          appear here after a repository writer pushes one.
        </p>
        {#if !repository.authStatus?.authenticated}
          <Button class="mt-5" variant="outline" href={loginHref()}
            >Sign in</Button
          >
        {/if}
      {/if}
    </div>
  {/if}
</div>
