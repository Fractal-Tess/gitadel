<script lang="ts">
  import { Building2, Users } from "lucide-svelte";

  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import type { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";

  let { state: account }: { state: AccountSettingsState } = $props();
  let removeDialogOpen = $state(false);
  let pendingMember = $state<string | null>(null);
  const inputClass =
    "w-full rounded-md border bg-background px-3 py-2 text-sm outline-none focus:border-ring focus:ring-2 focus:ring-ring/20";

  function requestRemoveMember(username: string) {
    pendingMember = username;
    removeDialogOpen = true;
  }

  async function removeMember() {
    if (!pendingMember) return;
    await account.removeMember(pendingMember);
    if (!account.error) {
      removeDialogOpen = false;
      pendingMember = null;
    }
  }
</script>

<div class="grid gap-6 lg:grid-cols-[20rem_minmax(0,1fr)]">
  <section class="rounded-md border bg-card/25">
    <header class="flex items-center gap-3 border-b px-5 py-4">
      <Building2 class="size-4 text-muted-foreground" />
      <h2 class="text-sm font-semibold">Organizations</h2>
    </header>
    <div class="p-5">
      <ul class="grid gap-2">
        {#each account.organizations as organization (organization.id)}
          <li>
            <button
              class={account.selectedOrganization?.id === organization.id
                ? "w-full rounded-md border bg-muted p-3 text-left text-sm"
                : "w-full rounded-md border p-3 text-left text-sm hover:bg-muted"}
              onclick={() => void account.selectOrganization(organization)}
            >
              <span class="font-medium">{organization.display_name}</span>
              <span class="block text-xs text-muted-foreground">
                {organization.slug} · {organization.role}
              </span>
            </button>
          </li>
        {:else}
          <li class="text-sm text-muted-foreground">No organizations yet.</li>
        {/each}
      </ul>
      <form
        class="mt-5 grid gap-3 border-t pt-5"
        onsubmit={(event) => {
          event.preventDefault();
          void account.createOrganization();
        }}
      >
        <label class="grid gap-1.5 text-sm font-medium">
          Short name
          <input
            class={inputClass}
            bind:value={account.organizationSlug}
            required
          />
        </label>
        <label class="grid gap-1.5 text-sm font-medium">
          Display name
          <input
            class={inputClass}
            bind:value={account.organizationDisplayName}
            required
          />
        </label>
        <Button type="submit" disabled={account.working}>
          Create organization
        </Button>
      </form>
    </div>
  </section>

  <section class="rounded-md border bg-card/25">
    <header class="flex items-center gap-3 border-b px-5 py-4">
      <Users class="size-4 text-muted-foreground" />
      <h2 class="text-sm font-semibold">
        {account.selectedOrganization?.display_name ?? "Members"}
      </h2>
    </header>
    <div class="p-5">
      {#if account.selectedOrganization}
        <ul class="grid gap-2">
          {#each account.members as member (member.username)}
            <li class="flex items-center justify-between rounded-md border p-3">
              <div>
                <p class="text-sm font-medium">{member.username}</p>
                <p class="text-xs text-muted-foreground">{member.role}</p>
              </div>
              {#if account.selectedOrganization.role === "owner"}
                <Button
                  size="sm"
                  variant="ghost"
                  onclick={() => requestRemoveMember(member.username)}
                >
                  Remove
                </Button>
              {/if}
            </li>
          {/each}
        </ul>
        {#if account.selectedOrganization.role === "owner"}
          <form
            class="mt-5 grid gap-3 border-t pt-5 sm:grid-cols-[minmax(0,1fr)_9rem_auto] sm:items-end"
            onsubmit={(event) => {
              event.preventDefault();
              void account.addMember();
            }}
          >
            <label class="grid gap-1.5 text-sm font-medium">
              Username
              <input
                class={inputClass}
                bind:value={account.memberUsername}
                required
              />
            </label>
            <label class="grid gap-1.5 text-sm font-medium">
              Role
              <Select.Root
                type="single"
                value={account.memberRole}
                onValueChange={(value) => {
                  if (value === "owner" || value === "member") {
                    account.memberRole = value;
                  }
                }}
              >
                <Select.Trigger class="w-full">
                  {account.memberRole}
                </Select.Trigger>
                <Select.Content>
                  <Select.Item value="member">Member</Select.Item>
                  <Select.Item value="owner">Owner</Select.Item>
                </Select.Content>
              </Select.Root>
            </label>
            <Button type="submit" disabled={account.working}>Add member</Button>
          </form>
        {/if}
      {:else}
        <p class="text-sm text-muted-foreground">
          Select an organization to manage its members.
        </p>
      {/if}
    </div>
  </section>

  <AlertDialog.Root bind:open={removeDialogOpen}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title>
          Remove {pendingMember ?? "this member"}?
        </AlertDialog.Title>
        <AlertDialog.Description>
          They will lose access granted through this organization.
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action
          variant="destructive"
          disabled={account.working}
          onclick={() => void removeMember()}
        >
          Remove member
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>
</div>
