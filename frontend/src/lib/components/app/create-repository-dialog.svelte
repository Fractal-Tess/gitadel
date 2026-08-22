<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";

  import {
    ApiFailure,
    jsonBody,
    repositorySchema,
    requestJson,
    type Organization,
  } from "$lib/api.js";
  import {
    invalidateExplore,
    loadOrganizations,
  } from "$lib/navigation-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  const app = useAppState();
  const shell = useShellState();
  const viewer = $derived(app.authStatus?.user?.username);

  let organizations = $state.raw<Organization[]>([]);
  let organizationsLoadedFor: string | null = null;
  let wasOpen = false;
  let creating = $state(false);
  let error = $state<string | null>(null);
  let namespace = $state("");
  let name = $state("");
  let description = $state("");
  let visibility = $state<"public" | "private">("private");

  // This component persists across account changes. Refresh owners once per
  // opening and reset account-scoped form state when the viewer changes.
  $effect(() => {
    const requestedViewer = viewer;
    const open = shell.createOpen;
    const viewerChanged = organizationsLoadedFor !== requestedViewer;
    if (viewerChanged) {
      organizationsLoadedFor = requestedViewer ?? null;
      organizations = [];
      namespace = requestedViewer ?? "";
    }
    if (!open) {
      wasOpen = false;
      return;
    }
    if (wasOpen && !viewerChanged) return;
    wasOpen = true;

    error = null;
    namespace ||= requestedViewer ?? "";
    visibility = app.instance?.default_repository_visibility ?? "private";
    if (!requestedViewer) return;

    void loadOrganizations(requestedViewer)
      .then((loaded) => {
        if (viewer !== requestedViewer) return;
        organizations = loaded.filter((item) => item.role === "owner");
      })
      .catch(() => undefined);
  });

  async function createRepository(): Promise<void> {
    creating = true;
    error = null;
    try {
      const repository = await requestJson(
        "/api/v1/repositories",
        repositorySchema,
        {
          method: "POST",
          body: jsonBody({
            namespace,
            name,
            description: description || null,
            visibility,
            object_format: "sha1",
          }),
        },
      );
      shell.createOpen = false;
      name = "";
      description = "";
      invalidateExplore(viewer);
      await goto(
        resolve("/[namespace]/[name]", {
          namespace: repository.namespace,
          name: repository.name,
        }),
      );
    } catch (caught) {
      error =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not create repository.";
    } finally {
      creating = false;
    }
  }
</script>

<Dialog.Root bind:open={shell.createOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>Create a repository</Dialog.Title>
      <Dialog.Description>
        Create an empty Git repository, then push your project over SSH.
      </Dialog.Description>
    </Dialog.Header>
    <form
      class="grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void createRepository();
      }}
    >
      {#if error}
        <p
          class="rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive"
        >
          {error}
        </p>
      {/if}
      <div class="grid gap-4 sm:grid-cols-[minmax(0,0.8fr)_minmax(0,1.2fr)]">
        <Field.Field>
          <Field.Label for="repository-namespace">Owner</Field.Label>
          <Select.Root
            type="single"
            value={namespace}
            onValueChange={(value) => {
              if (value) namespace = value;
            }}
          >
            <Select.Trigger id="repository-namespace" class="w-full">
              {namespace || "Select an owner"}
            </Select.Trigger>
            <Select.Content>
              {#if viewer}
                <Select.Item value={viewer}>{viewer}</Select.Item>
              {/if}
              {#each organizations as organization (organization.id)}
                <Select.Item value={organization.slug}>
                  {organization.slug}
                </Select.Item>
              {/each}
            </Select.Content>
          </Select.Root>
        </Field.Field>
        <Field.Field>
          <Field.Label for="repository-name">Repository name</Field.Label>
          <Input
            id="repository-name"
            bind:value={name}
            maxlength={100}
            placeholder="project-name"
            required
          />
        </Field.Field>
      </div>
      <Field.Field>
        <Field.Label for="repository-description">Description</Field.Label>
        <Textarea
          id="repository-description"
          bind:value={description}
          maxlength={512}
          placeholder="What is this project for?"
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="repository-visibility">Visibility</Field.Label>
        <Select.Root
          type="single"
          value={visibility}
          onValueChange={(value) => {
            if (value === "public" || value === "private") visibility = value;
          }}
        >
          <Select.Trigger id="repository-visibility" class="w-full">
            {visibility === "private" ? "Private" : "Public"}
          </Select.Trigger>
          <Select.Content>
            <Select.Item value="private">Private</Select.Item>
            <Select.Item value="public">Public</Select.Item>
          </Select.Content>
        </Select.Root>
      </Field.Field>
      <Dialog.Footer>
        <Dialog.Close>
          {#snippet child({ props })}
            <Button {...props} type="button" variant="outline">Cancel</Button>
          {/snippet}
        </Dialog.Close>
        <Button type="submit" disabled={creating || !namespace}>
          {creating ? "Creating…" : "Create repository"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
