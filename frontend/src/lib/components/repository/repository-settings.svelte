<script lang="ts">
  import Archive from "@lucide/svelte/icons/archive";
  import ImageIcon from "@lucide/svelte/icons/image";
  import Check from "@lucide/svelte/icons/check";
  import Upload from "@lucide/svelte/icons/upload";
  import { repositoryImageUrl } from "$lib/api/repositories.js";
  import MapPin from "@lucide/svelte/icons/map-pin";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";

  import RepositoryIcon from "$lib/components/repository/repository-icon.svelte";
  import RepositoryIntegrationSettings from "$lib/components/repository/repository-integration-settings.svelte";
  import AvatarCropDialog from "$lib/components/settings/avatar-crop-dialog.svelte";
  import RepositoryMirrorSettings from "$lib/components/repository/repository-mirror-settings.svelte";
  import RepositoryWebhookSettings from "$lib/components/repository/repository-webhook-settings.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import { isRepositorySettingsSection } from "$lib/repository/settings-sections.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  let visibility = $state<"public" | "private">("private");
  let defaultBranch = $state("");
  let repositoryName = $state("");
  let targetNamespace = $state("");
  let initializedFor = $state("");
  let iconEditorOpen = $state(false);
  let iconMode = $state<"automatic" | "selected" | "uploaded" | "none">(
    "automatic",
  );
  let selectedIconPath = $state("");
  let initializedIconFor = $state("");
  let initializedIconSelection = $state("");
  let moveDialogOpen = $state(false);
  let deleteDialogOpen = $state(false);
  // Sections are pages reached from the rail, so an unknown one falls back to
  // the first page rather than rendering nothing.
  const section = $derived(
    isRepositorySettingsSection(repository.settingsTab) &&
      (repository.settingsTab !== "mirror" || repository.repository?.mirrored)
      ? repository.settingsTab
      : "general",
  );

  // Only existing branches are valid targets for Git's symbolic HEAD, so the
  // current default is included even if the ref list has not loaded yet.
  const branches = $derived.by(() => {
    const names =
      repository.browser.refs?.branches.map((branch) => branch.name) ?? [];
    return names.includes(defaultBranch) || !defaultBranch
      ? names
      : [defaultBranch, ...names];
  });

  $effect(() => {
    const current = repository.repository;
    if (!current || initializedFor === current.id) return;
    initializedFor = current.id;
    visibility = current.visibility;
    defaultBranch = current.default_branch ?? "";
    repositoryName = current.name;
    targetNamespace = current.namespace;
  });

  const defaultTip = $derived(
    repository.browser.refs?.branches.find(
      (branch) => branch.name === repository.repository?.default_branch,
    )?.commit_oid ?? "",
  );
  $effect(() => {
    const current = repository.repository;
    const iconKey = current
      ? `${current.id}:${current.default_branch ?? ""}:${defaultTip}`
      : "";
    if (section !== "general" || !current || initializedIconFor === iconKey)
      return;
    initializedIconFor = iconKey;
    void repository.settings.loadIconCandidates();
  });

  $effect(() => {
    const settings = repository.settings;
    const candidates = settings.iconCandidates;
    if (
      section !== "general" ||
      settings.iconCandidatesLoading ||
      settings.iconSelectionPending ||
      settings.iconPending ||
      settings.iconCandidatesError
    )
      return;
    if (candidates && candidates.scan_status !== "pending") return;
    const timer = setTimeout(() => void settings.loadIconCandidates(), 1000);
    return () => clearTimeout(timer);
  });
  $effect(() => {
    const candidates = repository.settings.iconCandidates;
    if (!candidates) return;
    const key = `${repository.repository?.id}:${repository.repository?.default_branch}:${candidates.mode}:${candidates.selected_path ?? ""}`;
    if (key === initializedIconSelection) return;
    initializedIconSelection = key;
    iconMode = candidates.mode;
    selectedIconPath = candidates.selected_path ?? "";
  });

  async function saveGeneral() {
    const current = repository.repository;
    if (!current) return;
    try {
      await repository.settings.updateRepositoryControl({
        ...(visibility !== current.visibility && { visibility }),
        ...(defaultBranch &&
          defaultBranch !== current.default_branch && {
            default_branch: defaultBranch,
          }),
      });
    } catch {
      // The repository settings state owns mutation error toasts.
    }
  }

  async function confirmMoveRepository() {
    try {
      await repository.settings.updateRepositoryControl({
        name: repositoryName,
        namespace: targetNamespace,
      });
      moveDialogOpen = false;
    } catch {
      // The repository settings state owns mutation error toasts.
    }
  }

  async function confirmDeleteRepository() {
    try {
      await repository.settings.softDelete();
      deleteDialogOpen = false;
    } catch {
      // The repository settings state owns mutation error toasts.
    }
  }
</script>

{#if section === "general"}
  <div
    class="divide-y divide-border overflow-hidden rounded-xl bg-card/20 ring-1 ring-foreground/15"
  >
    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="repository-general-heading"
    >
      <header class="flex items-start gap-3">
        <Settings2 class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h2 id="repository-general-heading" class="font-semibold">General</h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Control who can see this repository and which branch is its default.
          </p>
        </div>
      </header>

      <form
        class="grid max-w-2xl gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void saveGeneral();
        }}
      >
        <Field.Field>
          <Field.Label>Visibility</Field.Label>
          <Select.Root type="single" bind:value={visibility}>
            <Select.Trigger class="w-full">
              {visibility === "public"
                ? "Public — visible to everyone"
                : "Private — restricted access"}
            </Select.Trigger>
            <Select.Content>
              <Select.Item value="public">
                Public — visible to everyone
              </Select.Item>
              <Select.Item value="private">
                Private — restricted access
              </Select.Item>
            </Select.Content>
          </Select.Root>
        </Field.Field>

        <Field.Field>
          <Field.Label>Default branch</Field.Label>
          <Select.Root type="single" bind:value={defaultBranch}>
            <Select.Trigger class="w-full">
              {defaultBranch || "Select a branch"}
            </Select.Trigger>
            <Select.Content>
              {#each branches as branch (branch)}
                <Select.Item value={branch}>{branch}</Select.Item>
              {/each}
            </Select.Content>
          </Select.Root>
          <Field.Description>
            Saving updates Git’s symbolic HEAD.
          </Field.Description>
        </Field.Field>

        <div class="flex justify-end">
          <Button
            type="submit"
            disabled={repository.settings.repositoryControlPending}
          >
            {repository.settings.repositoryControlPending
              ? "Saving…"
              : "Save general settings"}
          </Button>
        </div>
      </form>
    </section>

    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="repository-icon-heading"
    >
      <header class="flex items-start gap-3">
        <ImageIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h2 id="repository-icon-heading" class="font-semibold">Icon</h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Choose the icon shown wherever this repository is listed.
          </p>
        </div>
      </header>

      <div class="grid max-w-2xl gap-5">
        <div class="flex items-center gap-4">
          <RepositoryIcon
            namespace={repository.namespace}
            name={repository.name}
            iconUpdatedAt={repository.repository?.icon_updated_at ?? null}
            class="size-20 ring-1 ring-foreground/15"
          />
          <div class="text-sm">
            <p class="font-medium">
              Current: {repository.repository?.icon_source ?? "none"}
            </p>
            <p class="mt-1 text-xs text-muted-foreground">
              {#if repository.settings.iconCandidatesLoading}
                Loading logo candidates…
              {:else if repository.settings.iconCandidatesError}
                {repository.settings.iconCandidatesError}
              {:else if !repository.repository?.default_branch}
                Push a branch to discover logos, or upload an icon.
              {:else if repository.settings.iconCandidates?.scan_status === "pending"}
                Scanning the default branch for logos…
              {:else if repository.settings.iconCandidates?.scan_status === "failed"}
                Logo scan failed. You can upload an icon instead.
              {:else if repository.settings.iconCandidates?.scan_status === "partial"}
                Incomplete scan: some files were skipped or could not be
                decoded.
              {:else}
                {#if !repository.settings.iconCandidates?.candidates.length}
                  No logo candidates found on the default branch.
                {:else}
                  Candidate logos are read from a pinned commit.
                {/if}
              {/if}
            </p>
          </div>
        </div>
        <div class="grid gap-2">
          <span class="text-sm font-medium">Source</span>
          <div
            class="flex flex-wrap gap-2"
            role="group"
            aria-label="Icon source"
          >
            <Button
              type="button"
              variant={iconMode === "automatic" ? "default" : "outline"}
              disabled={repository.settings.iconSelectionPending}
              aria-pressed={iconMode === "automatic"}
              onclick={() => (iconMode = "automatic")}
            >
              Use automatic
            </Button>
            <Button
              type="button"
              variant={iconMode === "none" ? "default" : "outline"}
              disabled={repository.settings.iconSelectionPending}
              aria-pressed={iconMode === "none"}
              onclick={() => (iconMode = "none")}
            >
              Remove
            </Button>
            <Button
              type="button"
              variant={iconMode === "uploaded" ? "default" : "outline"}
              disabled={repository.settings.iconPending ||
                repository.settings.iconSelectionPending}
              onclick={() => (iconEditorOpen = true)}
            >
              <Upload data-icon="inline-start" />Upload
            </Button>
          </div>
        </div>
        <div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={repository.settings.iconCandidatesLoading ||
              repository.settings.iconSelectionPending ||
              repository.settings.iconPending}
            onclick={() => void repository.settings.loadIconCandidates()}
          >
            Refresh candidates
          </Button>
        </div>

        {#if repository.settings.iconCandidates?.selected_missing}
          <p
            class="rounded-md border border-amber-500/30 bg-amber-500/10 p-3 text-xs text-amber-700 dark:text-amber-300"
          >
            The previously selected logo is missing from the current default
            branch. Choose another candidate or use Automatic.
          </p>
        {/if}

        {#if repository.settings.iconCandidates?.candidates.length}
          <div class="grid gap-2">
            <div class="flex items-center justify-between">
              <span class="text-sm font-medium">Detected logos</span>
              {#if iconMode === "selected" && !selectedIconPath}
                <span class="text-xs text-destructive">Choose one</span>
              {/if}
            </div>
            <div class="grid grid-cols-2 gap-2 sm:grid-cols-3">
              {#each repository.settings.iconCandidates.candidates as candidate (candidate.path)}
                <button
                  type="button"
                  class="group relative grid gap-2 rounded-lg border p-2 text-left transition-colors hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  class:border-primary={iconMode === "selected" &&
                    selectedIconPath === candidate.path}
                  aria-pressed={iconMode === "selected" &&
                    selectedIconPath === candidate.path}
                  onclick={() => {
                    selectedIconPath = candidate.path;
                    iconMode = "selected";
                  }}
                >
                  {#if repository.settings.iconCandidates.commit_oid}
                    <img
                      class="aspect-square w-full rounded-md border bg-muted/30 object-contain"
                      src={repositoryImageUrl(
                        repository.namespace,
                        repository.name,
                        repository.settings.iconCandidates.commit_oid,
                        candidate.path,
                      )}
                      alt={`Preview of ${candidate.path}`}
                    />
                  {:else}
                    <div
                      class="grid aspect-square place-items-center rounded-md border bg-muted/30 text-xs text-muted-foreground"
                    >
                      No preview
                    </div>
                  {/if}
                  {#if iconMode === "selected" && selectedIconPath === candidate.path}
                    <Check
                      class="absolute top-3 right-3 size-4 rounded-full bg-primary p-0.5 text-primary-foreground"
                    />
                  {/if}
                  <span class="min-w-0 truncate font-mono text-[11px]"
                    >{candidate.path}</span
                  >
                  <span class="text-[10px] text-muted-foreground">
                    {candidate.width} × {candidate.height} · {candidate.mime_type}
                  </span>
                  {#if candidate.recommended}
                    <span class="text-[10px] font-medium text-primary"
                      >Recommended</span
                    >
                  {/if}
                  {#if candidate.reasons.length}
                    <span class="text-[10px] leading-4 text-muted-foreground"
                      >{candidate.reasons.join(" · ")}</span
                    >
                  {/if}
                </button>
              {/each}
            </div>
          </div>
        {/if}

        <div class="flex flex-wrap items-center justify-between gap-3">
          <p class="text-xs text-muted-foreground">
            {#if iconMode === "automatic"}
              Use the recommended logo from the default branch.
            {:else if iconMode === "selected"}
              Use this file as it changes on the default branch.
            {:else if iconMode === "uploaded"}
              The uploaded icon stays unchanged until you replace or remove it.
            {:else}
              No repository icon will be shown.
            {/if}
          </p>
          <Button
            type="button"
            disabled={repository.settings.iconSelectionPending ||
              iconMode === "uploaded" ||
              (iconMode === "selected" && !selectedIconPath)}
            onclick={() => {
              if (iconMode === "uploaded") return;
              void repository.settings.saveIconSelection(
                iconMode,
                iconMode === "selected" ? selectedIconPath : undefined,
                repository.settings.iconCandidates?.commit_oid ?? undefined,
              );
            }}
          >
            {repository.settings.iconSelectionPending ? "Saving…" : "Save icon"}
          </Button>
        </div>
      </div>
    </section>
  </div>

  <AvatarCropDialog
    bind:open={iconEditorOpen}
    onsave={(image) => repository.settings.updateIcon(image)}
  />
{:else if section === "location"}
  <div
    class="divide-y divide-border overflow-hidden rounded-xl bg-card/20 ring-1 ring-foreground/15"
  >
    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="repository-location-heading"
    >
      <header class="flex items-start gap-3">
        <MapPin class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h2 id="repository-location-heading" class="font-semibold">
            Location
          </h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Rename or move this repository to a namespace you own.
          </p>
        </div>
      </header>

      <form
        class="grid max-w-2xl gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          moveDialogOpen = true;
        }}
      >
        <Field.Field>
          <Field.Label for="repository-name">Repository name</Field.Label>
          <Input
            id="repository-name"
            bind:value={repositoryName}
            maxlength={100}
            required
          />
        </Field.Field>

        <Field.Field>
          <Field.Label>Namespace</Field.Label>
          <Select.Root type="single" bind:value={targetNamespace}>
            <Select.Trigger class="w-full">{targetNamespace}</Select.Trigger>
            <Select.Content>
              {#each repository.settings.ownedNamespaces as namespace (namespace)}
                <Select.Item value={namespace}>{namespace}</Select.Item>
              {/each}
            </Select.Content>
          </Select.Root>
        </Field.Field>

        <p
          class="rounded-md border bg-muted/30 p-3 text-xs leading-5 text-muted-foreground"
        >
          The repository ID and storage directory do not move. Existing clone
          and browser URLs remain available as aliases; direct collaborators are
          cleared so permissions are recalculated for the new namespace.
        </p>

        <div class="flex justify-end">
          <Button
            type="submit"
            variant="outline"
            disabled={repository.settings.repositoryControlPending}
          >
            {repository.settings.repositoryControlPending
              ? "Moving…"
              : "Save location"}
          </Button>
        </div>
      </form>
    </section>
  </div>
{:else if section === "mirror"}
  <RepositoryMirrorSettings state={repository} />
{:else if section === "webhooks"}
  <RepositoryWebhookSettings state={repository} />
{:else if section === "integrations"}
  <RepositoryIntegrationSettings state={repository} />
{:else}
  <div
    class="divide-y divide-border overflow-hidden rounded-xl bg-card/20 ring-1 ring-foreground/15"
  >
    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="repository-archive-heading"
    >
      <header class="flex items-start gap-3">
        <Archive class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h2 id="repository-archive-heading" class="font-semibold">
            Archive repository
          </h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Archived repositories remain cloneable, but reject all pushes.
          </p>
        </div>
      </header>

      <div class="flex max-w-2xl items-center justify-between gap-5">
        <p class="text-sm text-muted-foreground">
          {repository.repository?.archived_at === null
            ? "This repository accepts pushes."
            : "This repository is archived and read-only."}
        </p>
        <Switch
          checked={repository.repository?.archived_at !== null}
          disabled={repository.settings.lifecyclePending}
          aria-label="Archive repository"
          onclick={() =>
            void repository.settings.setArchived(
              repository.repository?.archived_at === null,
            )}
        />
      </div>
    </section>

    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
      aria-labelledby="repository-delete-heading"
    >
      <header class="flex items-start gap-3">
        <TriangleAlert class="mt-0.5 size-4 shrink-0 text-destructive" />
        <div>
          <h2 id="repository-delete-heading" class="font-semibold">
            Delete repository
          </h2>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Hide it from browsing and cloning. Data stays recoverable until a
            separate permanent purge.
          </p>
        </div>
      </header>

      <div
        class="flex max-w-2xl flex-wrap items-center justify-between gap-4 rounded-md border border-destructive/30 bg-destructive/5 p-4"
      >
        <p class="text-sm text-muted-foreground">
          This cannot be undone once the recovery period lapses.
        </p>
        <Button
          type="button"
          variant="destructive"
          class="gap-2"
          disabled={repository.settings.lifecyclePending}
          onclick={() => (deleteDialogOpen = true)}
        >
          <Trash2 class="size-3.5" />Delete repository
        </Button>
      </div>
    </section>
  </div>
{/if}

<AlertDialog.Root bind:open={moveDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Move this repository?</AlertDialog.Title>
      <AlertDialog.Description>
        Its Git data will stay in place and existing URLs will remain aliases.
        Direct collaborators will be cleared so permissions are recalculated for
        the new namespace.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action onclick={() => void confirmMoveRepository()}
        >Move repository</AlertDialog.Action
      >
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root bind:open={deleteDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title
        >Delete {repository.repository?.namespace ?? ""}/{repository.repository
          ?.name ?? ""}?</AlertDialog.Title
      >
      <AlertDialog.Description>
        This will soft-delete the repository. It can be restored during the
        recovery period, but will be hidden from browsing and cloning.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => void confirmDeleteRepository()}
        >Delete repository</AlertDialog.Action
      >
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
