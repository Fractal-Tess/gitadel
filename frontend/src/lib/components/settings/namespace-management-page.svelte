<script lang="ts">
  import { page } from "$app/state";
  import Plus from "@lucide/svelte/icons/plus";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import UserRound from "@lucide/svelte/icons/user-round";

  import ActionsSettings from "$lib/components/settings/actions-settings.svelte";
  import IntegrationsSettings from "$lib/components/settings/integrations-settings.svelte";
  import MirroringSettings from "$lib/components/settings/mirroring-settings.svelte";
  import MemberCombobox from "$lib/components/settings/member-combobox.svelte";
  import OrganizationProfileSettings from "$lib/components/settings/organization-profile-settings.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  type NamespaceView =
    "members" | "runners" | "integrations" | "mirror-credentials" | "settings";

  const app = useAppState();
  const accountState = new AccountSettingsState(app);
  const organizationState = accountState.organization;
  const actionsState = accountState.actions;
  const slug = $derived(page.params.namespace ?? "");
  const requestedView = $derived(
    page.url.pathname.split("/").at(-1) ?? "runners",
  );
  const view = $derived<NamespaceView>(
    requestedView === "members" ||
      requestedView === "integrations" ||
      requestedView === "mirror-credentials" ||
      requestedView === "settings"
      ? requestedView
      : "runners",
  );
  const personal = $derived(slug === app.authStatus?.user?.username);
  const organization = $derived(
    app.organizations.find((candidate) => candidate.slug === slug) ?? null,
  );
  const canManage = $derived(personal || organization?.role === "owner");
  const canAccess = $derived(
    canManage || (Boolean(organization) && view === "members"),
  );
  const label = $derived(personal ? slug : organization?.display_name || slug);
  const namespace = $derived({ slug, label });
  const scopeKey = $derived(
    `${app.authorizationScope.viewer ?? "anonymous"}:${app.authorizationScope.epoch}`,
  );
  let loadedMembersFor = "";
  const membersPending = $derived(
    view === "members" &&
      Boolean(organization) &&
      (organizationState.membersLoading ||
        loadedMembersFor !== `${scopeKey}:${organization?.slug}`),
  );
  let addMemberDialogOpen = $state(false);
  let removeDialogOpen = $state(false);
  let pendingMember = $state<string | null>(null);

  $effect(() => {
    accountState.syncScope();
    if (
      view !== "members" ||
      !organization ||
      loadedMembersFor === `${scopeKey}:${organization.slug}`
    )
      return;
    const target = `${scopeKey}:${organization.slug}`;
    loadedMembersFor = target;
    organizationState.membersLoading = true;
    organizationState.membersLoadError = null;
    void organizationState
      .selectOrganization(organization)
      .catch(() => {
        organizationState.membersLoadError = "Could not load organization members.";
      })
      .finally(() => {
        if (loadedMembersFor === target) organizationState.membersLoading = false;
      });
  });

  function openAddMember(): void {
    organizationState.memberUsername = "";
    organizationState.memberRole = "member";
    organizationState.error = null;
    addMemberDialogOpen = true;
  }

  async function addMember(): Promise<void> {
    await organizationState.addMember();
    if (!organizationState.error) addMemberDialogOpen = false;
  }

  function requestRemoveMember(username: string): void {
    pendingMember = username;
    removeDialogOpen = true;
  }

  async function removeMember(): Promise<void> {
    if (!pendingMember) return;
    await organizationState.removeMember(pendingMember);
    if (!organizationState.error) {
      removeDialogOpen = false;
      pendingMember = null;
    }
  }
</script>

<svelte:head>
  <title
    >{label} · {view === "members"
      ? "Members"
      : view === "runners"
        ? "Runners"
        : view === "integrations"
          ? "Integrations"
          : view === "mirror-credentials"
            ? "Mirror identities"
            : "Settings"} · {app.instance?.site_name ?? "Gitadel"}</title
  >
</svelte:head>

<div class="mx-auto max-w-5xl px-5 py-8 lg:px-8">
  {#if app.loading}
    <p class="py-16 text-center text-sm text-muted-foreground">
      Loading namespace…
    </p>
  {:else if !canAccess}
    <section class="rounded-lg border border-dashed p-6">
      <h1 class="font-semibold">Owner access required</h1>
      <p class="mt-1 text-sm text-muted-foreground">
        Only the organization owner can manage these resources.
      </p>
    </section>
  {:else}
    {#key `${slug}:${view}`}
      {#if view === "members" && organization}
        <section class="space-y-6" aria-labelledby="namespace-members-heading">
          <header>
            <h2
              id="namespace-members-heading"
              class="text-lg font-semibold tracking-tight"
            >
              Members
            </h2>
            <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
              {canManage ? "Manage" : "View"} access to repositories and shared resources
              in {label}.
            </p>
          </header>

          <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {#if canManage}
              <button
                type="button"
                class="group flex min-h-40 w-full flex-col items-center justify-center rounded-xl border border-dashed bg-card/15 p-5 text-center transition-colors hover:border-foreground/25 hover:bg-card/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                onclick={openAddMember}
              >
                <span
                  class="grid size-10 place-items-center rounded-full border bg-background transition-transform group-hover:scale-105"
                >
                  <Plus class="size-4" />
                </span>
                <span class="mt-3 font-medium">Add member</span>
                <span
                  class="mt-1.5 max-w-52 text-sm leading-5 text-muted-foreground"
                >
                  Grant another account access to {label}.
                </span>
              </button>
            {/if}
            {#if membersPending}
              <article
                class="flex min-h-40 flex-col rounded-xl border bg-card/40 p-4 shadow-sm"
                aria-label="Loading members"
              >
                <div class="flex items-start gap-3">
                  <span class="size-10 animate-pulse rounded-lg border bg-muted"
                  ></span>
                  <div class="grid flex-1 gap-2 pt-1">
                    <span class="h-3 w-28 animate-pulse rounded bg-muted"
                    ></span>
                    <span class="h-2.5 w-16 animate-pulse rounded bg-muted"
                    ></span>
                  </div>
                </div>
                <span class="mt-5 h-2.5 w-full animate-pulse rounded bg-muted"
                ></span>
                <span class="mt-2 h-2.5 w-3/4 animate-pulse rounded bg-muted"
                ></span>
              </article>
            {:else if organizationState.membersLoadError}
              <p
                class="rounded-xl border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive sm:col-span-2"
                role="alert"
              >
                {organizationState.membersLoadError}
              </p>
            {:else}
              {#each organizationState.members as member (member.username)}
                <article
                  class="flex min-h-40 flex-col rounded-xl border bg-card/40 p-4 shadow-sm"
                >
                  <div class="flex items-start gap-3">
                    <span class="rounded-lg border bg-background p-2">
                      <UserRound class="size-5 text-muted-foreground" />
                    </span>
                    <div class="min-w-0">
                      <h3 class="truncate text-sm font-semibold">
                        {member.username}
                      </h3>
                      <p
                        class="mt-0.5 text-xs capitalize text-muted-foreground"
                      >
                        {member.role}
                      </p>
                    </div>
                  </div>
                  <p class="mt-4 text-xs leading-5 text-muted-foreground">
                    {member.role === "owner"
                      ? "Can manage organization settings, members, and shared resources."
                      : "Can access repositories and resources shared with the organization."}
                  </p>
                  {#if canManage}
                    <div class="mt-auto flex justify-end pt-4">
                      <Button
                        type="button"
                        size="icon-sm"
                        variant="ghost"
                        class="text-muted-foreground hover:text-destructive"
                        aria-label={`Remove ${member.username}`}
                        onclick={() => requestRemoveMember(member.username)}
                      >
                        <Trash2 class="size-4" />
                      </Button>
                    </div>
                  {/if}
                </article>
              {:else}
                <div
                  class="rounded-xl border border-dashed p-8 text-center sm:col-span-2 xl:col-span-3"
                >
                  <UserRound class="mx-auto size-8 text-muted-foreground" />
                  <p class="mt-3 font-medium">No members found</p>
                  <p class="mt-1 text-sm text-muted-foreground">
                    Add a member to share organization resources.
                  </p>
                </div>
              {/each}
            {/if}
          </div>
        </section>
      {:else if view === "runners"}
        <ActionsSettings state={actionsState} {namespace} />
      {:else if view === "integrations"}
        <IntegrationsSettings {namespace} />
      {:else if view === "mirror-credentials"}
        <MirroringSettings {namespace} />
      {:else if view === "settings" && organization}
        <OrganizationProfileSettings {organization} />
      {/if}
    {/key}
  {/if}

  <Dialog.Root bind:open={addMemberDialogOpen}>
    <Dialog.Content class="ring-foreground/20 sm:max-w-md">
      <Dialog.Header>
        <Dialog.Title>Add organization member</Dialog.Title>
        <Dialog.Description>
          Grant a Gitadel account access to repositories and shared resources in
          {label}.
        </Dialog.Description>
      </Dialog.Header>
      <form
        class="grid gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void addMember();
        }}
      >
        {#if organizationState.error}
          <p
            class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
            role="alert"
          >
            {organizationState.error}
          </p>
        {/if}
        <div class="grid gap-1.5 text-sm font-medium">
          <span>Username</span>
          <MemberCombobox
            slug={organization?.slug ?? ""}
            bind:value={organizationState.memberUsername}
            disabled={organizationState.working}
            autofocus
          />
        </div>
        <label class="grid gap-1.5 text-sm font-medium">
          Role
          <Select.Root
            type="single"
            value={organizationState.memberRole}
            onValueChange={(value) => {
              if (value === "owner" || value === "member")
                organizationState.memberRole = value;
            }}
          >
            <Select.Trigger class="w-full">
              {organizationState.memberRole === "owner" ? "Owner" : "Member"}
            </Select.Trigger>
            <Select.Content>
              <Select.Item value="member">Member</Select.Item>
              <Select.Item value="owner">Owner</Select.Item>
            </Select.Content>
          </Select.Root>
        </label>
        <Dialog.Footer>
          <Button
            type="button"
            variant="outline"
            onclick={() => (addMemberDialogOpen = false)}>Cancel</Button
          >
          <Button
            type="submit"
            disabled={organizationState.working || !organizationState.memberUsername}
          >
            {organizationState.working ? "Adding…" : "Add member"}
          </Button>
        </Dialog.Footer>
      </form>
    </Dialog.Content>
  </Dialog.Root>

  <AlertDialog.Root bind:open={removeDialogOpen}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title
          >Remove {pendingMember ?? "this member"}?</AlertDialog.Title
        >
        <AlertDialog.Description
          >They will lose access granted through this organization.</AlertDialog.Description
        >
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action
          variant="destructive"
          disabled={organizationState.working}
          onclick={() => void removeMember()}>Remove member</AlertDialog.Action
        >
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>
</div>
