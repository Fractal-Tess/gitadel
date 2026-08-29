<script lang="ts">
  import { untrack } from "svelte";
  import {
    ArrowLeft,
    CircleAlert,
    CircleCheck,
    ExternalLink,
    Eye,
    EyeOff,
    LoaderCircle,
    PlugZap,
    RefreshCw,
    Rocket,
    Unplug,
  } from "lucide-svelte";
  import type {
    IntegrationProvider,
    IntegrationTestResult,
    IntegrationSourceConnection,
    NamespaceIntegration,
  } from "$lib/api.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const SOURCE_POLL_INTERVAL_MS = 1_000;
  const SOURCE_POLL_INDICATOR_MS = 300;
  export type ConnectionEditorTarget = { slug: string; label: string };
  export type ConnectionEditorMode = "connection" | "source";
  export type ConnectionEditorValue = {
    target: string;
    provider: string;
    name: string;
    url: string;
    internalUrl: string;
    apiKey: string;
  };

  let {
    providers,
    targets,
    integration = null,
    initialTarget = targets[0]?.slug ?? "",
    mode = "connection",
    requireSuccessfulTest = false,
    onsave,
    oncancel,
    onreveal = null,
    ontest = null,
    onproviderchange = null,
    sourceConnection = null,
    sourceLoading = false,
    sourceWorking = false,
    sourceError = null,
    onrefreshsource = null,
    onbindsource = null,
    oncreatesource = null,
    ondisconnectsource = null,
  }: {
    providers: IntegrationProvider[];
    targets: ConnectionEditorTarget[];
    integration?: NamespaceIntegration | null;
    initialTarget?: string;
    mode?: ConnectionEditorMode;
    requireSuccessfulTest?: boolean;
    onsave: (value: ConnectionEditorValue) => Promise<void>;
    oncancel: () => void;
    onreveal?: (() => Promise<string>) | null;
    onproviderchange?: ((provider: IntegrationProvider) => void) | null;
    ontest?:
      ((value: ConnectionEditorValue) => Promise<IntegrationTestResult>) | null;
    sourceConnection?: IntegrationSourceConnection | null;
    sourceLoading?: boolean;
    sourceWorking?: boolean;
    sourceError?: string | null;
    onrefreshsource?: (() => Promise<void>) | null;
    onbindsource?: ((sourceId: string) => Promise<void>) | null;
    oncreatesource?: ((name: string) => Promise<void>) | null;
    ondisconnectsource?: (() => Promise<void>) | null;
  } = $props();

  let step = $state<"provider" | "details">(
    untrack(() => (integration ? "details" : "provider")),
  );
  let target = $state(untrack(() => initialTarget));
  let provider = $state(untrack(() => integration?.provider ?? ""));
  let name = $state(untrack(() => integration?.name ?? ""));
  let url = $state(untrack(() => integration?.url ?? ""));
  let internalUrl = $state(
    untrack(
      () => integration?.internal_url ?? globalThis.location?.origin ?? "",
    ),
  );
  let apiKey = $state("");
  let apiKeyVisible = $state(false);
  let revealing = $state(false);
  let saving = $state(false);
  let testing = $state(false);
  let error = $state<string | null>(null);
  let testResult = $state<IntegrationTestResult | null>(null);
  let creatingSource = $state(false);
  let sourceName = $state(untrack(() => app.instance?.site_name.trim() ?? ""));
  let credentialRevision = $state(0);
  let testedCredentialRevision = $state(-1);
  let sourcePollActive = $state(false);

  const selectedProvider = $derived(
    providers.find((candidate) => candidate.slug === provider) ?? null,
  );
  const instanceName = $derived(app.instance?.site_name.trim() ?? "");
  const readySources = $derived(
    sourceConnection?.sources.filter((source) => source.ready) ?? [],
  );
  const testPassed = $derived(
    testResult !== null && testedCredentialRevision === credentialRevision,
  );
  const canTest = $derived(
    integration !== null ||
      (provider.length > 0 &&
        url.trim().length > 0 &&
        apiKey.trim().length > 0),
  );
  const shouldPollSource = $derived(
    mode === "source" &&
      sourceConnection?.binding?.ready === false &&
      onrefreshsource !== null,
  );

  $effect(() => {
    const refreshSource = untrack(() => onrefreshsource);
    if (!shouldPollSource || refreshSource === null) return;

    let requestPending = false;
    let stopped = false;
    async function pollSource() {
      if (requestPending || stopped || sourceLoading || sourceWorking) return;
      requestPending = true;
      sourcePollActive = true;
      const indicatorDelay = new Promise<void>((resolve) => {
        window.setTimeout(resolve, SOURCE_POLL_INDICATOR_MS);
      });
      await Promise.allSettled([refreshSource!(), indicatorDelay]);
      requestPending = false;
      if (!stopped) sourcePollActive = false;
    }

    const interval = window.setInterval(
      () => void pollSource(),
      SOURCE_POLL_INTERVAL_MS,
    );
    return () => {
      stopped = true;
      window.clearInterval(interval);
    };
  });

  function message(caught: unknown, fallback: string) {
    return caught instanceof Error ? caught.message : fallback;
  }

  function providerLogo(slug: string) {
    return slug === "dokploy" ? "/integrations/dokploy.svg" : null;
  }

  function selectProvider(selected: IntegrationProvider) {
    provider = selected.slug;
    name = selected.name;
    credentialRevision += 1;
    testResult = null;
    step = "details";
    error = null;
    onproviderchange?.(selected);
  }

  function invalidateTest() {
    credentialRevision += 1;
    testResult = null;
  }

  async function toggleApiKey() {
    if (apiKeyVisible) {
      apiKeyVisible = false;
      return;
    }
    if (integration && !apiKey && onreveal) {
      revealing = true;
      error = null;
      try {
        apiKey = await onreveal();
      } catch (caught) {
        error = message(caught, "Could not reveal the stored API key.");
        return;
      } finally {
        revealing = false;
      }
    }
    apiKeyVisible = true;
  }

  async function submit() {
    if (!target || !provider) return;
    saving = true;
    error = null;
    try {
      await onsave({ target, provider, name, url, internalUrl, apiKey });
    } catch (caught) {
      error = message(caught, "Could not save the integration.");
    } finally {
      saving = false;
    }
  }

  async function testConnection() {
    if (!ontest) return;
    testing = true;
    error = null;
    testResult = null;
    try {
      testResult = await ontest({
        target,
        provider,
        name,
        url,
        internalUrl,
        apiKey,
      });
      testedCredentialRevision = credentialRevision;
    } catch (caught) {
      error = message(caught, "The connection test failed.");
    } finally {
      testing = false;
    }
  }
</script>

{#if mode === "connection" && step === "provider"}
  <div class="grid gap-3 sm:grid-cols-2">
    {#each providers as candidate (candidate.slug)}
      <button
        type="button"
        class="group flex min-h-40 flex-col rounded-xl border bg-card/30 p-4 text-left transition-colors hover:border-foreground/25 hover:bg-card/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        onclick={() => selectProvider(candidate)}
      >
        <span
          class="grid size-11 place-items-center rounded-xl border bg-zinc-950"
          aria-hidden="true"
        >
          {#if providerLogo(candidate.slug)}
            <img
              class="size-7 object-contain"
              src={providerLogo(candidate.slug) ?? ""}
              alt=""
            />
          {:else}
            <Rocket class="size-5 text-primary" />
          {/if}
        </span>
        <span class="mt-4 font-semibold">{candidate.name}</span>
        <span class="mt-1 text-xs leading-5 text-muted-foreground">
          {candidate.description}
        </span>
      </button>
    {/each}
  </div>
  <div class="flex justify-end pt-2">
    <Button type="button" variant="ghost" onclick={oncancel}>Cancel</Button>
  </div>
{:else}
  <form
    class="grid gap-5"
    onsubmit={(event) => {
      event.preventDefault();
      if (mode === "connection") void submit();
    }}
  >
    {#if mode === "connection"}
      {#if !integration && targets.length > 1}
        <label class="grid gap-1.5 text-sm font-medium">
          Owner
          <Select.Root type="single" bind:value={target}>
            <Select.Trigger class="w-full">
              {targets.find((candidate) => candidate.slug === target)?.label ??
                "Select owner"}
            </Select.Trigger>
            <Select.Content>
              {#each targets as candidate (candidate.slug)}
                <Select.Item value={candidate.slug}
                  >{candidate.label}</Select.Item
                >
              {/each}
            </Select.Content>
          </Select.Root>
        </label>
      {/if}

      <label class="grid gap-1.5 text-sm font-medium">
        Name
        <Input
          bind:value={name}
          maxlength={80}
          placeholder={selectedProvider?.name ?? "Production"}
          disabled={saving}
          required
        />
        <span class="text-xs font-normal text-muted-foreground">
          Use a label that distinguishes this connection, such as Production or
          EU.
        </span>
      </label>
      <label class="grid gap-1.5 text-sm font-medium">
        Integration URL
        <Input
          bind:value={url}
          oninput={invalidateTest}
          type="url"
          placeholder="https://integration.example.com"
          disabled={saving}
          required
        />
      </label>
      {#if selectedProvider?.source_required}
        <label class="grid gap-1.5 text-sm font-medium">
          Gitadel URL from Dokploy
          <Input
            bind:value={internalUrl}
            type="url"
            placeholder="http://gitadel.internal:3030"
            disabled={saving}
            required
          />
          <span class="text-xs font-normal text-muted-foreground">
            The URL or IP address Dokploy uses to reach this Gitadel instance.
            Include the http:// or https:// scheme.
          </span>
        </label>
      {/if}
      <div class="grid gap-1.5 text-sm font-medium">
        <label for="integration-connection-api-key">API key</label>
        <div class="relative">
          <Input
            id="integration-connection-api-key"
            class="pr-11"
            bind:value={apiKey}
            oninput={invalidateTest}
            type={apiKeyVisible ? "text" : "password"}
            autocomplete="new-password"
            placeholder={integration ? "••••••••••••••••" : "Paste API key"}
            disabled={saving || revealing}
            required={!integration}
          />
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            class="absolute right-1 top-1/2 -translate-y-1/2"
            disabled={saving || revealing}
            aria-label={apiKeyVisible ? "Hide API key" : "Show API key"}
            onclick={() => void toggleApiKey()}
          >
            {#if revealing}
              <LoaderCircle class="size-4 animate-spin" />
            {:else if apiKeyVisible}
              <EyeOff class="size-4" />
            {:else}
              <Eye class="size-4" />
            {/if}
          </Button>
        </div>
        <span class="text-xs font-normal text-muted-foreground">
          {integration
            ? "The stored key is masked. Reveal it or type a replacement."
            : "The key is stored server-side. Use the eye button to check it before saving."}
        </span>
      </div>
    {/if}

    {#if mode === "source" && integration && selectedProvider?.source_required}
      <section
        class="grid gap-3 border-t pt-5"
        aria-labelledby="integration-source-heading"
      >
        <div class="flex items-start justify-between gap-3">
          <div>
            <h3 id="integration-source-heading" class="text-sm font-semibold">
              Repository source
            </h3>
            <p class="mt-1 text-xs leading-5 text-muted-foreground">
              Choose the exact Dokploy Gitea provider that clones repositories
              from {instanceName}.
            </p>
          </div>
          {#if onrefreshsource}
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              disabled={sourceLoading || sourceWorking}
              aria-label="Refresh source status"
              onclick={() => void onrefreshsource?.()}
            >
              <RefreshCw
                class={`size-3.5 ${sourceLoading ? "animate-spin" : ""}`}
              />
            </Button>
          {/if}
        </div>

        {#if sourceLoading && !sourceConnection}
          <p class="flex items-center gap-2 text-xs text-muted-foreground">
            <LoaderCircle class="size-3.5 animate-spin" />Checking source
            connection…
          </p>
        {:else if sourceConnection?.binding}
          {@const binding = sourceConnection.binding}
          <div class="rounded-lg border bg-muted/20 p-3">
            <div class="flex items-start gap-2.5">
              {#if binding.ready}
                <CircleCheck class="mt-0.5 size-4 shrink-0 text-emerald-500" />
              {:else}
                <CircleAlert class="mt-0.5 size-4 shrink-0 text-amber-500" />
              {/if}
              <div class="min-w-0 flex-1">
                <p class="truncate text-sm font-medium">{binding.name}</p>
                <p class="mt-0.5 text-xs text-muted-foreground">
                  {binding.ready
                    ? `Connected and ready to clone ${instanceName} repositories.`
                    : binding.authorization_url
                      ? "Created, but Dokploy still needs your authorization."
                      : "The linked provider is unavailable in Dokploy."}
                </p>
              </div>
              <span
                class={`rounded-full border px-2 py-0.5 text-[0.68rem] font-medium ${
                  binding.ready
                    ? "border-emerald-500/30 text-emerald-500"
                    : "text-amber-500"
                }`}
              >
                {binding.ready ? "Ready" : "Action needed"}
              </span>
            </div>
            {#if !binding.ready && binding.authorization_url && shouldPollSource}
              <div
                class="mt-3 flex items-center gap-2 rounded-md border border-primary/20 bg-primary/5 px-2.5 py-2 text-xs text-muted-foreground"
              >
                <span
                  class="grid size-5 shrink-0 place-items-center rounded-full bg-primary/10"
                  aria-hidden="true"
                >
                  <LoaderCircle
                    class={`size-3.5 text-primary ${sourcePollActive ? "animate-spin" : ""}`}
                  />
                </span>
                <span aria-hidden="true">
                  {sourcePollActive
                    ? "Checking Dokploy now…"
                    : "Checking authorization every second"}
                </span>
                <span class="sr-only">
                  Gitadel checks Dokploy authorization once per second.
                </span>
              </div>
            {/if}
            <div class="mt-3 flex flex-wrap gap-2">
              {#if !binding.ready && binding.authorization_url}
                <Button
                  href={binding.authorization_url}
                  target="_blank"
                  rel="noreferrer"
                  size="sm"
                  class="gap-1.5"
                >
                  Authorize in Dokploy <ExternalLink class="size-3.5" />
                </Button>
              {/if}
              {#if ondisconnectsource}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="gap-1.5"
                  disabled={sourceWorking}
                  onclick={() => void ondisconnectsource?.()}
                >
                  <Unplug class="size-3.5" />Disconnect
                </Button>
              {/if}
            </div>
          </div>
        {:else if creatingSource}
          <div class="grid gap-4">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              class="w-fit gap-1.5 px-0 hover:bg-transparent"
              disabled={sourceWorking}
              onclick={() => (creatingSource = false)}
            >
              <ArrowLeft class="size-3.5" />Back to providers
            </Button>
            <label class="grid gap-1.5 text-sm font-medium">
              Provider name
              <Input
                bind:value={sourceName}
                maxlength={80}
                disabled={sourceWorking}
                placeholder={instanceName}
              />
            </label>
            <p class="text-xs leading-5 text-muted-foreground">
              {instanceName} creates the OAuth application and Dokploy provider. You
              only complete authorization in Dokploy.
            </p>
            <div class="flex justify-end">
              <Button
                type="button"
                disabled={!sourceName.trim() || sourceWorking}
                onclick={() => void oncreatesource?.(sourceName)}
              >
                {#if sourceWorking}<LoaderCircle
                    class="size-3.5 animate-spin"
                  />{/if}
                Create provider
              </Button>
            </div>
          </div>
        {:else}
          <div
            class="grid max-h-[60vh] gap-3 overflow-y-auto pr-1 sm:grid-cols-2"
          >
            <IntegrationAddCard
              title="New provider"
              description={`Create a Gitea provider managed by ${instanceName}.`}
              compact
              onclick={() => {
                if (!sourceWorking) creatingSource = true;
              }}
            />
            {#each readySources as source (source.id)}
              <IntegrationConnectionCard
                name={source.name}
                provider="dokploy"
                providerName="Dokploy"
                subtitle="Gitea provider"
                description={`Use this authorized provider to clone ${instanceName} repositories.`}
                enabled
                statusLabel="Ready"
                compact
                onselect={() => {
                  if (!sourceWorking) void onbindsource?.(source.id);
                }}
              />
            {/each}
          </div>
        {/if}

        {#if sourceError}
          <p
            class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-xs text-destructive"
            role="alert"
          >
            {sourceError}
          </p>
        {/if}
      </section>
    {/if}

    {#if mode === "connection" && testResult}
      <div
        class="rounded-md border border-emerald-500/25 bg-emerald-500/8 p-3 text-sm text-emerald-300"
        role="status"
      >
        <p class="flex items-center gap-2">
          <CircleCheck class="size-4 shrink-0" />
          Connection works{testResult.account
            ? ` — signed in as ${testResult.account}`
            : ""}.
        </p>
        {#each testResult.warnings as warning (warning)}
          <p class="mt-1.5 text-xs leading-5 text-amber-300">{warning}</p>
        {/each}
      </div>
    {/if}
    {#if error}
      <p
        class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
        role="alert"
      >
        {error}
      </p>
    {/if}

    {#if mode === "connection"}
      <div
        class="flex flex-col-reverse gap-2 sm:flex-row sm:items-center sm:justify-between"
      >
        <div>
          {#if ontest}
            <Button
              type="button"
              variant="outline"
              class="gap-2"
              disabled={testing || saving || !canTest}
              onclick={() => void testConnection()}
            >
              {#if testing}
                <LoaderCircle class="size-4 animate-spin" />
              {:else}
                <PlugZap class="size-4" />
              {/if}
              Test connection
            </Button>
          {:else if !integration}
            <Button
              type="button"
              variant="ghost"
              class="gap-2"
              onclick={() => (step = "provider")}
            >
              <ArrowLeft class="size-4" />Back
            </Button>
          {/if}
        </div>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="ghost" onclick={oncancel}
            >Cancel</Button
          >
          <Button
            type="submit"
            disabled={saving ||
              !target ||
              !provider ||
              (requireSuccessfulTest && !testPassed)}
          >
            {#if saving}<LoaderCircle class="size-4 animate-spin" />{/if}
            {integration
              ? "Save changes"
              : requireSuccessfulTest
                ? "Continue"
                : "Add connection"}
          </Button>
        </div>
      </div>
    {:else}
      <div class="flex justify-end">
        <Button type="button" variant="ghost" onclick={oncancel}>Close</Button>
      </div>
    {/if}
  </form>
{/if}
