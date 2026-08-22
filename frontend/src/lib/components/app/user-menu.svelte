<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { LogOut, ScrollText, Settings2, ShieldCheck } from "lucide-svelte";

  import * as Avatar from "$lib/components/ui/avatar/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import { avatarUrl, requestEmpty } from "$lib/api.js";
  import {
    clearAccountSettings,
    preloadAccountSettings,
  } from "$lib/navigation-cache.js";
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
      if (username) clearAccountSettings(username);
      await app.refreshAuth();
      await goto(resolve("/login"));
    } finally {
      working = false;
    }
  }
</script>

{#if app.authStatus?.authenticated}
  <DropdownMenu.Root>
    <DropdownMenu.Trigger
      onpointerenter={() => preloadAccountSettings(username)}
      onfocus={() => preloadAccountSettings(username)}
    >
      {#snippet child({ props })}
        <Button
          {...props}
          variant="ghost"
          size="icon"
          class="shrink-0"
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
    <DropdownMenu.Content align="end" class="w-56">
      <DropdownMenu.Label class="truncate font-normal">
        <span class="text-muted-foreground">Signed in as</span>
        <span class="mt-0.5 block font-medium">{username}</span>
      </DropdownMenu.Label>
      <DropdownMenu.Separator />
      <DropdownMenu.Item onclick={() => void goto(resolve("/settings"))}>
        <Settings2 />Account settings
      </DropdownMenu.Item>
      {#if app.authStatus.user?.is_admin}
        <DropdownMenu.Item onclick={() => void goto(resolve("/admin"))}>
          <ShieldCheck />Administration
        </DropdownMenu.Item>
      {/if}
      <DropdownMenu.Item onclick={() => void goto(resolve("/changelog"))}>
        <ScrollText />Changelog
      </DropdownMenu.Item>
      <DropdownMenu.Separator />
      <DropdownMenu.Item disabled={working} onclick={() => void logout()}>
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
