<script lang="ts">
  import { resolve } from "$app/paths";
  import Building2 from "@lucide/svelte/icons/building-2";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import Settings2 from "@lucide/svelte/icons/settings-2";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const account = new AccountSettingsState(app);
  const organizationState = account.organization;
  let createDialogOpen = $state(false);

  $effect(() => {
    account.syncScope();
    void account.initialize("account");
  });

  async function createOrganization(): Promise<void> {
    await organizationState.createOrganization();
    if (!organizationState.error) createDialogOpen = false;
  }
</script>

<svelte:head>
  <title>Organizations · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<div class="mx-auto max-w-5xl px-5 py-8 lg:px-8">
  <header class="mb-6 flex flex-wrap items-start justify-between gap-4">
    <div>
      <h1 class="text-xl font-semibold tracking-tight">Organizations</h1>
      <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
        Open an organization and manage its repositories and shared resources.
      </p>
    </div>
    <Button class="gap-2" onclick={() => (createDialogOpen = true)}>
      <Building2 class="size-4" />New organization
    </Button>
  </header>

  {#if account.loading}
    <p class="py-16 text-center text-sm text-muted-foreground">
      Loading organizations…
    </p>
  {:else}
    <div class="overflow-hidden rounded-xl border bg-card/30 shadow-sm">
      {#each organizationState.organizations as organization (organization.id)}
        <article
          class="flex flex-wrap items-center gap-4 border-b p-4 last:border-b-0"
        >
          <span
            class="grid size-10 shrink-0 place-items-center rounded-lg border bg-muted"
          >
            <Building2 class="size-5 text-primary" />
          </span>
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <h2 class="truncate text-sm font-medium">
                {organization.display_name}
              </h2>
              <span
                class="rounded-full border px-2 py-0.5 text-[0.6875rem] capitalize text-muted-foreground"
                >{organization.role}</span
              >
            </div>
            <p class="mt-0.5 truncate font-mono text-xs text-muted-foreground">
              {organization.slug}
            </p>
          </div>
          <div class="flex items-center gap-2">
            <Button
              variant="ghost"
              size="sm"
              class="gap-2"
              href={resolve("/[namespace]", { namespace: organization.slug })}
            >
              <GitBranch class="size-4" />Repositories
            </Button>
            {#if organization.role === "owner"}
              <Button
                variant="outline"
                size="sm"
                class="gap-2"
                href={resolve("/[namespace]/members", {
                  namespace: organization.slug,
                })}
              >
                <Settings2 class="size-4" />Manage
              </Button>
            {/if}
          </div>
        </article>
      {:else}
        <p class="p-5 text-sm text-muted-foreground">
          You do not belong to an organization yet.
        </p>
      {/each}
    </div>
  {/if}

  <Dialog.Root bind:open={createDialogOpen}>
    <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
      <Dialog.Header>
        <Dialog.Title>New organization</Dialog.Title>
        <Dialog.Description
          >Create a shared repository namespace. You will be its first owner.</Dialog.Description
        >
      </Dialog.Header>
      <form
        class="grid gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void createOrganization();
        }}
      >
        <Field.Field>
          <Field.Label for="organization-slug">Namespace</Field.Label>
          <Input
            id="organization-slug"
            bind:value={organizationState.organizationSlug}
            placeholder="acme"
            autofocus
            required
          />
          <Field.Description
            >Used in repository URLs. It cannot be changed later.</Field.Description
          >
        </Field.Field>
        <Field.Field>
          <Field.Label for="organization-display-name">Display name</Field.Label
          >
          <Input
            id="organization-display-name"
            bind:value={organizationState.organizationDisplayName}
            placeholder="Acme"
            required
          />
        </Field.Field>
        <Dialog.Footer>
          <Dialog.Close>
            {#snippet child({ props })}<Button
                {...props}
                type="button"
                variant="outline">Cancel</Button
              >{/snippet}
          </Dialog.Close>
          <Button type="submit" disabled={organizationState.working}
            >{organizationState.working ? "Creating…" : "Create organization"}</Button
          >
        </Dialog.Footer>
      </form>
    </Dialog.Content>
  </Dialog.Root>
</div>
