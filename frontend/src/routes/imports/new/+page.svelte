<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { ArrowLeft, KeyRound, LoaderCircle, Search } from "lucide-svelte";
  import type { Component } from "svelte";
  import SiForgejo from "@icons-pack/svelte-simple-icons/icons/SiForgejo";
  import SiGitea from "@icons-pack/svelte-simple-icons/icons/SiGitea";
  import SiGithub from "@icons-pack/svelte-simple-icons/icons/SiGithub";
  import SiGitlab from "@icons-pack/svelte-simple-icons/icons/SiGitlab";

  import {
    ApiFailure,
    importDiscoverySchema,
    jsonBody,
    mirrorIdentitiesSchema,
    repositoryImportSchema,
    requestJson,
    type ImportDiscovery,
    type MirrorIdentity,
    type RemoteImportRepository,
  } from "$lib/api.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  type Provider = "github" | "gitlab" | "gitea" | "forgejo";
  type ImportMode = "provider" | "direct";

  const app = useAppState();
  const providers: Array<{
    id: Provider;
    name: string;
    description: string;
    icon: Component;
  }> = [
    {
      id: "github",
      name: "GitHub",
      description: "Discover every repository available to a GitHub token.",
      icon: SiGithub,
    },
    {
      id: "gitlab",
      name: "GitLab",
      description: "Discover projects from GitLab.com or your own instance.",
      icon: SiGitlab,
    },
    {
      id: "gitea",
      name: "Gitea",
      description: "Discover repositories from a Gitea instance.",
      icon: SiGitea,
    },
    {
      id: "forgejo",
      name: "Forgejo",
      description: "Discover repositories from a Forgejo instance.",
      icon: SiForgejo,
    },
  ];

  let mode = $state<ImportMode>("provider");
  let provider = $state<Provider>("github");
  let targetNamespace = $state("");
  let identityId = $state("");
  let identities = $state.raw<MirrorIdentity[]>([]);
  let identitiesLoading = $state(false);
  let discovery = $state.raw<ImportDiscovery | null>(null);
  let selected = $state<Record<string, boolean>>({});
  let targetNames = $state<Record<string, string>>({});
  let search = $state("");
  let includeArchived = $state(false);
  let directUrl = $state("");
  let directName = $state("");
  let directVisibility = $state("private");
  let connecting = $state(false);
  let importing = $state(false);
  let error = $state<string | null>(null);
  let identityLoadSequence = 0;

  const viewer = $derived(app.authStatus?.user?.username ?? "");
  const namespace = $derived(targetNamespace || viewer);
  const organizations = $derived(
    app.organizations.filter((organization) => organization.role === "owner"),
  );
  const selectedProvider = $derived(
    providers.find((candidate) => candidate.id === provider) ?? providers[0],
  );
  const eligibleIdentities = $derived.by(() => {
    if (mode === "provider") {
      return identities.filter(
        (identity) =>
          identity.kind === "token" && identity.provider === provider,
      );
    }
    if (!directUrl.trim()) return identities;
    const ssh = isSshUrl(directUrl);
    return identities.filter((identity) =>
      ssh ? identity.kind === "ssh" : identity.kind !== "ssh",
    );
  });
  const selectedIdentity = $derived(
    eligibleIdentities.find((identity) => identity.id === identityId) ?? null,
  );
  const filteredRepositories = $derived.by(() => {
    const query = search.trim().toLowerCase();
    return (discovery?.repositories ?? []).filter(
      (repository) =>
        (includeArchived || !repository.archived) &&
        (!query ||
          repository.full_name.toLowerCase().includes(query) ||
          repository.description?.toLowerCase().includes(query)),
    );
  });
  const selectedCount = $derived(
    Object.values(selected).filter(Boolean).length,
  );

  $effect(() => {
    const currentNamespace = namespace;
    if (currentNamespace) void loadIdentities(currentNamespace);
  });

  function isSshUrl(value: string): boolean {
    const remote = value.trim();
    return remote.startsWith("ssh://") || /^git@[^:]+:.+/.test(remote);
  }

  function message(caught: unknown, fallback: string): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  async function loadIdentities(currentNamespace: string): Promise<void> {
    const sequence = ++identityLoadSequence;
    identitiesLoading = true;
    try {
      const next = await requestJson(
        `/api/v1/namespaces/${encodeURIComponent(currentNamespace)}/mirror-identities`,
        mirrorIdentitiesSchema,
      );
      if (sequence !== identityLoadSequence) return;
      identities = next;
      if (!next.some((identity) => identity.id === identityId)) identityId = "";
    } catch (caught) {
      if (sequence !== identityLoadSequence) return;
      identities = [];
      error = message(caught, "Could not load repository identities.");
    } finally {
      if (sequence === identityLoadSequence) identitiesLoading = false;
    }
  }

  function chooseProvider(next: Provider): void {
    provider = next;
    identityId = "";
    discovery = null;
    selected = {};
    targetNames = {};
    error = null;
  }

  function chooseMode(next: ImportMode): void {
    mode = next;
    identityId = "";
    discovery = null;
    selected = {};
    targetNames = {};
    error = null;
  }

  async function connect(): Promise<void> {
    if (!namespace || !identityId) return;
    connecting = true;
    error = null;
    try {
      const result = await requestJson(
        "/api/v1/repository-imports/discover",
        importDiscoverySchema,
        {
          method: "POST",
          body: jsonBody({
            provider,
            namespace,
            identity_id: identityId,
          }),
        },
      );
      selected = Object.fromEntries(
        result.repositories.map((repository) => [repository.id, false]),
      );
      targetNames = Object.fromEntries(
        result.repositories.map((repository) => [
          repository.id,
          repository.name,
        ]),
      );
      discovery = result;
    } catch (caught) {
      error = message(caught, "Could not load repositories from the source.");
    } finally {
      connecting = false;
    }
  }

  function toggleVisible(checked: boolean): void {
    for (const repository of filteredRepositories) {
      selected[repository.id] = checked;
    }
  }

  async function startImport(): Promise<void> {
    if (!namespace || !identityId || selectedCount === 0) return;
    importing = true;
    error = null;
    try {
      const result = await requestJson(
        "/api/v1/repository-imports",
        repositoryImportSchema,
        {
          method: "POST",
          body: jsonBody({
            provider,
            identity_id: identityId,
            target_namespace: namespace,
            repositories: (discovery?.repositories ?? [])
              .filter((repository) => selected[repository.id])
              .map((repository) => ({
                source_id: repository.id,
                target_namespace: null,
                target_name: targetNames[repository.id],
              })),
          }),
        },
      );
      await goto(resolve("/imports/[id]", { id: result.id }));
    } catch (caught) {
      error = message(caught, "Could not start the repository import.");
    } finally {
      importing = false;
    }
  }

  async function startDirectImport(): Promise<void> {
    if (
      !namespace ||
      !selectedIdentity ||
      !directUrl.trim() ||
      !directName.trim()
    )
      return;
    importing = true;
    error = null;
    try {
      const result = await requestJson(
        "/api/v1/repository-imports/direct",
        repositoryImportSchema,
        {
          method: "POST",
          body: jsonBody({
            remote_url: directUrl.trim(),
            identity_id: selectedIdentity.id,
            target_namespace: namespace,
            target_name: directName.trim(),
            visibility: directVisibility,
          }),
        },
      );
      await goto(resolve("/imports/[id]", { id: result.id }));
    } catch (caught) {
      error = message(caught, "Could not start the repository import.");
    } finally {
      importing = false;
    }
  }

  function repositoryMeta(repository: RemoteImportRepository): string {
    return [
      repository.visibility,
      repository.fork ? "fork" : null,
      repository.archived ? "archived" : null,
    ]
      .filter(Boolean)
      .join(" · ");
  }

  function openSource(url: string): void {
    try {
      const parsed = new URL(url);
      if (parsed.protocol === "https:" || parsed.protocol === "http:") {
        globalThis.open(parsed, "_blank", "noopener,noreferrer");
      }
    } catch {
      // Ignore malformed optional provider metadata.
    }
  }

  function identityDescription(identity: MirrorIdentity): string {
    if (identity.kind === "ssh") return identity.fingerprint ?? "SSH key";
    if (identity.kind === "basic")
      return identity.username ?? "Username and password";
    return identity.instance_url ?? identity.provider ?? "Access token";
  }
</script>

<svelte:head>
  <title>Import repositories · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<div class="mx-auto max-w-6xl px-5 py-8 lg:px-8">
  <div class="mb-7 flex items-start gap-3">
    <Button
      size="icon"
      variant="ghost"
      aria-label="Back"
      onclick={() => void goto(resolve("/"))}
    >
      <ArrowLeft class="size-4" />
    </Button>
    <div>
      <h1 class="text-xl font-semibold">Import repositories</h1>
      <p class="mt-1 text-sm text-muted-foreground">
        Reuse a repository identity owned by the destination namespace.
      </p>
    </div>
  </div>

  {#if error}
    <p
      class="mb-5 rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive"
      role="alert"
    >
      {error}
    </p>
  {/if}

  {#if !discovery}
    <div
      class="mb-6 grid grid-cols-2 gap-2 rounded-lg bg-muted/40 p-1 sm:w-fit"
    >
      <button
        type="button"
        class={[
          "rounded-md px-4 py-2 text-sm font-medium transition-colors",
          mode === "provider"
            ? "bg-background shadow-sm"
            : "text-muted-foreground",
        ]}
        aria-pressed={mode === "provider"}
        onclick={() => chooseMode("provider")}>Provider account</button
      >
      <button
        type="button"
        class={[
          "rounded-md px-4 py-2 text-sm font-medium transition-colors",
          mode === "direct"
            ? "bg-background shadow-sm"
            : "text-muted-foreground",
        ]}
        aria-pressed={mode === "direct"}
        onclick={() => chooseMode("direct")}>Direct Git URL</button
      >
    </div>

    <div class="grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(20rem,0.8fr)]">
      {#if mode === "provider"}
        <section>
          <h2 class="text-sm font-medium">1. Choose a source</h2>
          <div class="mt-3 grid gap-3 sm:grid-cols-2">
            {#each providers as candidate (candidate.id)}
              {@const Icon = candidate.icon}
              <button
                type="button"
                class={[
                  "rounded-xl border p-4 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                  provider === candidate.id
                    ? "border-primary bg-primary/5"
                    : "bg-card/20 hover:bg-card/40",
                ]}
                aria-pressed={provider === candidate.id}
                onclick={() => chooseProvider(candidate.id)}
              >
                <span class="flex items-center gap-2 font-medium">
                  <Icon class="size-4" aria-hidden="true" />{candidate.name}
                </span>
                <span
                  class="mt-2 block text-sm leading-5 text-muted-foreground"
                >
                  {candidate.description}
                </span>
              </button>
            {/each}
          </div>
        </section>
      {:else}
        <section class="rounded-xl border bg-card/20 p-5">
          <h2 class="font-medium">Import one Git repository</h2>
          <p class="mt-1 text-sm leading-5 text-muted-foreground">
            Use an HTTPS URL with a token or password identity, or an SSH URL
            with an SSH identity.
          </p>
          <div class="mt-5 grid gap-4">
            <Field.Field>
              <Field.Label for="direct-url">Git URL</Field.Label>
              <Input
                id="direct-url"
                bind:value={directUrl}
                placeholder="git@code.example.com:team/project.git"
                required
              />
            </Field.Field>
            <Field.Field>
              <Field.Label for="direct-name">Repository name</Field.Label>
              <Input
                id="direct-name"
                bind:value={directName}
                maxlength={100}
                required
              />
            </Field.Field>
            <Field.Field>
              <Field.Label>Visibility</Field.Label>
              <Select.Root type="single" bind:value={directVisibility}>
                <Select.Trigger class="w-full"
                  >{directVisibility === "public"
                    ? "Public"
                    : "Private"}</Select.Trigger
                >
                <Select.Content>
                  <Select.Item value="private">Private</Select.Item>
                  <Select.Item value="public">Public</Select.Item>
                </Select.Content>
              </Select.Root>
            </Field.Field>
          </div>
        </section>
      {/if}

      <form
        class="rounded-xl border bg-card/20 p-5"
        onsubmit={(event) => {
          event.preventDefault();
          if (mode === "provider") void connect();
          else void startDirectImport();
        }}
      >
        <h2 class="font-medium">
          {mode === "provider" ? "2." : "2."} Choose destination and identity
        </h2>
        <p class="mt-1 text-sm text-muted-foreground">
          Secrets stay in namespace settings and are never entered here.
        </p>
        <div class="mt-5 grid gap-4">
          <Field.Field>
            <Field.Label>Destination namespace</Field.Label>
            <Select.Root
              type="single"
              value={namespace}
              onValueChange={(value) => {
                targetNamespace = value;
                identityId = "";
              }}
            >
              <Select.Trigger class="w-full"
                >{namespace || "Choose namespace"}</Select.Trigger
              >
              <Select.Content>
                {#if viewer}<Select.Item value={viewer}>{viewer}</Select.Item
                  >{/if}
                {#each organizations as organization (organization.id)}
                  <Select.Item value={organization.slug}
                    >{organization.slug}</Select.Item
                  >
                {/each}
              </Select.Content>
            </Select.Root>
          </Field.Field>

          <Field.Field>
            <Field.Label>Repository identity</Field.Label>
            <Select.Root
              type="single"
              bind:value={identityId}
              disabled={identitiesLoading}
            >
              <Select.Trigger class="w-full">
                {selectedIdentity?.name ??
                  (identitiesLoading
                    ? "Loading identities…"
                    : "Choose identity")}
              </Select.Trigger>
              <Select.Content>
                {#each eligibleIdentities as identity (identity.id)}
                  <Select.Item value={identity.id}>
                    {identity.name} · {identity.kind === "ssh"
                      ? "SSH"
                      : (identity.provider ?? identity.kind)}
                  </Select.Item>
                {/each}
              </Select.Content>
            </Select.Root>
            {#if selectedIdentity}
              <Field.Description
                >{identityDescription(selectedIdentity)}</Field.Description
              >
            {:else if !identitiesLoading && eligibleIdentities.length === 0}
              <Field.Description>
                No compatible identity exists in {namespace}.
              </Field.Description>
            {/if}
          </Field.Field>

          <Button
            type="button"
            variant="outline"
            class="justify-start"
            onclick={() =>
              void goto(
                resolve("/[namespace]/mirror-credentials", { namespace }),
              )}
          >
            <KeyRound class="size-4" /> Manage identities
          </Button>

          <Button
            type="submit"
            disabled={connecting ||
              importing ||
              !selectedIdentity ||
              (mode === "direct" && (!directUrl.trim() || !directName.trim()))}
          >
            {#if connecting || importing}<LoaderCircle
                class="size-4 animate-spin"
              />{/if}
            {mode === "provider"
              ? connecting
                ? "Loading repositories…"
                : `Continue with ${selectedProvider.name}`
              : importing
                ? "Starting import…"
                : "Import repository"}
          </Button>
        </div>
      </form>
    </div>
  {:else}
    <div class="grid gap-5">
      <section class="rounded-xl border bg-card/20 p-5">
        <div class="flex flex-wrap items-end justify-between gap-4">
          <div>
            <p class="text-sm font-medium">
              Connected to {selectedProvider.name} as {discovery.account}
            </p>
            <p class="mt-1 text-sm text-muted-foreground">
              {selectedCount} selected for {namespace} · {discovery.repositories
                .length} detected
            </p>
          </div>
          <Button
            variant="outline"
            size="sm"
            onclick={() => (discovery = null)}
          >
            Change source
          </Button>
        </div>
        <div class="mt-5">
          <Field.Field>
            <Field.Label for="import-search">Filter repositories</Field.Label>
            <div class="relative">
              <Search
                class="pointer-events-none absolute left-3 top-2.5 size-4 text-muted-foreground"
              />
              <Input
                id="import-search"
                class="pl-9"
                bind:value={search}
                placeholder="Search by name"
              />
            </div>
          </Field.Field>
        </div>
        <label class="mt-4 flex w-fit items-center gap-2 text-sm">
          <input
            type="checkbox"
            bind:checked={includeArchived}
            class="size-4 accent-primary"
          />
          Include archived repositories
        </label>
      </section>

      <section class="overflow-hidden rounded-xl border bg-card/20">
        <div
          class="flex flex-wrap items-center justify-between gap-3 border-b px-4 py-3"
        >
          <label class="flex items-center gap-2 text-sm font-medium">
            <input
              type="checkbox"
              checked={filteredRepositories.length > 0 &&
                filteredRepositories.every(
                  (repository) => selected[repository.id],
                )}
              onchange={(event) => toggleVisible(event.currentTarget.checked)}
              class="size-4 accent-primary"
            />
            Select all shown
          </label>
          <span class="text-sm text-muted-foreground"
            >{selectedCount} selected</span
          >
        </div>
        <div class="max-h-[32rem] divide-y overflow-y-auto">
          {#each filteredRepositories as repository (repository.id)}
            <div
              class="grid gap-3 p-4 md:grid-cols-[auto_minmax(0,1fr)_15rem_8rem] md:items-center"
            >
              <input
                type="checkbox"
                bind:checked={selected[repository.id]}
                aria-label={`Import ${repository.full_name}`}
                class="size-4 accent-primary"
              />
              <div class="min-w-0">
                <button
                  type="button"
                  class="block max-w-full truncate text-left font-medium hover:underline"
                  onclick={() => openSource(repository.web_url)}
                >
                  {repository.full_name}
                </button>
                <p class="mt-0.5 truncate text-xs text-muted-foreground">
                  {repository.description ?? repositoryMeta(repository)}
                </p>
              </div>
              <Input
                aria-label={`Destination name for ${repository.full_name}`}
                bind:value={targetNames[repository.id]}
                maxlength={100}
                disabled={!selected[repository.id]}
              />
              <span class="text-sm capitalize text-muted-foreground">
                {repository.visibility === "public" ? "Public" : "Private"}
              </span>
            </div>
          {:else}
            <p class="p-8 text-center text-sm text-muted-foreground">
              No repositories match this filter.
            </p>
          {/each}
        </div>
      </section>

      <div
        class="rounded-xl border border-amber-500/30 bg-amber-500/5 p-4 text-sm leading-6 text-muted-foreground"
      >
        This imports branches, tags, source code, commit history, Git LFS
        objects, releases and release assets, and repository labels. Issues,
        pull or merge requests, wiki content, and permissions are not imported.
      </div>

      <div class="flex justify-end">
        <Button
          size="lg"
          disabled={importing || selectedCount === 0}
          onclick={() => void startImport()}
        >
          {#if importing}<LoaderCircle class="size-4 animate-spin" />{/if}
          {importing
            ? "Starting import…"
            : `Import ${selectedCount} repositories`}
        </Button>
      </div>
    </div>
  {/if}
</div>
