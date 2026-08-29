<script lang="ts">
  import Check from "@lucide/svelte/icons/check";
  import ChevronsUpDown from "@lucide/svelte/icons/chevrons-up-down";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import UserRound from "@lucide/svelte/icons/user-round";

  import { avatarUrl } from "$lib/api/account.js";
  import { memberSuggestionSchema, type MemberSuggestion } from "$lib/api/organizations.js";
  import { requestJson } from "$lib/api/transport.js";
  import * as Avatar from "$lib/components/ui/avatar/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Command from "$lib/components/ui/command/index.js";
  import * as Popover from "$lib/components/ui/popover/index.js";

  let {
    slug,
    value = $bindable(""),
    disabled = false,
    autofocus = false,
  }: {
    slug: string;
    value?: string;
    disabled?: boolean;
    autofocus?: boolean;
  } = $props();

  let open = $state(false);
  let query = $state("");
  let suggestions = $state.raw<MemberSuggestion[]>([]);
  let loading = $state(false);
  let searchError = $state<string | null>(null);
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;
  let requestController: AbortController | undefined;

  const selected = $derived(
    suggestions.find((suggestion) => suggestion.username === value),
  );

  function handleOpen(next: boolean): void {
    open = next;
    if (!next) {
      clearTimeout(debounceTimer);
      requestController?.abort();
      return;
    }
    query = value;
    void searchUsers(query);
  }

  function scheduleSearch(): void {
    clearTimeout(debounceTimer);
    requestController?.abort();
    const search = query.trim().toLowerCase();
    if (
      search.length > 39 ||
      !Array.from(search).every(
        (character) =>
          (character >= "a" && character <= "z") ||
          (character >= "0" && character <= "9") ||
          character === "-",
      )
    ) {
      suggestions = [];
      loading = false;
      searchError = "Usernames use letters, numbers, and hyphens.";
      return;
    }
    loading = true;
    searchError = null;
    debounceTimer = setTimeout(() => void searchUsers(search), 250);
  }

  async function searchUsers(search: string): Promise<void> {
    requestController?.abort();
    const controller = new AbortController();
    requestController = controller;
    loading = true;
    searchError = null;
    try {
      suggestions = await requestJson(
        `/api/v1/organizations/${encodeURIComponent(slug)}/member-suggestions?q=${encodeURIComponent(search)}`,
        memberSuggestionSchema.array(),
        { signal: controller.signal },
      );
    } catch (caught) {
      if (caught instanceof DOMException && caught.name === "AbortError")
        return;
      suggestions = [];
      searchError =
        caught instanceof Error ? caught.message : "Could not search users.";
    } finally {
      if (requestController === controller) {
        requestController = undefined;
        loading = false;
      }
    }
  }

  function pick(suggestion: MemberSuggestion): void {
    value = suggestion.username;
    query = suggestion.username;
    open = false;
  }
</script>

<Popover.Root {open} onOpenChange={handleOpen}>
  <Popover.Trigger aria-label="Choose a Gitadel user" {disabled}>
    {#snippet child({ props })}
      <Button
        type="button"
        variant="outline"
        role="combobox"
        aria-expanded={open}
        class="w-full justify-between font-normal"
        {disabled}
        {autofocus}
        {...props}
      >
        {#if value}
          <span class="flex min-w-0 items-center gap-2">
            <Avatar.Root class="size-5">
              {#if selected && avatarUrl(selected.id, selected.avatar_updated_at)}
                <Avatar.Image
                  src={avatarUrl(selected.id, selected.avatar_updated_at) ??
                    undefined}
                  alt=""
                />
              {/if}
              <Avatar.Fallback class="text-[8px] font-medium uppercase">
                {value.slice(0, 2)}
              </Avatar.Fallback>
            </Avatar.Root>
            <span class="truncate">{value}</span>
          </span>
        {:else}
          <span class="flex items-center gap-2 text-muted-foreground">
            <UserRound class="size-3.5" />
            Search for a user…
          </span>
        {/if}
        <ChevronsUpDown class="size-3.5 shrink-0 text-muted-foreground" />
      </Button>
    {/snippet}
  </Popover.Trigger>
  <Popover.Content class="w-80 p-0" align="start">
    <Command.Root shouldFilter={false}>
      <Command.Input
        bind:value={query}
        placeholder="Search usernames…"
        class="h-9"
        oninput={scheduleSearch}
      />
      <Command.List class="max-h-60 scroll-py-1">
        {#if loading}
          <Command.Loading>
            <div
              class="flex items-center gap-2 px-3 py-4 text-sm text-muted-foreground"
            >
              <LoaderCircle class="size-4 animate-spin" />
              Searching users…
            </div>
          </Command.Loading>
        {:else if searchError}
          <p class="px-3 py-4 text-sm text-destructive" role="alert">
            {searchError}
          </p>
        {:else if suggestions.length === 0}
          <Command.Empty>
            {query.trim() ? "No matching users." : "No users available."}
          </Command.Empty>
        {:else}
          {#each suggestions as suggestion (suggestion.id)}
            <Command.Item
              value={suggestion.username}
              class="gap-2"
              onSelect={() => pick(suggestion)}
            >
              <Avatar.Root class="size-6">
                {#if avatarUrl(suggestion.id, suggestion.avatar_updated_at)}
                  <Avatar.Image
                    src={avatarUrl(
                      suggestion.id,
                      suggestion.avatar_updated_at,
                    ) ?? undefined}
                    alt=""
                  />
                {/if}
                <Avatar.Fallback class="text-[9px] font-medium uppercase">
                  {suggestion.username.slice(0, 2)}
                </Avatar.Fallback>
              </Avatar.Root>
              <span class="truncate">{suggestion.username}</span>
              {#if value === suggestion.username}
                <Check class="ml-auto size-3.5" />
              {/if}
            </Command.Item>
          {/each}
        {/if}
      </Command.List>
    </Command.Root>
  </Popover.Content>
</Popover.Root>
