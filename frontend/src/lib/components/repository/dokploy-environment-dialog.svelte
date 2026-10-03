<script lang="ts">
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import {
    NEW_DOKPLOY_PROJECT,
    type DokployConfigureState,
  } from "$lib/repository/state/integrations-state.svelte.js";

  let { configure }: { configure: DokployConfigureState } = $props();
</script>

<Dialog.Root bind:open={configure.environmentDialogOpen}>
  <Dialog.Content>
    <form
      onsubmit={(event) => {
        event.preventDefault();
        void configure.createEnvironment();
      }}
    >
      <Dialog.Header>
        <Dialog.Title>Create environment</Dialog.Title>
        <Dialog.Description>
          Name the environment, then choose an existing parent project or create
          a new one.
        </Dialog.Description>
      </Dialog.Header>
      <Field.Group class="py-5">
        <Field.Field>
          <Field.Label for="dokploy-environment-name">
            New environment name
          </Field.Label>
          <Input
            id="dokploy-environment-name"
            bind:value={configure.environmentName}
            placeholder="staging"
            required
          />
          <Field.Description>
            This creates a new environment inside the selected project.
          </Field.Description>
        </Field.Field>
        <Field.Field>
          <Field.Label for="dokploy-environment-project">
            Parent project
          </Field.Label>
          <Select.Root
            type="single"
            bind:value={configure.environmentProjectId}
          >
            <Select.Trigger id="dokploy-environment-project" class="w-full">
              {configure.environmentProjectId === NEW_DOKPLOY_PROJECT
                ? "Create new project"
                : (configure.catalog?.projects.find(
                    (project) => project.id === configure.environmentProjectId,
                  )?.name ?? "Select a project")}
            </Select.Trigger>
            <Select.Content>
              <Select.Group>
                <Select.Item value={NEW_DOKPLOY_PROJECT}>
                  Create new project
                </Select.Item>
                {#if (configure.catalog?.projects.length ?? 0) > 0}
                  <Select.Separator />
                  {#each configure.catalog?.projects ?? [] as project (project.id)}
                    <Select.Item value={project.id}>{project.name}</Select.Item>
                  {/each}
                {/if}
              </Select.Group>
            </Select.Content>
          </Select.Root>
          <Field.Description>
            {configure.environmentProjectId === NEW_DOKPLOY_PROJECT
              ? "Dokploy automatically adds a production environment to every new project."
              : "Dokploy requires every environment to belong to a project."}
          </Field.Description>
        </Field.Field>
        {#if configure.environmentProjectId === NEW_DOKPLOY_PROJECT}
          <Field.Field>
            <Field.Label for="dokploy-project-name">
              New project name
            </Field.Label>
            <Input
              id="dokploy-project-name"
              bind:value={configure.environmentProjectName}
              placeholder="Gitadel"
              required
            />
          </Field.Field>
        {/if}
        <Field.Field>
          <Field.Label for="dokploy-environment-description">
            Description <span class="font-normal text-muted-foreground"
              >Optional</span
            >
          </Field.Label>
          <Input
            id="dokploy-environment-description"
            bind:value={configure.environmentDescription}
            placeholder="Preview deployments"
          />
        </Field.Field>
      </Field.Group>
      <Dialog.Footer>
        <Button
          type="button"
          variant="outline"
          onclick={() => (configure.environmentDialogOpen = false)}
        >
          Cancel
        </Button>
        <Button
          type="submit"
          disabled={configure.working ||
            !configure.environmentProjectId ||
            !configure.environmentName.trim() ||
            (configure.environmentProjectId === NEW_DOKPLOY_PROJECT &&
              !configure.environmentProjectName.trim())}
        >
          {#if configure.working}<Spinner class="size-4 animate-spin" />{/if}
          Create environment
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
