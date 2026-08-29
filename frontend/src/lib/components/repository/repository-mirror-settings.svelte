<script lang="ts">
  import { onMount } from "svelte";
  import Clock3 from "@lucide/svelte/icons/clock-3";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import { toast } from "svelte-sonner";

  import {
    ApiFailure,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import {
    mirrorIdentitiesSchema,
    repositoryMirrorSchema,
    type MirrorIdentity,
    type RepositoryMirror,
  } from "$lib/api/mirrors.js";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  let mirror = $state<RepositoryMirror | null>(null);
  let identities = $state<MirrorIdentity[]>([]);
  let loading = $state(true);
  let saving = $state(false);
  let syncing = $state(false);
  let error = $state<string | null>(null);
  let conversionError = $state<string | null>(null);
  let convertDialogOpen = $state(false);
  let converting = $state(false);
  let schedule = $state("");
  let identityId = $state("");
  let pollTimer: ReturnType<typeof setTimeout> | undefined;

  const endpoint = $derived(
    `/api/v1/repositories/${encodeURIComponent(repository.namespace)}/${encodeURIComponent(repository.name)}/mirror`,
  );
  const identitiesEndpoint = $derived(
    `/api/v1/namespaces/${encodeURIComponent(repository.namespace)}/mirror-identities`,
  );
  const compatibleIdentities = $derived(
    identities.filter((identity) =>
      identity.instance_url
        ? sameOrigin(identity.instance_url, mirror?.remote_url ?? "")
        : false,
    ),
  );


  function message(caught: unknown): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : "The mirror request failed.";
  }
  const selectedIdentity = $derived(
    compatibleIdentities.find((identity) => identity.id === identityId) ?? null,
  );

  function formatTimestamp(value: string | null): string {
    if (!value) return "Never";
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(new Date(value));
  }

  function applyMirror(next: RepositoryMirror): void {
    mirror = next;
    schedule = next.schedule ?? "";
    identityId = next.identity_id ?? "";
    syncing = next.syncing;
  }

  function scheduleRefresh(): void {
    if (!mirror?.syncing) return;
    clearTimeout(pollTimer);
    pollTimer = setTimeout(() => void load(true), 2_000);
  }

  async function load(silent = false): Promise<void> {
    if (!silent) loading = true;
    try {
      const [next, loadedIdentities] = await Promise.all([
        requestJson(endpoint, repositoryMirrorSchema),
        requestJson(identitiesEndpoint, mirrorIdentitiesSchema),
      ]);
      applyMirror(next);
      identities = loadedIdentities;
      scheduleRefresh();
    } catch (caught) {
      error = message(caught);
    } finally {
      loading = false;
    }
  }

  async function save(): Promise<void> {
    if (!mirror) return;
    saving = true;
    error = null;
    try {
      const next = await requestJson(endpoint, repositoryMirrorSchema, {
        method: "PATCH",
        body: jsonBody({
          schedule: schedule || null,
          identity_id: identityId || null,
        }),
      });
      applyMirror(next);
      toast.success("Mirror settings saved.");
    } catch (caught) {
      error = message(caught);
    } finally {
      saving = false;
    }
  }

  async function syncNow(): Promise<void> {
    syncing = true;
    error = null;
    try {
      const next = await requestJson(
        `${endpoint}/sync`,
        repositoryMirrorSchema,
        { method: "POST" },
      );
      applyMirror(next);
      toast.success("Mirror synchronization started.");
      scheduleRefresh();
    } catch (caught) {
      error = message(caught);
      syncing = false;
    }
  }

  async function convertToStandard(): Promise<void> {
    if (!mirror) return;
    converting = true;
    conversionError = null;
    try {
      await requestEmpty(endpoint, { method: "DELETE" });
      repository.repository = repository.repository
        ? { ...repository.repository, mirrored: false }
        : null;
      clearTimeout(pollTimer);
      mirror = null;
      convertDialogOpen = false;
      toast.success("Mirror converted to a standard repository.");
      repository.navigate("overview");
    } catch (caught) {
      conversionError = message(caught);
    } finally {
      converting = false;
    }
  }


  function identityLabel(identity: MirrorIdentity): string {
    const provider = identity.provider
      ? identity.provider[0].toUpperCase() + identity.provider.slice(1)
      : "Token";
    return `${identity.name} · ${provider}`;
  }

  function sameOrigin(serverUrl: string, remoteUrl: string): boolean {
    try {
      const server = new URL(serverUrl);
      const remote = new URL(remoteUrl);
      return server.protocol === remote.protocol && server.host === remote.host;
    } catch {
      return false;
    }
  }

  onMount(() => {
    void load();
    return () => clearTimeout(pollTimer);
  });
</script>

<div
  class="divide-y divide-border overflow-hidden rounded-xl bg-card/20 ring-1 ring-foreground/15"
>
  <section
    class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
    aria-labelledby="repository-mirror-status-heading"
  >
    <header class="flex items-start gap-3">
      <RefreshCw
        class={`mt-0.5 size-4 shrink-0 text-muted-foreground ${mirror?.syncing ? "animate-spin" : ""}`}
      />
      <div>
        <h2 id="repository-mirror-status-heading" class="font-semibold">
          Mirror status
        </h2>
        <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
          This repository is read-only. Synchronization overwrites local refs
          with the upstream state.
        </p>
      </div>
    </header>

    {#if loading}
      <p class="text-sm text-muted-foreground">Loading mirror status…</p>
    {:else if mirror}
      <div class="grid max-w-2xl gap-4">
        {#if error}
          <p
            class="rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive"
            role="alert"
          >
            {error}
          </p>
        {/if}
        <div class="rounded-lg border bg-background/60 p-4">
          <p
            class="text-xs font-medium uppercase tracking-wide text-muted-foreground"
          >
            Upstream
          </p>
          <p class="mt-1 break-all font-mono text-sm">{mirror.remote_url}</p>
        </div>
        <dl class="grid gap-3 text-sm sm:grid-cols-2">
          <div>
            <dt class="text-muted-foreground">Last successful sync</dt>
            <dd class="mt-0.5 font-medium">
              {formatTimestamp(mirror.last_synced_at)}
            </dd>
          </div>
          <div>
            <dt class="text-muted-foreground">Next scheduled sync</dt>
            <dd class="mt-0.5 font-medium">
              {mirror.schedule
                ? formatTimestamp(mirror.next_sync_at)
                : "Manual only"}
            </dd>
          </div>
        </dl>
        {#if mirror.last_error}
          <p
            class="rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive"
          >
            <span class="font-medium">Last synchronization failed.</span>
            {mirror.last_error}
          </p>
        {/if}
        {#if mirror.metadata_error}
          <p
            class="rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive"
          >
            <span class="font-medium">GitHub metadata import failed.</span>
            {mirror.metadata_error}
          </p>
        {/if}
        <div class="flex flex-wrap items-center justify-between gap-3">
          <p class="text-xs text-muted-foreground">
            {mirror.syncing
              ? "Synchronization is running."
              : `Last attempted ${formatTimestamp(mirror.last_attempted_at)}.`}
          </p>
          <Button
            type="button"
            onclick={() => void syncNow()}
            disabled={syncing || mirror.syncing}
          >
            <RefreshCw
              class={`size-4 ${syncing || mirror.syncing ? "animate-spin" : ""}`}
            />
            {syncing || mirror.syncing ? "Syncing…" : "Sync now"}
          </Button>
        </div>
      </div>
    {:else}
      <p class="text-sm text-destructive">
        {error ?? "Mirror status unavailable."}
      </p>
    {/if}
  </section>

  {#if mirror}
    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="repository-mirror-schedule-heading"
    >
      <header class="flex items-start gap-3">
        <Clock3 class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h2 id="repository-mirror-schedule-heading" class="font-semibold">
            Schedule
          </h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Choose when Gitadel fetches upstream changes automatically.
          </p>
        </div>
      </header>

      <form
        class="grid max-w-2xl gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <Field.Field>
          <Field.Label for="mirror-sync-schedule">Synchronization</Field.Label>
          <Select.Root type="single" bind:value={schedule}>
            <Select.Trigger id="mirror-sync-schedule" class="w-full">
              {schedule === ""
                ? "Manual only"
                : schedule === "0 0 * * * *"
                  ? "Every hour"
                  : schedule === "0 0 */6 * * *"
                    ? "Every 6 hours"
                    : "Every day at 02:00 UTC"}
            </Select.Trigger>
            <Select.Content>
              <Select.Item value="">Manual only</Select.Item>
              <Select.Item value="0 0 * * * *">Every hour</Select.Item>
              <Select.Item value="0 0 */6 * * *">Every 6 hours</Select.Item>
              <Select.Item value="0 0 2 * * *"
                >Every day at 02:00 UTC</Select.Item
              >
            </Select.Content>
          </Select.Root>
        </Field.Field>
        <div class="flex justify-end">
          <Button type="submit" disabled={saving}
            >{saving ? "Saving…" : "Save schedule"}</Button
          >
        </div>
      </form>
    </section>

    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="repository-mirror-authentication-heading"
    >
      <header class="flex items-start gap-3">
        <KeyRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h2
            id="repository-mirror-authentication-heading"
            class="font-semibold"
          >
            Authentication
          </h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Choose a token identity for this git server, or leave the mirror
            unauthenticated when the upstream is public.
          </p>
        </div>
      </header>

      <form
        class="grid max-w-2xl gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <Field.Field>
          <Field.Label for="mirror-identity">Mirror identity</Field.Label>
          <Select.Root type="single" bind:value={identityId}>
            <Select.Trigger id="mirror-identity" class="w-full">
              {selectedIdentity
                ? identityLabel(selectedIdentity)
                : "Public"}
            </Select.Trigger>
            <Select.Content>
              <Select.Item value="">Public</Select.Item>
              {#each compatibleIdentities as identity (identity.id)}
                <Select.Item value={identity.id}
                  >{identityLabel(identity)}</Select.Item
                >
              {/each}
            </Select.Content>
          </Select.Root>
          {#if compatibleIdentities.length === 0}
            <Field.Description>
              No token identity for this git server is available in the
              repository namespace. Public remains available.
            </Field.Description>
          {/if}
        </Field.Field>

        <div class="flex justify-end">
          <Button type="submit" variant="outline" disabled={saving}
            >{saving ? "Saving…" : "Save authentication"}</Button
          >
        </div>
      </form>
    </section>
    {#if repository.repository?.can_manage}
      <section
        class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
        aria-labelledby="repository-mirror-conversion-heading"
      >
        <header>
          <h2 id="repository-mirror-conversion-heading" class="font-semibold">
            Convert to standard repository
          </h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Stop mirroring and make this repository writable again.
          </p>
        </header>

        <div class="max-w-2xl">
          <div
            class="rounded-lg border border-destructive/30 bg-destructive/5 p-4"
          >
            <p class="text-sm leading-5">
              Synchronization stops. Refs and imported content remain, and
              pushes become writable.
            </p>
            {#if conversionError}
              <p class="mt-3 text-sm text-destructive" role="alert">
                {conversionError}
              </p>
            {/if}
            <AlertDialog.Root bind:open={convertDialogOpen}>
              <Button
                type="button"
                class="mt-4"
                variant="destructive"
                disabled={mirror.syncing || converting}
                onclick={() => (convertDialogOpen = true)}
              >
                {mirror.syncing
                  ? "Wait for synchronization to finish"
                  : "Convert to standard"}
              </Button>
              <AlertDialog.Content>
                <AlertDialog.Header>
                  <AlertDialog.Title>
                    Convert this mirror to a standard repository?
                  </AlertDialog.Title>
                  <AlertDialog.Description>
                    Synchronization will stop. Existing refs and imported
                    content remain, and pushes become writable.
                  </AlertDialog.Description>
                </AlertDialog.Header>
                <AlertDialog.Footer>
                  <AlertDialog.Cancel disabled={converting}>
                    Cancel
                  </AlertDialog.Cancel>
                  <AlertDialog.Action
                    variant="destructive"
                    disabled={converting}
                    onclick={(event) => {
                      event.preventDefault();
                      void convertToStandard();
                    }}
                  >
                    {#if converting}<LoaderCircle
                        class="size-4 animate-spin"
                      />Converting…{:else}Convert repository{/if}
                  </AlertDialog.Action>
                </AlertDialog.Footer>
              </AlertDialog.Content>
            </AlertDialog.Root>
          </div>
        </div>
      </section>
    {/if}
  {/if}
</div>
