<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import ArrowDownToLine from "@lucide/svelte/icons/arrow-down-to-line";
  import Building2 from "@lucide/svelte/icons/building-2";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";

  import { avatarUrl } from "$lib/api/account.js";
  import { organizationAvatarUrl } from "$lib/api/organizations.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Avatar from "$lib/components/ui/avatar/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";

  import {
    ApiFailure,
    jsonBody,
    requestJson,
  } from "$lib/api/transport.js";
  import { mirrorIdentitiesSchema, type MirrorIdentity } from "$lib/api/mirrors.js";
  import { organizationSchema } from "$lib/api/organizations.js";
  import { repositorySchema } from "$lib/api/repositories.js";
  import { invalidateExplore } from "$lib/navigation-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  type CreateMode = "choose" | "repository" | "mirror" | "organization";
  type MirrorIdentityOption = MirrorIdentity & {
    namespace: string;
    namespaceLabel: string;
  };

  const app = useAppState();
  const shell = useShellState();
  const viewer = $derived(app.authStatus?.user?.username);
  const organizations = $derived(
    app.organizations.filter((organization) => organization.role === "owner"),
  );
  const ownerOptions = $derived.by(() => {
    const user = app.authStatus?.user;
    return [
      ...(user
        ? [
            {
              slug: user.username,
              label: user.username,
              description: "Personal namespace",
              imageUrl: avatarUrl(user.id, user.avatar_updated_at),
              organization: false,
            },
          ]
        : []),
      ...organizations.map((organization) => ({
        slug: organization.slug,
        label: organization.display_name || organization.slug,
        description: organization.slug,
        imageUrl: organizationAvatarUrl(
          organization.slug,
          organization.avatar_updated_at,
        ),
        organization: true,
      })),
    ];
  });
  const selectedOwner = $derived(
    ownerOptions.find((owner) => owner.slug === namespace) ?? null,
  );

  let mode = $state<CreateMode>("choose");
  let wasOpen = false;
  let ownerFor: string | null = null;
  let creating = $state(false);
  let error = $state<string | null>(null);
  let namespace = $state("");
  let name = $state("");
  let description = $state("");
  let visibility = $state<"public" | "private">("private");
  let mirrorRemoteUrl = $state("");
  let mirrorIdentityId = $state("");
  let mirrorSchedule = $state("");
  let mirrorIdentities = $state<MirrorIdentityOption[]>([]);
  let mirrorOptionsLoading = $state(false);
  let organizationSlug = $state("");
  let organizationDisplayName = $state("");

  const selectedMirrorIdentity = $derived(
    mirrorIdentities.find((identity) => identity.id === mirrorIdentityId) ?? null,
  );

  $effect(() => {
    const requestedViewer = viewer;
    if (ownerFor === requestedViewer) return;
    ownerFor = requestedViewer ?? null;
    namespace = requestedViewer ?? "";
  });

  $effect(() => {
    const open = shell.createOpen;
    if (!open) {
      wasOpen = false;
      return;
    }
    if (wasOpen) return;
    wasOpen = true;
    mode = "choose";
    error = null;
    namespace ||= viewer ?? "";
    visibility =
      app.authStatus?.user?.default_repository_visibility ?? "private";
    void app.refreshOrganizations().catch(() => undefined);
  });

  function message(caught: unknown, fallback: string): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  function identitiesPath(target: string): string {
    return `/api/v1/namespaces/${encodeURIComponent(target)}/mirror-identities`;
  }


  function suggestedRepositoryName(url: string): string {
    const sshMatch = url.trim().match(/^git@[^:]+:(?:.+\/)?(.+?)(?:\.git)?$/i);
    if (sshMatch) return sshMatch[1];
    try {
      const path = new URL(url).pathname.replace(/\/+$/, "");
      return (
        path
          .split("/")
          .pop()
          ?.replace(/\.git$/i, "") ?? ""
      );
    } catch {
      return "";
    }
  }

  async function loadMirrorOptions(): Promise<void> {
    mirrorOptionsLoading = true;
    try {
      const loaded = await Promise.all(
        ownerOptions.map(async (owner) => {
          const identities = await requestJson(
            identitiesPath(owner.slug),
            mirrorIdentitiesSchema,
          );
          return identities.map((identity) => ({
            ...identity,
            namespace: owner.slug,
            namespaceLabel: owner.label,
          }));
        }),
      );
      mirrorIdentities = loaded.flat();
    } catch (caught) {
      error = message(caught, "Could not load mirror identities.");
    } finally {
      mirrorOptionsLoading = false;
    }
  }

  function selectMode(next: Exclude<CreateMode, "choose">): void {
    error = null;
    mode = next;
    if (next === "mirror") {
      mirrorIdentityId = "";
      void loadMirrorOptions();
    }
  }
  function backToChoices(): void {
    error = null;
    mode = "choose";
  }

  function selectNamespace(next: string | undefined): void {
    if (!next || (mode === "mirror" && mirrorIdentityId)) return;
    namespace = next;
  }

  function selectMirrorIdentity(next: string | undefined): void {
    mirrorIdentityId = next ?? "";
    const identity = mirrorIdentities.find((candidate) => candidate.id === next);
    if (identity) namespace = identity.namespace;
  }

  function updateMirrorRemoteUrl(value: string): void {
    const priorSuggestedName = suggestedRepositoryName(mirrorRemoteUrl);
    mirrorRemoteUrl = value;
    const suggestedName = suggestedRepositoryName(value);
    if (suggestedName && (!name || name === priorSuggestedName))
      name = suggestedName;
  }

  function identityLabel(identity: MirrorIdentityOption): string {
    const provider = {
      github: "GitHub",
      gitlab: "GitLab",
      gitea: "Gitea",
      forgejo: "Forgejo",
    }[identity.provider ?? "github"];
    return `${identity.name} · ${provider} · ${identity.namespaceLabel}`;
  }

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
            ...(mode === "mirror" && {
              mirror: {
                remote_url: mirrorRemoteUrl,
                identity_id: mirrorIdentityId || null,
                schedule: mirrorSchedule || null,
              },
            }),
          }),
        },
      );
      shell.createOpen = false;
      name = "";
      description = "";
      mirrorRemoteUrl = "";
      mirrorIdentityId = "";
      mirrorSchedule = "";
      invalidateExplore(app.authorizationScope);
      await goto(
        resolve("/[namespace]/[name]", {
          namespace: repository.namespace,
          name: repository.name,
        }),
      );
    } catch (caught) {
      error = message(caught, "Could not create repository.");
    } finally {
      creating = false;
    }
  }

  async function createOrganization(): Promise<void> {
    creating = true;
    error = null;
    try {
      const organization = await requestJson(
        "/api/v1/organizations",
        organizationSchema,
        {
          method: "POST",
          body: jsonBody({
            slug: organizationSlug,
            display_name: organizationDisplayName,
          }),
        },
      );
      app.addOrganization(organization);
      shell.createOpen = false;
      organizationSlug = "";
      organizationDisplayName = "";
      await goto(resolve("/[namespace]", { namespace: organization.slug }));
    } catch (caught) {
      error = message(caught, "Could not create organization.");
    } finally {
      creating = false;
    }
  }
</script>

<Dialog.Root bind:open={shell.createOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-2xl">
    {#if mode === "choose"}
      <Dialog.Header>
        <Dialog.Title>Create new</Dialog.Title>
        <Dialog.Description>
          Create a repository, mirror an upstream, import from another forge, or
          create an organization.
        </Dialog.Description>
      </Dialog.Header>
      <div class="grid gap-3">
        <button
          type="button"
          class="group flex min-h-32 items-center gap-5 rounded-xl border bg-card/25 p-5 text-left transition-colors hover:border-foreground/25 hover:bg-card/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onclick={() => selectMode("repository")}
        >
          <span
            class="grid size-12 shrink-0 place-items-center rounded-full border bg-background transition-transform group-hover:scale-105"
          >
            <GitBranch class="size-5" />
          </span>
          <span>
            <span class="font-medium">New repository</span>
            <span
              class="mt-1.5 block max-w-lg text-sm leading-5 text-muted-foreground"
            >
              Create an empty Git repository in your namespace or an
              organization.
            </span>
          </span>
        </button>
        <div class="grid gap-3 sm:grid-cols-3">
          <button
            type="button"
            class="group flex min-h-40 flex-col items-center justify-center rounded-xl border bg-card/25 p-5 text-center transition-colors hover:border-foreground/25 hover:bg-card/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            onclick={() => selectMode("mirror")}
          >
            <span
              class="grid size-12 place-items-center rounded-full border bg-background transition-transform group-hover:scale-105"
            >
              <RefreshCw class="size-5" />
            </span>
            <span class="mt-4 font-medium">Mirror repository</span>
            <span
              class="mt-1.5 max-w-52 text-sm leading-5 text-muted-foreground"
            >
              Clone an upstream and keep every ref synchronized.
            </span>
          </button>
          <button
            type="button"
            class="group flex min-h-40 flex-col items-center justify-center rounded-xl border bg-card/25 p-5 text-center transition-colors hover:border-foreground/25 hover:bg-card/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            onclick={() => selectMode("organization")}
          >
            <span
              class="grid size-12 place-items-center rounded-full border bg-background transition-transform group-hover:scale-105"
            >
              <Building2 class="size-5" />
            </span>
            <span class="mt-4 font-medium">New organization</span>
            <span
              class="mt-1.5 max-w-52 text-sm leading-5 text-muted-foreground"
            >
              Create a shared namespace for repositories and members.
            </span>
          </button>
          <button
            type="button"
            class="group flex min-h-40 flex-col items-center justify-center rounded-xl border bg-card/25 p-5 text-center transition-colors hover:border-foreground/25 hover:bg-card/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            onclick={() => {
              shell.createOpen = false;
              void goto(resolve("/imports/new"));
            }}
          >
            <span
              class="grid size-12 place-items-center rounded-full border bg-background transition-transform group-hover:scale-105"
            >
              <ArrowDownToLine class="size-5" />
            </span>
            <span class="mt-4 font-medium">Import repositories</span>
            <span
              class="mt-1.5 max-w-52 text-sm leading-5 text-muted-foreground"
            >
              Choose repositories from GitHub, GitLab, Gitea, or Forgejo.
            </span>
          </button>
        </div>
      </div>
    {:else if mode === "repository" || mode === "mirror"}
      <Dialog.Header>
        <Dialog.Title>
          {mode === "mirror" ? "Mirror a repository" : "Create a repository"}
        </Dialog.Title>
        <Dialog.Description>
          {mode === "mirror"
            ? "Create a read-only copy that tracks every upstream ref."
            : "Create an empty Git repository, then push your project over SSH."}
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
        {#if mode === "mirror"}
          <Field.Field>
            <Field.Label for="mirror-identity">Mirror identity</Field.Label>
            <Select.Root
              type="single"
              value={mirrorIdentityId}
              onValueChange={selectMirrorIdentity}
              disabled={mirrorOptionsLoading}
            >
              <Select.Trigger id="mirror-identity" class="w-full">
                {selectedMirrorIdentity
                  ? identityLabel(selectedMirrorIdentity)
                  : "Public"}
              </Select.Trigger>
              <Select.Content>
                <Select.Item value="">Public</Select.Item>
                {#each mirrorIdentities as identity (identity.id)}
                  <Select.Item value={identity.id}>
                    {identityLabel(identity)}
                  </Select.Item>
                {/each}
              </Select.Content>
            </Select.Root>
            <Field.Description>
              Public mirrors send no credential. Selecting a token identity
              also selects and locks its personal or organization owner.
            </Field.Description>
          </Field.Field>

          <Field.Field>
            <Field.Label for="mirror-remote-url">Upstream URL</Field.Label>
            <Input
              id="mirror-remote-url"
              type="url"
              value={mirrorRemoteUrl}
              oninput={(event) =>
                updateMirrorRemoteUrl(event.currentTarget.value)}
              placeholder="https://github.com/owner/repository.git"
              required
            />
            <Field.Description>
              Use the HTTPS clone URL. The repository name is filled from this
              URL and can still be changed.
            </Field.Description>
          </Field.Field>

          <div class="rounded-lg border bg-background/40 p-4">
            <p class="text-sm font-medium">Mirror identities</p>
            <p class="mt-1 text-xs text-muted-foreground">
              Add a server token once, then reuse it for private mirrors in the
              same namespace.
            </p>
            <Button
              type="button"
              variant="outline"
              size="sm"
              class="mt-3"
              onclick={() => {
                shell.createOpen = false;
                void goto(
                  resolve("/[namespace]/mirror-credentials", { namespace }),
                );
              }}
            >
              Manage identities
            </Button>
          </div>


          <Field.Field>
            <Field.Label for="mirror-schedule">Synchronization</Field.Label>
            <Select.Root type="single" bind:value={mirrorSchedule}>
              <Select.Trigger id="mirror-schedule" class="w-full">
                {mirrorSchedule === ""
                  ? "Manual only"
                  : mirrorSchedule === "0 0 * * * *"
                    ? "Every hour"
                    : mirrorSchedule === "0 0 */6 * * *"
                      ? "Every 6 hours"
                      : "Every day at 02:00 UTC"}
              </Select.Trigger>
              <Select.Content>
                <Select.Item value="">Manual only</Select.Item>
                <Select.Item value="0 0 * * * *">Every hour</Select.Item>
                <Select.Item value="0 0 */6 * * *">Every 6 hours</Select.Item>
                <Select.Item value="0 0 2 * * *"
                  >Every day at 02:00 UTC</Select.Item
                >
              </Select.Content>
            </Select.Root>
          </Field.Field>
        {/if}
        <div class="grid gap-4 sm:grid-cols-[minmax(0,0.8fr)_minmax(0,1.2fr)]">
          <Field.Field>
            <Field.Label for="repository-namespace">Owner</Field.Label>
            <Select.Root
              type="single"
              value={namespace}
              onValueChange={selectNamespace}
              disabled={mode === "mirror" && Boolean(mirrorIdentityId)}
            >
              <Select.Trigger
                id="repository-namespace"
                class="h-auto min-h-9 w-full py-1.5"
              >
                {#if selectedOwner}
                  <span class="flex min-w-0 items-center gap-2 text-left">
                    <Avatar.Root class="size-6">
                      {#if selectedOwner.imageUrl}
                        <Avatar.Image src={selectedOwner.imageUrl} alt="" />
                      {/if}
                      <Avatar.Fallback>
                        {#if selectedOwner.organization}
                          <Building2 class="size-3.5" />
                        {:else}
                          <GitBranch class="size-3.5" />
                        {/if}
                      </Avatar.Fallback>
                    </Avatar.Root>
                    <span class="min-w-0">
                      <span class="block truncate">{selectedOwner.label}</span>
                      <span class="block truncate text-xs text-muted-foreground"
                        >{selectedOwner.description}</span
                      >
                    </span>
                  </span>
                {:else}
                  Select an owner
                {/if}
              </Select.Trigger>
              <Select.Content>
                {#each ownerOptions as owner (owner.slug)}
                  <Select.Item value={owner.slug}>
                    <span class="flex min-w-0 items-center gap-2">
                      <Avatar.Root class="size-7">
                        {#if owner.imageUrl}
                          <Avatar.Image src={owner.imageUrl} alt="" />
                        {/if}
                        <Avatar.Fallback>
                          {#if owner.organization}
                            <Building2 class="size-3.5" />
                          {:else}
                            <GitBranch class="size-3.5" />
                          {/if}
                        </Avatar.Fallback>
                      </Avatar.Root>
                      <span class="min-w-0">
                        <span class="block truncate font-medium"
                          >{owner.label}</span
                        >
                        <span
                          class="block truncate text-xs text-muted-foreground"
                          >{owner.description}</span
                        >
                      </span>
                    </span>
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
        <Dialog.Footer class="sm:justify-between">
          <Button type="button" variant="ghost" onclick={backToChoices}>
            <ArrowLeft class="size-4" />Back
          </Button>
          <div class="flex justify-end gap-2">
            <Dialog.Close>
              {#snippet child({ props })}
                <Button {...props} type="button" variant="outline"
                  >Cancel</Button
                >
              {/snippet}
            </Dialog.Close>
            <Button
              type="submit"
              disabled={creating ||
                !namespace ||
                (mode === "mirror" && !mirrorRemoteUrl)}
            >
              {creating
                ? mode === "mirror"
                  ? "Mirroring…"
                  : "Creating…"
                : mode === "mirror"
                  ? "Mirror repository"
                  : "Create repository"}
            </Button>
          </div>
        </Dialog.Footer>
      </form>
    {:else}
      <Dialog.Header>
        <Dialog.Title>Create an organization</Dialog.Title>
        <Dialog.Description>
          Organizations provide shared repository namespaces for teams.
        </Dialog.Description>
      </Dialog.Header>
      <form
        class="grid gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void createOrganization();
        }}
      >
        {#if error}
          <p
            class="rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive"
          >
            {error}
          </p>
        {/if}
        <Field.Field>
          <Field.Label for="organization-slug">Short name</Field.Label>
          <Input
            id="organization-slug"
            bind:value={organizationSlug}
            maxlength={39}
            placeholder="team-name"
            required
          />
          <Field.Description>
            Used in repository URLs, for example /team-name/project.
          </Field.Description>
        </Field.Field>
        <Field.Field>
          <Field.Label for="organization-display-name">Display name</Field.Label
          >
          <Input
            id="organization-display-name"
            bind:value={organizationDisplayName}
            maxlength={128}
            placeholder="Team Name"
            required
          />
        </Field.Field>
        <Dialog.Footer class="sm:justify-between">
          <Button type="button" variant="ghost" onclick={backToChoices}>
            <ArrowLeft class="size-4" />Back
          </Button>
          <div class="flex justify-end gap-2">
            <Dialog.Close>
              {#snippet child({ props })}
                <Button {...props} type="button" variant="outline"
                  >Cancel</Button
                >
              {/snippet}
            </Dialog.Close>
            <Button type="submit" disabled={creating}>
              {creating ? "Creating…" : "Create organization"}
            </Button>
          </div>
        </Dialog.Footer>
      </form>
    {/if}
  </Dialog.Content>
</Dialog.Root>
