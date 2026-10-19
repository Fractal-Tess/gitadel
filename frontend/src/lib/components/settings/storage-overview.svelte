<script lang="ts">
  import Cloud from "@lucide/svelte/icons/cloud";
  import Gauge from "@lucide/svelte/icons/gauge";
  import HardDrive from "@lucide/svelte/icons/hard-drive";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";

  import type {
    StorageCategory,
    StorageOverview,
    StorageVolume,
  } from "$lib/api/storage.js";
  import { errorMessage } from "$lib/api/transport.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import SettingsSectionHeader from "$lib/components/settings/settings-section-header.svelte";
  import { formatBytes } from "$lib/format.js";
  import {
    loadStorageOverview,
    peekStorageOverview,
    refreshStorageOverview,
  } from "$lib/settings/settings-data-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  let overview = $state.raw<StorageOverview | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // One colour per kind of data, the same on every volume.
  const colors: Record<string, string> = {
    repositories: "bg-chart-3",
    database: "bg-chart-2",
    lfs: "bg-chart-1",
    registry: "bg-chart-4",
    artifacts: "bg-chart-5",
  };

  const remote = $derived(
    overview?.categories.filter((category) => !category.volume_id) ?? [],
  );

  $effect(() => {
    const scope = app.authorizationScope;
    const cached = peekStorageOverview(scope);
    if (cached) {
      overview = cached;
      return;
    }
    void load(() => loadStorageOverview(scope), scope);
  });

  async function load(
    request: () => Promise<StorageOverview>,
    scope = app.authorizationScope,
  ) {
    loading = true;
    error = null;
    try {
      const loaded = await request();
      if (app.authorizationScope === scope) overview = loaded;
    } catch (caught) {
      if (app.authorizationScope === scope) error = errorMessage(caught);
    } finally {
      if (app.authorizationScope === scope) loading = false;
    }
  }

  function categoriesOn(volume: StorageVolume): StorageCategory[] {
    return (overview?.categories ?? []).filter(
      (category) => category.volume_id === volume.id,
    );
  }

  function percent(volume: StorageVolume, bytes: number) {
    return volume.total_bytes > 0 ? (bytes / volume.total_bytes) * 100 : 0;
  }

  // Gitadel's own files are part of the volume's used space; whatever is
  // left of it belongs to something else on the same disk.
  function otherBytes(volume: StorageVolume) {
    return Math.max(volume.used_bytes - volume.gitadel_bytes, 0);
  }

  function share(volume: StorageVolume, bytes: number) {
    const value = percent(volume, bytes);
    return value > 0 && value < 1 ? "<1%" : `${Math.round(value)}%`;
  }
</script>

<section class="flex flex-col gap-4" aria-labelledby="storage-overview-title">
  <SettingsSectionHeader
    id="storage-overview-title"
    icon={Gauge}
    title="Disk usage"
    description="Every disk Gitadel writes to, and how much of it Gitadel uses."
  >
    {#snippet actions()}
      <Button
        variant="outline"
        size="sm"
        disabled={loading}
        onclick={() =>
          void load(() => refreshStorageOverview(app.authorizationScope))}
      >
        {#if loading}
          <Spinner class="size-4" data-icon="inline-start" />
        {:else}
          <RefreshCw data-icon="inline-start" />
        {/if}
        Refresh
      </Button>
    {/snippet}
  </SettingsSectionHeader>

  {#if error}
    <Alert.Root variant="destructive">
      <TriangleAlert class="size-4" />
      <Alert.Title>Could not measure storage</Alert.Title>
      <Alert.Description>{error}</Alert.Description>
    </Alert.Root>
  {:else if !overview}
    <div
      class="flex items-center gap-2 rounded-xl border p-5 text-sm text-muted-foreground"
    >
      <Spinner class="size-4" /> Measuring disk usage…
    </div>
  {:else}
    {#each overview.volumes as volume (volume.id)}
      {@const other = otherBytes(volume)}
      <article class="rounded-xl bg-card p-5 ring-1 ring-foreground/10">
        <header class="flex flex-wrap items-baseline justify-between gap-2">
          <div class="flex items-center gap-2">
            <HardDrive class="size-4 text-muted-foreground" />
            <h3 class="font-mono text-sm font-medium">
              {volume.mount_point ?? "Local disk"}
            </h3>
          </div>
          <p class="text-sm text-muted-foreground">
            <span class="font-medium text-foreground tabular-nums">
              {formatBytes(volume.available_bytes)}
            </span>
            free of {formatBytes(volume.total_bytes)}
          </p>
        </header>

        <div
          class="mt-4 flex h-3 overflow-hidden rounded-full bg-muted"
          role="img"
          aria-label={`${formatBytes(volume.gitadel_bytes)} used by Gitadel, ${formatBytes(other)} used by other files, ${formatBytes(volume.available_bytes)} free of ${formatBytes(volume.total_bytes)}`}
        >
          {#each categoriesOn(volume) as category (category.key + category.location)}
            <div
              class={colors[category.key] ?? "bg-primary"}
              style:width={`${percent(volume, category.bytes)}%`}
            ></div>
          {/each}
          <div
            class="bg-foreground/20"
            style:width={`${percent(volume, other)}%`}
          ></div>
          {#if volume.reserved_bytes > 0}
            <div
              class="bg-foreground/8"
              style:width={`${percent(volume, volume.reserved_bytes)}%`}
            ></div>
          {/if}
        </div>

        <dl class="mt-4 grid grid-cols-2 gap-4 sm:grid-cols-4">
          <div>
            <dt class="text-xs text-muted-foreground">Used by Gitadel</dt>
            <dd class="mt-0.5 text-sm font-medium tabular-nums">
              {formatBytes(volume.gitadel_bytes)}
              <span class="font-normal text-muted-foreground">
                · {share(volume, volume.gitadel_bytes)}
              </span>
            </dd>
          </div>
          <div>
            <dt class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <span class="size-2 rounded-full bg-foreground/20"></span>
              Other files
            </dt>
            <dd class="mt-0.5 text-sm font-medium tabular-nums">
              {formatBytes(other)}
              <span class="font-normal text-muted-foreground">
                · {share(volume, other)}
              </span>
            </dd>
          </div>
          <div>
            <dt class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <span class="size-2 rounded-full border bg-muted"></span>
              Free
            </dt>
            <dd class="mt-0.5 text-sm font-medium tabular-nums">
              {formatBytes(volume.available_bytes)}
              <span class="font-normal text-muted-foreground">
                · {share(volume, volume.available_bytes)}
              </span>
            </dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">Total size</dt>
            <dd class="mt-0.5 text-sm font-medium tabular-nums">
              {formatBytes(volume.total_bytes)}
            </dd>
          </div>
        </dl>
        {#if volume.reserved_bytes > 0}
          <p class="mt-2 text-xs text-muted-foreground">
            The filesystem keeps {formatBytes(volume.reserved_bytes)} in reserve for
            the system, so it is neither used nor free.
          </p>
        {/if}

        <ul class="mt-4 divide-y border-t">
          {#each categoriesOn(volume) as category (category.key + category.location)}
            <li class="flex items-center gap-3 py-2.5 text-sm">
              <span
                class={[
                  "size-2.5 shrink-0 rounded-full",
                  colors[category.key] ?? "bg-primary",
                ]}
              ></span>
              <span class="w-40 shrink-0 font-medium">{category.label}</span>
              <span
                class="min-w-0 flex-1 font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]"
                title={category.location}>{category.location}</span
              >
              <span class="shrink-0 tabular-nums">
                {formatBytes(category.bytes)}
              </span>
            </li>
          {/each}
        </ul>
      </article>
    {/each}

    {#if remote.length > 0}
      <article class="rounded-xl bg-card p-5 ring-1 ring-foreground/10">
        <header class="flex items-center gap-2">
          <Cloud class="size-4 text-muted-foreground" />
          <h3 class="text-sm font-medium">Outside this server</h3>
        </header>
        <p class="mt-1 text-xs text-muted-foreground">
          Object storage and external databases report what Gitadel stores, but
          not how much room is left.
        </p>
        <ul class="mt-3 divide-y border-t">
          {#each remote as category (category.key + category.location)}
            <li class="flex items-center gap-3 py-2.5 text-sm">
              <span
                class={[
                  "size-2.5 shrink-0 rounded-full",
                  colors[category.key] ?? "bg-primary",
                ]}
              ></span>
              <span class="w-40 shrink-0 font-medium">{category.label}</span>
              <span
                class="min-w-0 flex-1 font-mono text-xs text-muted-foreground [overflow-wrap:anywhere]"
                title={category.location}>{category.location}</span
              >
              <span class="shrink-0 tabular-nums">
                {formatBytes(category.bytes)}
              </span>
            </li>
          {/each}
        </ul>
      </article>
    {/if}
  {/if}
</section>
