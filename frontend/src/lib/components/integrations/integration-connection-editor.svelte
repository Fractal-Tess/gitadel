<script lang="ts">
  import { untrack } from "svelte";
  import { toast } from "svelte-sonner";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import CircleCheck from "@lucide/svelte/icons/circle-check";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import Eye from "@lucide/svelte/icons/eye";
  import EyeOff from "@lucide/svelte/icons/eye-off";
  import PlugZap from "@lucide/svelte/icons/plug-zap";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Rocket from "@lucide/svelte/icons/rocket";
  import Unplug from "@lucide/svelte/icons/unplug";
  import { errorMessage } from "$lib/api/transport.js";
  import {
    sourceSetupStep,
    type IntegrationProvider,
    type IntegrationSourceConnection,
    type IntegrationTestResult,
    type NamespaceIntegration,
    type SourceCheck,
  } from "$lib/api/integrations.js";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
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
    sourceCheck = null,
    sourceChecking = false,
    onrefreshsource = null,
    onbindsource = null,
    oncreatesource = null,
    ondisconnectsource = null,
    onverifysource = null,
    onsetinternalurl = null,
    oncontinue = null,
    continueLabel = "Continue",
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
      | ((value: ConnectionEditorValue) => Promise<IntegrationTestResult>)
      | null;
    sourceConnection?: IntegrationSourceConnection | null;
    sourceLoading?: boolean;
    sourceWorking?: boolean;
    sourceError?: string | null;
    /** The latest reachability check; null until one has run. */
    sourceCheck?: SourceCheck | null;
    sourceChecking?: boolean;
    onrefreshsource?: (() => Promise<void>) | null;
    onbindsource?: ((sourceId: string) => Promise<void>) | null;
    oncreatesource?: ((name: string) => Promise<void>) | null;
    ondisconnectsource?: (() => Promise<void>) | null;
    onverifysource?: (() => Promise<void>) | null;
    onsetinternalurl?: ((internalUrl: string) => Promise<void>) | null;
    /** Shown on the last step instead of a plain Done button. */
    oncontinue?: (() => void) | null;
    continueLabel?: string;
  } = $props();

  let view = $state<ConnectionEditorMode>(untrack(() => mode));
  let step = $state<"provider" | "details">(
    untrack(() => (integration ? "details" : "provider")),
  );
  let target = $state(untrack(() => initialTarget));
  let provider = $state(untrack(() => integration?.provider ?? ""));
  let name = $state(untrack(() => integration?.name ?? ""));
  let url = $state(untrack(() => integration?.url ?? ""));
  // New connections send this page's origin; Gitadel tries other addresses
  // itself when Dokploy cannot reach it.
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
  let testResult = $state<IntegrationTestResult | null>(null);
  let testError = $state<string | null>(null);
  let sourceName = $state(untrack(() => app.instance?.site_name.trim() ?? ""));
  let credentialRevision = $state(0);
  let testedCredentialRevision = $state(-1);
  let sourcePollActive = $state(false);
  // The address field starts from what the last check used.
  let address = $derived(
    sourceCheck?.internal_url ?? integration?.internal_url ?? "",
  );
  let autoVerified = false;

  const selectedProvider = $derived(
    providers.find((candidate) => candidate.slug === provider) ?? null,
  );
  const instanceName = $derived(app.instance?.site_name.trim() || "Gitadel");
  // Ready providers first; the rest stay visible with the reason they cannot
  // be used, so a provider for another forge is not silently missing.
  const listedSources = $derived(
    [...(sourceConnection?.sources ?? [])].sort(
      (left, right) => Number(right.ready) - Number(left.ready),
    ),
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
  const binding = $derived(sourceConnection?.binding ?? null);
  const setupStep = $derived(sourceSetupStep(binding, sourceCheck));
  // 0 Dokploy account, 1 connect, 2 reachability, 3 deploy a repository.
  const currentStep = $derived(
    setupStep === "connect" || setupStep === "authorize"
      ? 1
      : setupStep === "reach"
        ? 2
        : 3,
  );
  const checkBusy = $derived(sourceChecking || sourceWorking);
  const shouldPollSource = $derived(
    view === "source" &&
      binding?.ready === false &&
      binding.targets_instance &&
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

  // A bound, authorized provider opened without a check result is checked
  // once, so the reachability step reflects what Dokploy sees now.
  $effect(() => {
    if (
      view !== "source" ||
      setupStep !== "reach" ||
      sourceCheck !== null ||
      checkBusy ||
      autoVerified
    )
      return;
    const verify = untrack(() => onverifysource);
    if (!verify) return;
    autoVerified = true;
    void verify();
  });

  function providerLogo(slug: string) {
    return slug === "dokploy" ? "/integrations/dokploy.svg" : null;
  }

  function countOf(count: number, one: string, many: string) {
    return `${count} ${count === 1 ? one : many}`;
  }

  function selectProvider(selected: IntegrationProvider) {
    provider = selected.slug;
    name = selected.name;
    credentialRevision += 1;
    testResult = null;
    testError = null;
    step = "details";
    onproviderchange?.(selected);
  }

  function invalidateTest() {
    credentialRevision += 1;
    testResult = null;
    testError = null;
  }

  function cancelConnection() {
    // Editing the details of a connection opened at its setup steps returns
    // to those steps.
    if (integration && mode === "source") {
      view = "source";
      return;
    }
    oncancel();
  }

  async function toggleApiKey() {
    if (apiKeyVisible) {
      apiKeyVisible = false;
      return;
    }
    if (integration && !apiKey && onreveal) {
      revealing = true;
      try {
        apiKey = await onreveal();
      } catch (caught) {
        toast.error(
          errorMessage(caught, "Could not reveal the stored API key."),
        );
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
    try {
      await onsave({ target, provider, name, url, internalUrl, apiKey });
      if (integration && selectedProvider?.source_required) {
        autoVerified = false;
        view = "source";
      }
    } catch (caught) {
      toast.error(errorMessage(caught, "Could not save the integration."));
    } finally {
      saving = false;
    }
  }

  async function testConnection() {
    if (!ontest) return;
    testing = true;
    testResult = null;
    testError = null;
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
      testError = errorMessage(caught, "The connection test failed.");
    } finally {
      testing = false;
    }
  }

  async function saveAddress() {
    const value = address.trim();
    if (!value || !onsetinternalurl) return;
    await onsetinternalurl(value);
  }
</script>

{#snippet stepMarker(index: number, title: string)}
  {@const done = index < currentStep}
  {@const current = index === currentStep}
  <div class="flex items-start gap-2.5">
    <span
      class={[
        "mt-0.5 grid size-5 shrink-0 place-items-center rounded-full border text-[0.68rem] font-semibold",
        done && "border-emerald-500/40 bg-emerald-500/10 text-emerald-500",
        current && "border-primary/40 text-primary",
        !done && !current && "text-muted-foreground",
      ]}
      aria-hidden="true"
    >
      {#if done}<CircleCheck class="size-3.5" />{:else}{index + 1}{/if}
    </span>
    <span
      class={[
        "min-w-0 flex-1 text-sm font-medium",
        !done && !current && "text-muted-foreground",
      ]}>{title}</span
    >
    <span
      class={[
        "shrink-0 text-[0.68rem] font-medium",
        done ? "text-emerald-500" : current ? "text-primary" : "text-muted-foreground",
      ]}
    >
      {done ? "Done" : current ? "Next" : "Waiting"}
    </span>
  </div>
{/snippet}

{#if view === "connection" && step === "provider"}
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
{:else if view === "connection"}
  <form
    class="grid gap-5"
    onsubmit={(event) => {
      event.preventDefault();
      void submit();
    }}
  >
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
              <Select.Item value={candidate.slug}>{candidate.label}</Select.Item
              >
            {/each}
          </Select.Content>
        </Select.Root>
      </label>
    {/if}

    {#if selectedProvider?.source_required && !integration}
      <p class="text-xs leading-5 text-muted-foreground">
        Step 1 of 4: sign in to Dokploy. Next, Gitadel connects itself to
        Dokploy and checks that Dokploy can reach it.
      </p>
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
      {selectedProvider?.name ?? "Integration"} URL
      <Input
        bind:value={url}
        oninput={invalidateTest}
        type="url"
        placeholder="https://dokploy.example.com"
        disabled={saving}
        required
      />
      <span class="text-xs font-normal text-muted-foreground">
        The address you open {selectedProvider?.name ?? "the service"} at in your
        browser.
      </span>
    </label>
    {#if selectedProvider?.source_required && integration}
      <label class="grid gap-1.5 text-sm font-medium">
        Address Dokploy uses to reach Gitadel
        <Input
          bind:value={internalUrl}
          type="url"
          placeholder="http://gitadel.internal:3030"
          disabled={saving}
          required
        />
        <span class="text-xs font-normal text-muted-foreground">
          Include http:// or https://. After saving, Gitadel checks the address
          again.
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
            <Spinner class="size-4" />
          {:else if apiKeyVisible}
            <EyeOff class="size-4" />
          {:else}
            <Eye class="size-4" />
          {/if}
        </Button>
      </div>
      <span class="text-xs font-normal text-muted-foreground">
        {#if integration}
          The stored key is masked. Reveal it or type a replacement.
        {:else if selectedProvider?.slug === "dokploy"}
          Create one in Dokploy under Settings → Profile → API/CLI. It is stored
          encrypted on the server.
        {:else}
          The key is stored encrypted on the server.
        {/if}
      </span>
    </div>

    <div aria-live="polite">
      {#if testPassed && testResult}
        <Alert.Root>
          <CircleCheck class="size-4 shrink-0 text-emerald-500" />
          <Alert.Title>
            Connection works{testResult.account
              ? ` — signed in as ${testResult.account}`
              : ""}
          </Alert.Title>
          {#if testResult.warnings.length > 0}
            <Alert.Description
              >{testResult.warnings.join(" ")}</Alert.Description
            >
          {/if}
        </Alert.Root>
      {:else if testError}
        <Alert.Root variant="destructive">
          <CircleAlert class="size-4 shrink-0" />
          <Alert.Title>The connection test failed</Alert.Title>
          <Alert.Description>{testError}</Alert.Description>
        </Alert.Root>
      {/if}
    </div>

    {#if requireSuccessfulTest && !testPassed}
      <p class="text-xs text-muted-foreground">
        Test the connection to continue. Changing the URL or API key requires a
        new test.
      </p>
    {/if}
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
              <Spinner class="size-4" data-icon="inline-start" />
            {:else}
              <PlugZap class="size-4" data-icon="inline-start" />
            {/if}
            Test connection
          </Button>
        {:else if !integration}
          <Button
            type="button"
            variant="ghost"
            onclick={() => (step = "provider")}
          >
            <ArrowLeft data-icon="inline-start" />Back
          </Button>
        {/if}
      </div>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="ghost" onclick={cancelConnection}>
          {integration && mode === "source" ? "Back" : "Cancel"}
        </Button>
        <Button
          type="submit"
          disabled={saving ||
            !target ||
            !provider ||
            (requireSuccessfulTest && !testPassed)}
        >
          {#if saving}<Spinner class="size-4" data-icon="inline-start" />{/if}
          {integration
            ? "Save changes"
            : requireSuccessfulTest
              ? "Continue"
              : "Add connection"}
        </Button>
      </div>
    </div>
  </form>
{:else if integration}
  <section class="grid gap-4" aria-labelledby="integration-source-heading">
    <div class="flex items-start justify-between gap-3">
      <div>
        <h3 id="integration-source-heading" class="text-sm font-semibold">
          Connect {integration.name}
        </h3>
        <p class="mt-1 text-xs leading-5 text-muted-foreground">
          Dokploy deploys repositories from {instanceName}. A step is marked
          done only after Gitadel has confirmed it with Dokploy.
        </p>
      </div>
      {#if onrefreshsource}
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          disabled={sourceLoading || sourceWorking}
          aria-label="Refresh connection status"
          onclick={() => void onrefreshsource?.()}
        >
          <RefreshCw class={`size-3.5 ${sourceLoading ? "animate-spin" : ""}`} />
        </Button>
      {/if}
    </div>

    {#if sourceLoading && !sourceConnection}
      <p
        class="flex items-center gap-2 text-xs text-muted-foreground"
        role="status"
      >
        <Spinner class="size-3.5" />Asking Dokploy how this connection is set
        up…
      </p>
    {:else}
      <ol class="grid gap-2" aria-label="Connection setup steps">
        <!-- Step 1: the Dokploy account; the connection exists, so it works. -->
        <li class="grid gap-2 rounded-lg border bg-muted/10 px-3 py-2.5">
          {@render stepMarker(0, "Dokploy account")}
          <div
            class="flex items-center justify-between gap-2 pl-7.5 text-xs text-muted-foreground"
          >
            <span class="min-w-0 truncate">Signed in to {integration.url}</span>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              class="h-7 shrink-0 px-2 text-xs"
              disabled={sourceWorking}
              onclick={() => (view = "connection")}
            >
              Edit details
            </Button>
          </div>
        </li>

        <!-- Step 2: a provider Dokploy clones from, holding a token. -->
        <li
          class={[
            "grid gap-3 rounded-lg border px-3 py-2.5",
            currentStep === 1 ? "border-primary/40 bg-primary/5" : "bg-muted/10",
          ]}
          aria-current={currentStep === 1 ? "step" : undefined}
        >
          {@render stepMarker(1, `Connect ${instanceName} to Dokploy`)}
          <div class="grid gap-3 pl-7.5">
            {#if currentStep > 1 && binding}
              <div
                class="flex items-center justify-between gap-2 text-xs text-muted-foreground"
              >
                <span class="min-w-0">
                  Dokploy clones through the Gitea provider
                  <span class="font-medium text-foreground">{binding.name}</span
                  >{binding.managed ? `, managed by ${instanceName}` : ""}.
                </span>
                {#if ondisconnectsource}
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    class="h-7 shrink-0 gap-1.5 px-2 text-xs"
                    disabled={sourceWorking || sourceChecking}
                    onclick={() => void ondisconnectsource?.()}
                  >
                    <Unplug class="size-3.5" />Disconnect
                  </Button>
                {/if}
              </div>
            {:else if binding && !binding.targets_instance}
              <Alert.Root variant="destructive">
                <CircleAlert class="size-4 shrink-0" />
                <Alert.Title>{binding.name} belongs to another forge</Alert.Title>
                <Alert.Description>
                  {binding.problem ??
                    `This provider does not point at ${instanceName}.`}
                </Alert.Description>
              </Alert.Root>
              {#if ondisconnectsource}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="w-fit gap-1.5"
                  disabled={sourceWorking}
                  onclick={() => void ondisconnectsource?.()}
                >
                  <Unplug class="size-3.5" />Disconnect and choose another
                </Button>
              {/if}
            {:else if binding && setupStep === "authorize"}
              <p class="text-xs leading-5 text-muted-foreground">
                {binding.name} was created outside {instanceName}, so Dokploy
                needs your approval. Click Authorize in Dokploy, approve access
                on the page that opens, then come back here. This page updates
                by itself.
              </p>
              {#if binding.url}
                <p class="text-xs text-muted-foreground">
                  The approval page opens at <span class="font-mono"
                    >{binding.url}</span
                  >. This browser must be able to reach that address.
                </p>
              {/if}
              {#if shouldPollSource}
                <p
                  class="flex items-center gap-2 text-xs text-muted-foreground"
                  role="status"
                >
                  <Spinner
                    class={`size-3.5 text-primary ${sourcePollActive ? "animate-spin" : ""}`}
                  />
                  Waiting for approval in Dokploy…
                </p>
              {/if}
              <div class="flex flex-wrap gap-2">
                {#if binding.authorization_url}
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
            {:else if sourceWorking}
              <p
                class="flex items-center gap-2 text-xs text-muted-foreground"
                role="status"
              >
                <Spinner class="size-3.5 animate-spin" />
                Creating the provider in Dokploy and checking it can reach {instanceName}…
              </p>
            {:else}
              <p class="text-xs leading-5 text-muted-foreground">
                {instanceName} creates a Gitea provider in Dokploy and gives it
                read access to your repositories. There is nothing to approve in
                Dokploy.
              </p>
              <form
                class="flex flex-col gap-2 sm:flex-row sm:items-end"
                onsubmit={(event) => {
                  event.preventDefault();
                  if (sourceName.trim()) void oncreatesource?.(sourceName.trim());
                }}
              >
                <label class="grid flex-1 gap-1.5 text-xs font-medium">
                  Provider name in Dokploy
                  <Input
                    bind:value={sourceName}
                    maxlength={80}
                    placeholder={instanceName}
                    required
                  />
                </label>
                <Button
                  type="submit"
                  class="gap-1.5"
                  disabled={!sourceName.trim() || !oncreatesource}
                >
                  <PlugZap class="size-4" data-icon="inline-start" />Connect
                </Button>
              </form>
              {#if listedSources.length > 0}
                <details class="group text-xs">
                  <summary
                    class="cursor-pointer text-muted-foreground hover:text-foreground"
                  >
                    Use an existing Dokploy provider
                  </summary>
                  <div
                    class="mt-3 grid max-h-[40vh] gap-3 overflow-y-auto pr-1 sm:grid-cols-2"
                  >
                    {#each listedSources as source (source.id)}
                      <IntegrationConnectionCard
                        name={source.name}
                        provider="dokploy"
                        providerName="Dokploy"
                        subtitle={source.url ?? "Gitea provider"}
                        description={source.ready
                          ? `Use this provider to clone ${instanceName} repositories.`
                          : (source.problem ?? "This provider cannot be used.")}
                        enabled={source.ready}
                        statusLabel={source.ready
                          ? "Ready"
                          : source.targets_instance
                            ? "Not authorized"
                            : "Other forge"}
                        compact
                        onselect={source.ready
                          ? () => {
                              if (!sourceWorking) void onbindsource?.(source.id);
                            }
                          : null}
                      />
                    {/each}
                  </div>
                </details>
              {/if}
            {/if}
          </div>
        </li>

        <!-- Step 3: Dokploy listed repositories through the provider. -->
        <li
          class={[
            "grid gap-3 rounded-lg border px-3 py-2.5",
            currentStep === 2 ? "border-primary/40 bg-primary/5" : "bg-muted/10",
          ]}
          aria-current={currentStep === 2 ? "step" : undefined}
        >
          {@render stepMarker(2, `Dokploy can reach ${instanceName}`)}
          {#if currentStep > 2 && sourceCheck}
            <div class="grid gap-1 pl-7.5 text-xs text-muted-foreground">
              <div class="flex items-center justify-between gap-2">
                <span class="min-w-0">
                  Dokploy reached {instanceName} at
                  <span class="font-mono break-all text-foreground"
                    >{sourceCheck.internal_url}</span
                  >{sourceCheck.repositories !== null
                    ? ` and can see ${countOf(sourceCheck.repositories, "repository", "repositories")}.`
                    : "."}
                </span>
                {#if onverifysource}
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    class="h-7 shrink-0 gap-1.5 px-2 text-xs"
                    disabled={checkBusy}
                    onclick={() => void onverifysource?.()}
                  >
                    {#if sourceChecking}<Spinner
                        class="size-3 animate-spin"
                      />{/if}Check again
                  </Button>
                {/if}
              </div>
              {#if sourceCheck.attempts.length > 0}
                <span>
                  {instanceName} switched to this address after {countOf(
                    sourceCheck.attempts.length,
                    "other address",
                    "other addresses",
                  )} failed.
                </span>
              {/if}
            </div>
          {:else if currentStep === 2}
            <div class="grid gap-3 pl-7.5">
              {#if checkBusy}
                <p
                  class="flex items-center gap-2 text-xs text-muted-foreground"
                  role="status"
                >
                  <Spinner class="size-3.5 animate-spin" />
                  Checking that Dokploy can reach {instanceName}…
                </p>
              {:else if !sourceCheck}
                <p class="text-xs text-muted-foreground">
                  Not checked yet. Gitadel asks Dokploy to list your
                  repositories through the provider.
                </p>
                {#if onverifysource}
                  <Button
                    type="button"
                    size="sm"
                    class="w-fit"
                    onclick={() => void onverifysource?.()}>Check now</Button
                  >
                {/if}
              {:else}
                <Alert.Root variant="destructive">
                  <CircleAlert class="size-4 shrink-0" />
                  <Alert.Title>Dokploy could not reach {instanceName}</Alert.Title>
                  <Alert.Description>
                    <ul class="mt-1 grid gap-1.5">
                      {#each sourceCheck.attempts as attempt, index (index)}
                        <li>
                          <span class="font-mono break-all">{attempt.url}</span>:
                          {attempt.error}
                        </li>
                      {/each}
                    </ul>
                  </Alert.Description>
                </Alert.Root>
                {#if binding?.managed && onsetinternalurl}
                  <form
                    class="grid gap-2"
                    onsubmit={(event) => {
                      event.preventDefault();
                      void saveAddress();
                    }}
                  >
                    <label class="grid gap-1.5 text-xs font-medium">
                      Address Dokploy should use to reach {instanceName}
                      <Input
                        bind:value={address}
                        type="url"
                        placeholder="http://10.0.0.5:3030"
                        required
                      />
                      <span class="font-normal text-muted-foreground">
                        An address the Dokploy server can open, such as a LAN
                        IP or internal hostname. Include http:// or https://.
                      </span>
                    </label>
                    <div class="flex flex-wrap gap-2">
                      <Button type="submit" size="sm" disabled={!address.trim()}>
                        Save and check again
                      </Button>
                      {#if onverifysource}
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          onclick={() => void onverifysource?.()}
                          >Check again</Button
                        >
                      {/if}
                    </div>
                  </form>
                {:else}
                  <p class="text-xs leading-5 text-muted-foreground">
                    {binding?.name ?? "This provider"} was created outside {instanceName}.
                    Correct its URL in Dokploy under Settings → Git, then check
                    again.
                  </p>
                  {#if onverifysource}
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      class="w-fit"
                      onclick={() => void onverifysource?.()}>Check again</Button
                    >
                  {/if}
                {/if}
              {/if}
            </div>
          {/if}
        </li>

        <!-- Step 4: what to do next once everything is confirmed. -->
        <li
          class={[
            "grid gap-3 rounded-lg border px-3 py-2.5",
            currentStep === 3 ? "border-primary/40 bg-primary/5" : "bg-muted/10",
          ]}
          aria-current={currentStep === 3 ? "step" : undefined}
        >
          {@render stepMarker(3, "Deploy a repository")}
          {#if currentStep === 3}
            <div class="grid gap-3 pl-7.5">
              <p class="text-xs leading-5 text-muted-foreground">
                {#if oncontinue}
                  Everything is connected. Continue to add this connection to the
                  repository, then set up its deployment.
                {:else}
                  Everything is connected. Open a repository, go to Settings →
                  Integrations, choose Add, and pick {integration.name}. Gitadel
                  suggests how to build it.
                {/if}
              </p>
              <Button
                type="button"
                size="sm"
                class="w-fit"
                onclick={() => (oncontinue ?? oncancel)()}
              >
                {oncontinue ? continueLabel : "Done"}
              </Button>
            </div>
          {/if}
        </li>
      </ol>
    {/if}

    {#if sourceError}
      <Alert.Root variant="destructive">
        <CircleAlert class="size-4 shrink-0" />
        <Alert.Title>Dokploy did not answer as expected</Alert.Title>
        <Alert.Description>{sourceError}</Alert.Description>
      </Alert.Root>
    {/if}

    <div class="flex justify-end">
      <Button type="button" variant="ghost" onclick={oncancel}>Close</Button>
    </div>
  </section>
{/if}
