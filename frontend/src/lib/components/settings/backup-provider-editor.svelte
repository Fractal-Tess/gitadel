<script lang="ts">
  import { untrack } from "svelte";
  import {
    ArrowLeft,
    CheckCircle2,
    CloudUpload,
    HardDrive,
    LoaderCircle,
  } from "lucide-svelte";

  import type {
    BackupProvider,
    BackupProviderCatalogItem,
    BackupProviderKind,
  } from "$lib/api.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";

  export type BackupProviderEditorValue = {
    id: string | null;
    name: string;
    provider: BackupProviderKind;
    path: string;
    endpoint: string;
    bucket: string;
    accessKey: string;
    secretKey: string;
    region: string;
    prefix: string;
    testToken: string | null;
  };

  let {
    providers,
    connection = null,
    onsave,
    ontest,
    oncancel,
  }: {
    providers: BackupProviderCatalogItem[];
    connection?: BackupProvider | null;
    onsave: (value: BackupProviderEditorValue) => Promise<void>;
    ontest: (
      value: BackupProviderEditorValue,
    ) => Promise<{ test_token: string; message: string }>;
    oncancel: () => void;
  } = $props();

  let step = $state<"provider" | "details">(
    untrack(() => (connection ? "details" : "provider")),
  );
  let provider = $state<BackupProviderKind>(
    untrack(() => connection?.provider ?? "filesystem"),
  );
  let name = $state(untrack(() => connection?.name ?? ""));
  let path = $state(untrack(() => connection?.path ?? ""));
  let endpoint = $state(untrack(() => connection?.endpoint ?? ""));
  let bucket = $state(untrack(() => connection?.bucket ?? ""));
  let accessKey = $state("");
  let secretKey = $state("");
  let region = $state(untrack(() => connection?.region ?? "us-east-1"));
  let prefix = $state(untrack(() => connection?.prefix ?? "backups"));
  let testing = $state(false);
  let saving = $state(false);
  let error = $state<string | null>(null);
  let testMessage = $state<string | null>(null);
  let testToken = $state<string | null>(null);
  let testedFingerprint = $state<string | null>(null);

  const selectedProvider = $derived(
    providers.find((candidate) => candidate.slug === provider) ?? null,
  );
  const fingerprint = $derived(
    JSON.stringify({
      provider,
      path,
      endpoint,
      bucket,
      accessKey,
      secretKey,
      region,
      prefix,
    }),
  );
  const testPassed = $derived(
    testToken !== null && testedFingerprint === fingerprint,
  );
  const canTest = $derived(
    name.trim().length > 0 &&
      (provider === "filesystem"
        ? path.trim().length > 0
        : endpoint.trim().length > 0 &&
          bucket.trim().length > 0 &&
          region.trim().length > 0 &&
          ((accessKey.trim().length > 0 && secretKey.trim().length > 0) ||
            connection?.provider === "s3")),
  );

  function value(): BackupProviderEditorValue {
    return {
      id: connection?.id ?? null,
      name,
      provider,
      path,
      endpoint,
      bucket,
      accessKey,
      secretKey,
      region,
      prefix,
      testToken,
    };
  }

  function selectProvider(selected: BackupProviderCatalogItem) {
    provider = selected.slug;
    name = selected.name;
    error = null;
    testMessage = null;
    testToken = null;
    testedFingerprint = null;
    step = "details";
  }

  function message(caught: unknown, fallback: string) {
    return caught instanceof Error ? caught.message : fallback;
  }

  async function test() {
    testing = true;
    error = null;
    testMessage = null;
    const tested = fingerprint;
    try {
      const response = await ontest(value());
      testToken = response.test_token;
      testedFingerprint = tested;
      testMessage = response.message;
    } catch (caught) {
      error = message(caught, "Could not test the backup provider.");
    } finally {
      testing = false;
    }
  }

  async function submit() {
    if (!testPassed) return;
    saving = true;
    error = null;
    try {
      await onsave(value());
    } catch (caught) {
      error = message(caught, "Could not save the backup provider.");
    } finally {
      saving = false;
    }
  }
</script>

{#if step === "provider"}
  <div class="grid gap-3 sm:grid-cols-2">
    {#each providers as candidate (candidate.slug)}
      <button
        type="button"
        class="group flex min-h-44 flex-col rounded-xl border bg-card/30 p-4 text-left transition-colors hover:border-foreground/25 hover:bg-card/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        onclick={() => selectProvider(candidate)}
      >
        <span
          class="grid size-11 place-items-center rounded-xl border bg-zinc-950"
          aria-hidden="true"
        >
          {#if candidate.slug === "filesystem"}
            <HardDrive class="size-5 text-primary" />
          {:else}
            <CloudUpload class="size-5 text-primary" />
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
      void submit();
    }}
  >
    {#if !connection}
      <Button
        type="button"
        variant="ghost"
        size="sm"
        class="w-fit gap-1.5 px-0 hover:bg-transparent"
        disabled={testing || saving}
        onclick={() => (step = "provider")}
      >
        <ArrowLeft class="size-3.5" />Back to providers
      </Button>
    {/if}

    <div class="flex items-center gap-3 rounded-lg border bg-muted/20 p-3">
      <span class="grid size-9 place-items-center rounded-lg border bg-zinc-950">
        {#if provider === "filesystem"}
          <HardDrive class="size-4 text-primary" />
        {:else}
          <CloudUpload class="size-4 text-primary" />
        {/if}
      </span>
      <div>
        <p class="text-sm font-medium">{selectedProvider?.name}</p>
        <p class="text-xs text-muted-foreground">{selectedProvider?.description}</p>
      </div>
    </div>

    <label class="grid gap-1.5 text-sm font-medium">
      Provider name
      <Input
        bind:value={name}
        maxlength={80}
        placeholder={provider === "filesystem" ? "Local backups" : "Production S3"}
        disabled={testing || saving}
        required
      />
      <span class="text-xs font-normal text-muted-foreground">
        Use a label that distinguishes this backup destination.
      </span>
    </label>

    {#if provider === "filesystem"}
      <label class="grid gap-1.5 text-sm font-medium">
        Directory path
        <Input
          bind:value={path}
          class="font-mono"
          placeholder="/var/lib/gitadel/backups"
          disabled={testing || saving}
          required
        />
        <span class="text-xs font-normal text-muted-foreground">
          Absolute path on the Gitadel host. The directory is created when needed.
        </span>
      </label>
    {:else}
      <label class="grid gap-1.5 text-sm font-medium">
        Endpoint
        <Input
          bind:value={endpoint}
          type="url"
          class="font-mono"
          placeholder="https://s3.example.com"
          disabled={testing || saving}
          required
        />
      </label>
      <div class="grid gap-4 sm:grid-cols-2">
        <label class="grid gap-1.5 text-sm font-medium">
          Bucket
          <Input bind:value={bucket} disabled={testing || saving} required />
        </label>
        <label class="grid gap-1.5 text-sm font-medium">
          Region
          <Input bind:value={region} disabled={testing || saving} required />
        </label>
        <label class="grid gap-1.5 text-sm font-medium">
          Access key
          <Input
            bind:value={accessKey}
            autocomplete="off"
            placeholder={connection?.access_key_hint ?? "Required"}
            disabled={testing || saving}
            required={!connection || connection.provider !== "s3"}
          />
        </label>
        <label class="grid gap-1.5 text-sm font-medium">
          Secret key
          <Input
            bind:value={secretKey}
            type="password"
            autocomplete="new-password"
            placeholder={connection?.provider === "s3"
              ? "Leave blank to keep current secret"
              : "Required"}
            disabled={testing || saving}
            required={!connection || connection.provider !== "s3"}
          />
        </label>
      </div>
      <label class="grid gap-1.5 text-sm font-medium">
        Object prefix
        <Input
          bind:value={prefix}
          class="font-mono"
          placeholder="backups"
          disabled={testing || saving}
        />
      </label>
    {/if}

    {#if error}
      <p class="text-sm text-destructive" role="alert">{error}</p>
    {:else if testPassed && testMessage}
      <p class="flex items-center gap-2 text-sm text-emerald-400">
        <CheckCircle2 class="size-4" />{testMessage}
      </p>
    {/if}

    <div class="flex flex-wrap items-center justify-end gap-2 border-t pt-4">
      <Button
        type="button"
        variant="ghost"
        disabled={testing || saving}
        onclick={oncancel}
      >
        Cancel
      </Button>
      <Button
        type="button"
        variant="outline"
        disabled={testing || saving || !canTest}
        onclick={() => void test()}
      >
        {#if testing}
          <LoaderCircle class="size-3.5 animate-spin" />Testing…
        {:else}
          Test provider
        {/if}
      </Button>
      <Button type="submit" disabled={testing || saving || !testPassed}>
        {#if saving}
          <LoaderCircle class="size-3.5 animate-spin" />Saving…
        {:else}
          Save provider
        {/if}
      </Button>
    </div>
  </form>
{/if}
