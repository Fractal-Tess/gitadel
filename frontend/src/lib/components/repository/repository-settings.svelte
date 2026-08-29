<script lang="ts">
  import Archive from "@lucide/svelte/icons/archive";
  import MapPin from "@lucide/svelte/icons/map-pin";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";

  import RepositoryIntegrationSettings from "$lib/components/repository/repository-integration-settings.svelte";
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
    const names = repository.browser.refs?.branches.map((branch) => branch.name) ?? [];
    return names.includes(defaultBranch) || !defaultBranch
      ? names
      : [defaultBranch, ...names];
  });

  $effect(() => {
    const current = repository.repository;
    if (!current || initializedFor === current.id) return;
    initializedFor = current.id;
    visibility = current.visibility;
    defaultBranch = current.default_branch;
    repositoryName = current.name;
    targetNamespace = current.namespace;
  });

  async function saveGeneral() {
    const current = repository.repository;
    if (!current) return;
    try {
      await repository.settings.updateRepositoryControl({
        ...(visibility !== current.visibility && { visibility }),
        ...(defaultBranch !== current.default_branch && {
          default_branch: defaultBranch,
        }),
      });
    } catch {
      // The page-level error region explains how to recover.
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
      // The page-level error region explains how to recover.
    }
  }

  async function confirmDeleteRepository() {
    try {
      await repository.settings.softDelete();
      deleteDialogOpen = false;
    } catch {
      // The page-level error region explains how to recover.
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
          <Button type="submit" disabled={repository.settings.repositoryControlPending}>
            {repository.settings.repositoryControlPending
              ? "Saving…"
              : "Save general settings"}
          </Button>
        </div>
      </form>
    </section>
  </div>
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
            {repository.settings.repositoryControlPending ? "Moving…" : "Save location"}
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
