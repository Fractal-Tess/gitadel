<script lang="ts">
  import { Building2, Users } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import type { AccountSettingsState } from "$lib/settings/account-settings-state.svelte.js";

  let { state }: { state: AccountSettingsState } = $props();
  const inputClass =
    "w-full rounded-md border bg-background px-3 py-2 text-sm outline-none focus:border-ring focus:ring-2 focus:ring-ring/20";
</script>

<div class="grid gap-6 lg:grid-cols-[20rem_minmax(0,1fr)]">
  <section class="rounded-md border bg-card/25">
    <header class="flex items-center gap-3 border-b px-5 py-4">
      <Building2 class="size-4 text-muted-foreground" />
      <h2 class="text-sm font-semibold">Organizations</h2>
    </header>
    <div class="p-5">
      <ul class="grid gap-2">
        {#each state.organizations as organization (organization.id)}
          <li>
            <button
              class={state.selectedOrganization?.id === organization.id
                ? "w-full rounded-md border bg-muted p-3 text-left text-sm"
                : "w-full rounded-md border p-3 text-left text-sm hover:bg-muted"}
              onclick={() => void state.selectOrganization(organization)}
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
          void state.createOrganization();
        }}
      >
        <label class="grid gap-1.5 text-sm font-medium">
          Short name
          <input class={inputClass} bind:value={state.organizationSlug} required />
        </label>
        <label class="grid gap-1.5 text-sm font-medium">
          Display name
          <input
            class={inputClass}
            bind:value={state.organizationDisplayName}
            required
          />
        </label>
        <Button type="submit" disabled={state.working}>Create organization</Button>
      </form>
    </div>
  </section>

  <section class="rounded-md border bg-card/25">
    <header class="flex items-center gap-3 border-b px-5 py-4">
      <Users class="size-4 text-muted-foreground" />
      <h2 class="text-sm font-semibold">
        {state.selectedOrganization?.display_name ?? "Members"}
      </h2>
    </header>
    <div class="p-5">
      {#if state.selectedOrganization}
        <ul class="grid gap-2">
          {#each state.members as member (member.username)}
            <li class="flex items-center justify-between rounded-md border p-3">
              <div>
                <p class="text-sm font-medium">{member.username}</p>
                <p class="text-xs text-muted-foreground">{member.role}</p>
              </div>
              {#if state.selectedOrganization.role === "owner"}
                <Button
                  size="sm"
                  variant="ghost"
                  onclick={() => void state.removeMember(member.username)}>Remove</Button
                >
              {/if}
            </li>
          {/each}
        </ul>
        {#if state.selectedOrganization.role === "owner"}
          <form
            class="mt-5 grid gap-3 border-t pt-5 sm:grid-cols-[minmax(0,1fr)_9rem_auto] sm:items-end"
            onsubmit={(event) => {
              event.preventDefault();
              void state.addMember();
            }}
          >
            <label class="grid gap-1.5 text-sm font-medium">
              Username
              <input class={inputClass} bind:value={state.memberUsername} required />
            </label>
            <label class="grid gap-1.5 text-sm font-medium">
              Role
              <Select.Root
                type="single"
                value={state.memberRole}
                onValueChange={(value) => {
                  if (value === "owner" || value === "member") {
                    state.memberRole = value;
                  }
                }}
              >
                <Select.Trigger class="w-full">{state.memberRole}</Select.Trigger>
                <Select.Content>
                  <Select.Item value="member">Member</Select.Item>
                  <Select.Item value="owner">Owner</Select.Item>
                </Select.Content>
              </Select.Root>
            </label>
            <Button type="submit" disabled={state.working}>Add member</Button>
          </form>
        {/if}
      {:else}
        <p class="text-sm text-muted-foreground">
          Select an organization to manage its members.
        </p>
      {/if}
    </div>
  </section>
</div>
