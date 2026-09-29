<!--
  The theme choice, as a submenu of the account menu. The checked option is
  the saved preference; picking another saves it to the account.
-->
<script lang="ts">
  import MoonIcon from "@lucide/svelte/icons/moon";
  import SunIcon from "@lucide/svelte/icons/sun";
  import { toast } from "svelte-sonner";

  import type { ThemePreference } from "$lib/api/auth.js";
  import { ApiFailure } from "$lib/api/transport.js";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  let working = $state(false);
  const preference = $derived(
    app.authStatus?.user?.theme_preference ?? "system",
  );

  async function selectMode(next: ThemePreference): Promise<void> {
    if (next === preference) return;
    working = true;
    try {
      await app.updateThemePreference(next);
    } catch (error) {
      toast.error(
        error instanceof ApiFailure || error instanceof Error
          ? error.message
          : "Could not save the theme preference.",
      );
    } finally {
      working = false;
    }
  }
</script>

<DropdownMenu.Sub>
  <DropdownMenu.SubTrigger class="py-1.5" disabled={working}>
    <SunIcon class="dark:hidden" />
    <MoonIcon class="hidden dark:block" />
    Theme
    <span class="flex-1 text-right text-xs text-muted-foreground capitalize">
      {preference}
    </span>
  </DropdownMenu.SubTrigger>
  <DropdownMenu.SubContent class="min-w-32">
    <DropdownMenu.RadioGroup
      value={preference}
      onValueChange={(value) => void selectMode(value as ThemePreference)}
    >
      <DropdownMenu.RadioItem value="light">Light</DropdownMenu.RadioItem>
      <DropdownMenu.RadioItem value="dark">Dark</DropdownMenu.RadioItem>
      <DropdownMenu.RadioItem value="system">System</DropdownMenu.RadioItem>
    </DropdownMenu.RadioGroup>
  </DropdownMenu.SubContent>
</DropdownMenu.Sub>
