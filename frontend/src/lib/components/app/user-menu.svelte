<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import LogOut from "@lucide/svelte/icons/log-out";
  import Rocket from "@lucide/svelte/icons/rocket";
  import Server from "@lucide/svelte/icons/server";
  import Settings2 from "@lucide/svelte/icons/settings-2";

  import ThemeSwitcher from "$lib/components/app/theme-switcher.svelte";
  import * as Avatar from "$lib/components/ui/avatar/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import { avatarUrl } from "$lib/api/account.js";
  import { requestEmpty } from "$lib/api/transport.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const username = $derived(app.authStatus?.user?.username ?? "");
  const imageUrl = $derived(
    app.authStatus?.user
      ? avatarUrl(app.authStatus.user.id, app.authStatus.user.avatar_updated_at)
      : null,
  );
  const returnTo = $derived(
    encodeURIComponent(`${page.url.pathname}${page.url.search}`),
  );

  let working = $state(false);

  async function logout(): Promise<void> {
    working = true;
    try {
      await requestEmpty("/api/v1/auth/logout", { method: "POST" });
      app.advanceAuthorizationScope();
      await app.refreshAuth();
      await goto(resolve("/login"));
    } finally {
      working = false;
    }
  }
</script>

{#if app.authStatus?.authenticated}
  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({ props })}
        <Button
          {...props}
          variant="ghost"
          size="icon"
          class="shrink-0 cursor-pointer"
          aria-label="Account menu"
        >
          <Avatar.Root class="size-6">
            {#if imageUrl}
              <Avatar.Image src={imageUrl} alt="" />
            {/if}
            <Avatar.Fallback class="text-[11px] uppercase">
              {username.slice(0, 2)}
            </Avatar.Fallback>
          </Avatar.Root>
        </Button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="end" class="w-64 p-1.5">
      <!-- Who is signed in doubles as the way to their public profile. -->
      <DropdownMenu.Item
        class="gap-3 px-2 py-2"
        onclick={() =>
          void goto(resolve("/[namespace]", { namespace: username }))}
      >
        <Avatar.Root class="size-9">
          {#if imageUrl}
            <Avatar.Image src={imageUrl} alt="" />
          {/if}
          <Avatar.Fallback class="text-xs uppercase">
            {username.slice(0, 2)}
          </Avatar.Fallback>
        </Avatar.Root>
        <span class="grid min-w-0 leading-tight">
          <span class="truncate font-medium">{username}</span>
          <span class="truncate text-xs text-muted-foreground">
            {app.authStatus.user?.is_admin ? "Administrator · " : ""}View profile
          </span>
        </span>
      </DropdownMenu.Item>
      <DropdownMenu.Separator class="my-1.5" />
      <DropdownMenu.Item
        class="py-1.5"
        onclick={() =>
          void goto(resolve("/-/account/[view]", { view: "profile" }))}
      >
        <Settings2 />Account settings
      </DropdownMenu.Item>
      <ThemeSwitcher />
      <DropdownMenu.Separator class="my-1.5" />
      <!-- The personal namespace's management pages live here rather than in
           a tab bar over the profile. -->
      <DropdownMenu.Group>
        <DropdownMenu.GroupHeading
          class="px-2 pt-1 pb-1.5 text-[11px] font-medium tracking-wide text-muted-foreground uppercase"
        >
          Your namespace
        </DropdownMenu.GroupHeading>
        <DropdownMenu.Item
          class="py-1.5"
          onclick={() =>
            void goto(resolve("/[namespace]/runners", { namespace: username }))}
        >
          <Server />Runners
        </DropdownMenu.Item>
        <DropdownMenu.Item
          class="py-1.5"
          onclick={() =>
            void goto(
              resolve("/[namespace]/integrations", { namespace: username }),
            )}
        >
          <Rocket />Integrations
        </DropdownMenu.Item>
        <DropdownMenu.Item
          class="py-1.5"
          onclick={() =>
            void goto(
              resolve("/[namespace]/mirror-credentials", {
                namespace: username,
              }),
            )}
        >
          <KeyRound />Mirror identities
        </DropdownMenu.Item>
      </DropdownMenu.Group>
      <DropdownMenu.Separator class="my-1.5" />
      <DropdownMenu.Item
        class="py-1.5"
        variant="destructive"
        disabled={working}
        onclick={() => void logout()}
      >
        <LogOut />{working ? "Signing out…" : "Sign out"}
      </DropdownMenu.Item>
    </DropdownMenu.Content>
  </DropdownMenu.Root>
{:else}
  <Button
    variant="outline"
    size="sm"
    class="shrink-0"
    href={`${resolve("/login")}?returnTo=${returnTo}`}
  >
    Sign in
  </Button>
{/if}
