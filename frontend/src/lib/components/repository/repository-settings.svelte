<script lang="ts">
  import { Archive, MapPin, Settings2, Trash2 } from "lucide-svelte";

  import RepositoryWebhookSettings from "$lib/components/repository/repository-webhook-settings.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: repository }: { state: RepositoryPageState } = $props();
  let description = $state("");
  let visibility = $state<"public" | "private">("private");
  let defaultBranch = $state("");
  let repositoryName = $state("");
  let targetNamespace = $state("");
  let initializedFor = $state("");

  $effect(() => {
    const current = repository.repository;
    if (!current || initializedFor === current.id) return;
    initializedFor = current.id;
    description = current.description ?? "";
    visibility = current.visibility;
    defaultBranch = current.default_branch;
    repositoryName = current.name;
    targetNamespace = current.namespace;
  });

  async function saveGeneral() {
    const current = repository.repository;
    if (!current) return;
    try {
      await repository.updateRepositoryControl({
        ...(description !== (current.description ?? "") && {
          description: description || null,
        }),
        ...(visibility !== current.visibility && { visibility }),
        ...(defaultBranch !== current.default_branch && {
          default_branch: defaultBranch,
        }),
      });
    } catch {
      // The page-level error region explains how to recover.
    }
  }

  async function moveRepository() {
    if (
      !globalThis.confirm(
        "Move this repository? Its Git data will stay in place and existing URLs will remain aliases.",
      )
    )
      return;
    try {
      await repository.updateRepositoryControl({
        name: repositoryName,
        namespace: targetNamespace,
      });
    } catch {
      // The page-level error region explains how to recover.
    }
  }

  async function deleteRepository() {
    const current = repository.repository;
    if (!current) return;
    if (
      !globalThis.confirm(
        `Soft-delete ${current.namespace}/${current.name}? It can be restored during the recovery period.`,
      )
    )
      return;
    try {
      await repository.softDelete();
    } catch {
      // The page-level error region explains how to recover.
    }
  }
</script>

<div class="mx-auto grid max-w-4xl gap-5">
  <Card.Root>
    <Card.Header class="border-b">
      <div class="flex items-start gap-3">
        <Settings2 class="mt-0.5 size-4 text-foreground/70" />
        <div>
          <h1 class="text-base font-medium">General settings</h1>
          <Card.Description
            >Edit repository metadata and its Git default.</Card.Description
          >
        </div>
      </div>
    </Card.Header>
    <Card.Content>
      <form
        class="grid gap-5"
        onsubmit={(event) => {
          event.preventDefault();
          void saveGeneral();
        }}
      >
        <Field.Field>
          <Field.Label for="repository-description">Description</Field.Label>
          <textarea
            id="repository-description"
            class="min-h-24 w-full rounded-md border bg-transparent px-3 py-2 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
            bind:value={description}
            maxlength={512}
            placeholder="What is this repository for?"></textarea>
        </Field.Field>
        <div class="grid gap-5 sm:grid-cols-2">
          <Field.Field>
            <Field.Label>Visibility</Field.Label>
            <Select.Root type="single" bind:value={visibility}>
              <Select.Trigger class="w-full"
                >{visibility === "public"
                  ? "Public — visible to everyone"
                  : "Private — restricted access"}</Select.Trigger
              >
              <Select.Content
                ><Select.Item value="public"
                  >Public — visible to everyone</Select.Item
                ><Select.Item value="private"
                  >Private — restricted access</Select.Item
                ></Select.Content
              >
            </Select.Root>
          </Field.Field>
          <Field.Field>
            <Field.Label for="default-branch">Default branch</Field.Label>
            <Input
              id="default-branch"
              bind:value={defaultBranch}
              maxlength={255}
              required
            />
            <Field.Description
              >Must be an existing branch. Saving updates Git’s symbolic HEAD.</Field.Description
            >
          </Field.Field>
        </div>
        <div class="flex justify-end">
          <Button type="submit" disabled={repository.repositoryControlPending}
            >{repository.repositoryControlPending
              ? "Saving…"
              : "Save general settings"}</Button
          >
        </div>
      </form>
    </Card.Content>
  </Card.Root>

  <Card.Root>
    <Card.Header class="border-b"
      ><div class="flex items-start gap-3">
        <MapPin class="mt-0.5 size-4 text-foreground/70" />
        <div>
          <h2 class="text-base font-medium">Repository location</h2>
          <Card.Description
            >Rename or move this repository to a namespace you own.</Card.Description
          >
        </div>
      </div></Card.Header
    >
    <Card.Content>
      <form
        class="grid gap-5"
        onsubmit={(event) => {
          event.preventDefault();
          void moveRepository();
        }}
      >
        <div class="grid gap-5 sm:grid-cols-2">
          <Field.Field
            ><Field.Label for="repository-name">Repository name</Field.Label
            ><Input
              id="repository-name"
              bind:value={repositoryName}
              maxlength={100}
              required
            /></Field.Field
          >
          <Field.Field
            ><Field.Label>Namespace</Field.Label><Select.Root
              type="single"
              bind:value={targetNamespace}
              ><Select.Trigger class="w-full">{targetNamespace}</Select.Trigger
              ><Select.Content
                >{#each repository.ownedNamespaces as namespace (namespace)}<Select.Item
                    value={namespace}>{namespace}</Select.Item
                  >{/each}</Select.Content
              ></Select.Root
            ></Field.Field
          >
        </div>
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
            disabled={repository.repositoryControlPending}
            >{repository.repositoryControlPending
              ? "Moving…"
              : "Save location"}</Button
          >
        </div>
      </form>
    </Card.Content>
  </Card.Root>

  <RepositoryWebhookSettings state={repository} />

  <Card.Root>
    <Card.Header class="border-b"
      ><div class="flex items-start gap-3">
        <Archive class="mt-0.5 size-4 text-foreground/70" />
        <div>
          <h2 class="text-base font-medium">Repository lifecycle</h2>
          <Card.Description
            >Control availability without immediately destroying data.</Card.Description
          >
        </div>
      </div></Card.Header
    >
    <Card.Content class="grid gap-4">
      <div
        class="flex items-center justify-between gap-5 rounded-md border p-4"
      >
        <div>
          <p class="text-sm font-medium">Archive repository</p>
          <p class="mt-1 text-xs leading-5 text-muted-foreground">
            Archived repositories remain cloneable, but reject all pushes.
          </p>
        </div>
        <Switch
          checked={repository.repository?.archived_at !== null}
          disabled={repository.lifecyclePending}
          aria-label="Archive repository"
          onclick={() =>
            void repository.setArchived(
              repository.repository?.archived_at === null,
            )}
        />
      </div>
      <div
        class="flex flex-wrap items-center justify-between gap-4 rounded-md border border-destructive/30 bg-destructive/5 p-4"
      >
        <div>
          <p class="text-sm font-medium">Soft-delete repository</p>
          <p class="mt-1 text-xs leading-5 text-muted-foreground">
            Hide it from browsing and cloning. Data stays recoverable until a
            separate permanent purge.
          </p>
        </div>
        <Button
          type="button"
          variant="destructive"
          class="gap-2"
          disabled={repository.lifecyclePending}
          onclick={() => void deleteRepository()}
          ><Trash2 class="size-3.5" />Delete repository</Button
        >
      </div>
    </Card.Content>
  </Card.Root>
</div>
