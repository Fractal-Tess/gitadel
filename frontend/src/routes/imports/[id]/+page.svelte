<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { page } from "$app/state";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import CheckCircle2 from "@lucide/svelte/icons/check-circle-2";
  import CircleDashed from "@lucide/svelte/icons/circle-dashed";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import XCircle from "@lucide/svelte/icons/x-circle";
  import { toast } from "svelte-sonner";
  import { Spinner } from "$lib/components/ui/spinner/index.js";

  import {
    ApiFailure,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import {
    repositoryImportSchema,
    type RepositoryImport,
    type RepositoryImportItem,
  } from "$lib/api/imports.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const id = $derived(page.params.id ?? "");
  let batch = $state.raw<RepositoryImport | null>(null);
  let loading = $state(true);
  let working = $state(false);
  let token = $state("");
  let loadError = $state<string | null>(null);
  let pollTimer: ReturnType<typeof setTimeout> | undefined;

  const active = $derived(
    batch?.state === "queued" || batch?.state === "running",
  );
  const failedCount = $derived(
    batch?.items.filter(
      (item) =>
        item.state === "failed" || item.state === "credentials_required",
    ).length ?? 0,
  );
  const completedCount = $derived(
    batch?.items.filter((item) => item.state === "completed").length ?? 0,
  );

  $effect(() => {
    id;
    void load();
  });

  $effect(() => {
    clearTimeout(pollTimer);
    if (active) pollTimer = setTimeout(() => void load(true), 1_500);
    return () => clearTimeout(pollTimer);
  });

  function message(caught: unknown, fallback: string): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  async function load(background = false): Promise<void> {
    if (!id) return;
    if (!background) loading = true;
    try {
      batch = await requestJson(
        `/api/v1/repository-imports/${encodeURIComponent(id)}`,
        repositoryImportSchema,
      );
      loadError = null;
    } catch (caught) {
      if (!background) {
        loadError = message(caught, "Could not load this import.");
      }
    } finally {
      if (!background) loading = false;
    }
  }

  async function retry(): Promise<void> {
    if (!token.trim()) return;
    working = true;
    loadError = null;
    try {
      batch = await requestJson(
        `/api/v1/repository-imports/${encodeURIComponent(id)}/retry`,
        repositoryImportSchema,
        { method: "POST", body: jsonBody({ token }) },
      );
      token = "";
    } catch (caught) {
      toast.error(message(caught, "Could not retry this import."));
    } finally {
      working = false;
    }
  }

  async function cancel(): Promise<void> {
    working = true;
    loadError = null;
    try {
      await requestEmpty(
        `/api/v1/repository-imports/${encodeURIComponent(id)}`,
        { method: "DELETE" },
      );
      await load(true);
    } catch (caught) {
      toast.error(
        message(caught, "Could not cancel queued repositories."),
      );
    } finally {
      working = false;
    }
  }

  function statusLabel(item: RepositoryImportItem): string {
    switch (item.state) {
      case "queued":
        return "Queued";
      case "cloning":
        return "Cloning Git and LFS";
      case "metadata":
        return "Importing releases and labels";
      case "completed":
        return "Imported";
      case "failed":
        return "Failed";
      case "cancelled":
        return "Cancelled";
      case "credentials_required":
        return "Reconnect required";
    }
  }

  function statusClass(item: RepositoryImportItem): string {
    return item.state === "completed"
      ? "text-emerald-600 dark:text-emerald-400"
      : item.state === "failed" || item.state === "credentials_required"
        ? "text-destructive"
        : "text-muted-foreground";
  }
</script>

<svelte:head>
  <title>Repository import · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<div class="mx-auto max-w-5xl px-5 py-8 lg:px-8">
  <div class="mb-7 flex items-start gap-3">
    <Button
      size="icon"
      variant="ghost"
      aria-label="Back to repositories"
      onclick={() => void goto(resolve("/"))}
    >
      <ArrowLeft class="size-4" />
    </Button>
    <div class="min-w-0 flex-1">
      <h1 class="text-2xl font-semibold tracking-tight">Repository import</h1>
      {#if batch}
        <p class="mt-1 text-sm text-muted-foreground">
          {batch.provider} · {completedCount} of {batch.items.length} complete
        </p>
      {/if}
    </div>
    {#if active}
      <Button
        variant="outline"
        disabled={working}
        onclick={() => void cancel()}
      >
        Cancel queued
      </Button>
    {/if}
  </div>

  {#if loadError}
    <Alert.Root class="mb-5" variant="destructive">
      <Alert.Title>Import unavailable</Alert.Title>
      <Alert.Description>{loadError}</Alert.Description>
    </Alert.Root>
  {/if}

  {#if loading}
    <div class="grid place-items-center py-20 text-sm text-muted-foreground">
      <Spinner class="mb-3 size-5 animate-spin" />Loading import…
    </div>
  {:else if batch}
    <section class="overflow-hidden rounded-xl border bg-card/20">
      <div
        class="flex flex-wrap items-center justify-between gap-3 border-b px-5 py-4"
      >
        <div>
          <p class="font-medium capitalize">
            {batch.state.replaceAll("_", " ")}
          </p>
          <p class="mt-1 text-xs text-muted-foreground">{batch.instance_url}</p>
        </div>
        {#if active}
          <span
            class="inline-flex items-center gap-2 text-sm text-muted-foreground"
          >
            <Spinner class="size-4 animate-spin" />Importing…
          </span>
        {:else}
          <Button
            variant="outline"
            size="sm"
            onclick={() => void load()}
          >
            <RefreshCw data-icon="inline-start" />Refresh
          </Button>
        {/if}
      </div>
      <div class="divide-y">
        {#each batch.items as item (item.id)}
          <div
            class="grid gap-3 px-5 py-4 sm:grid-cols-[auto_minmax(0,1fr)_auto] sm:items-center"
          >
            {#if item.state === "completed"}
              <CheckCircle2
                class="size-5 text-emerald-500"
                aria-hidden="true"
              />
            {:else if item.state === "failed" || item.state === "credentials_required"}
              <XCircle class="size-5 text-destructive" aria-hidden="true" />
            {:else if item.state === "cloning" || item.state === "metadata"}
              <Spinner class="size-5 animate-spin text-primary"
              aria-hidden="true" />
            {:else}
              <CircleDashed
                class="size-5 text-muted-foreground"
                aria-hidden="true"
              />
            {/if}
            <div class="min-w-0">
              <p class="truncate font-medium">{item.source_full_name}</p>
              <p class="mt-0.5 truncate text-xs text-muted-foreground">
                {item.target_namespace}/{item.target_name}
                {#if item.last_error}
                  · {item.last_error}{/if}
              </p>
            </div>
            <div class="flex items-center justify-between gap-3 sm:justify-end">
              <span class={["text-sm", statusClass(item)]}
                >{statusLabel(item)}</span
              >
              {#if item.state === "completed"}
                <Button
                  size="icon"
                  variant="ghost"
                  aria-label={`Open ${item.target_name}`}
                  onclick={() =>
                    void goto(
                      resolve("/[namespace]/[name]", {
                        namespace: item.target_namespace,
                        name: item.target_name,
                      }),
                    )}
                >
                  <ExternalLink class="size-4" />
                </Button>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    </section>

    {#if failedCount > 0 && !active}
      <form
        class="mt-5"
        onsubmit={(event) => {
          event.preventDefault();
          void retry();
        }}
      >
        <Card.Root>
          <Card.Header>
            <Card.Title>Retry {failedCount} failed repositories</Card.Title>
            <Card.Description>
              Reconnect the source token. It is used for this retry and is not
              saved.
            </Card.Description>
          </Card.Header>
          <Card.Content class="flex flex-col gap-3 sm:flex-row sm:items-end">
          <Field.Field class="flex-1">
            <Field.Label for="retry-import-token">Access token</Field.Label>
            <Input
              id="retry-import-token"
              type="password"
              bind:value={token}
              autocomplete="off"
              required
            />
          </Field.Field>
          <Button type="submit" disabled={working || !token.trim()}>
            {#if working}<Spinner class="size-4 animate-spin" />{/if}
            Retry failed
          </Button>
          </Card.Content>
        </Card.Root>
      </form>
    {/if}
  {/if}
</div>
