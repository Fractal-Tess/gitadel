<script lang="ts">
  import { Check, ChevronsUpDown, UserRound, X } from "lucide-svelte";

  import { avatarUrl, type IssueUser } from "$lib/api.js";
  import * as Avatar from "$lib/components/ui/avatar/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Command from "$lib/components/ui/command/index.js";
  import * as Popover from "$lib/components/ui/popover/index.js";

  let {
    users,
    value = $bindable(""),
    disabled = false,
  }: {
    users: IssueUser[];
    value?: string;
    disabled?: boolean;
  } = $props();

  let open = $state(false);

  const selected = $derived(users.find((user) => user.username === value));

  function pick(username: string) {
    value = username;
    open = false;
  }
</script>

<Popover.Root bind:open>
  <Popover.Trigger
    {disabled}
    aria-label={selected ? `Assignee: ${selected.username}` : "Select assignee"}
  >
    {#snippet child({ props })}
      <Button
        type="button"
        variant="outline"
        role="combobox"
        aria-expanded={open}
        class="w-full justify-between font-normal"
        {disabled}
        {...props}
      >
        {#if selected}
          <span class="flex min-w-0 items-center gap-2">
            <Avatar.Root class="size-5">
              {#if avatarUrl(selected.id, selected.avatar_updated_at)}
                <Avatar.Image
                  src={avatarUrl(selected.id, selected.avatar_updated_at) ??
                    undefined}
                  alt=""
                />
              {/if}
              <Avatar.Fallback class="text-[8px] font-medium uppercase">
                {selected.username.slice(0, 2)}
              </Avatar.Fallback>
            </Avatar.Root>
            <span class="truncate">{selected.username}</span>
          </span>
        {:else}
          <span class="flex items-center gap-2 text-muted-foreground">
            <UserRound class="size-3.5" />
            Select an assignee…
          </span>
        {/if}
        <ChevronsUpDown class="size-3.5 shrink-0 text-muted-foreground" />
      </Button>
    {/snippet}
  </Popover.Trigger>
  <Popover.Content class="w-64 p-0" align="start">
    <Command.Root>
      <Command.Input placeholder="Search users…" class="h-9" />
      <Command.List class="max-h-60 scroll-py-1">
        <Command.Empty>No matching user.</Command.Empty>
        <Command.Item
          value="_unassign"
          class="gap-2"
          keywords={["none", "remove", "unassigned"]}
          onSelect={() => pick("")}
        >
          <X class="size-3.5" />
          No assignee
          {#if !value}
            <Check class="ml-auto size-3.5" />
          {/if}
        </Command.Item>
        {#each users as user (user.id)}
          <Command.Item
            value={user.username}
            class="gap-2"
            onSelect={() => pick(user.username)}
          >
            <Avatar.Root class="size-5">
              {#if avatarUrl(user.id, user.avatar_updated_at)}
                <Avatar.Image
                  src={avatarUrl(user.id, user.avatar_updated_at) ?? undefined}
                  alt=""
                />
              {/if}
              <Avatar.Fallback class="text-[8px] font-medium uppercase">
                {user.username.slice(0, 2)}
              </Avatar.Fallback>
            </Avatar.Root>
            {user.username}
            {#if value === user.username}
              <Check class="ml-auto size-3.5" />
            {/if}
          </Command.Item>
        {/each}
      </Command.List>
    </Command.Root>
  </Popover.Content>
</Popover.Root>
