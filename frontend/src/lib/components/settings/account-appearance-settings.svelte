<!--
  How Gitadel looks and moves for this account: the colour theme, and whether
  the interface animates. Motion follows the browser's reduced-motion setting
  unless the account turns it off outright.
-->
<script lang="ts">
  import Palette from "@lucide/svelte/icons/palette";
  import { toast } from "svelte-sonner";

  import type { ThemePreference } from "$lib/api/auth.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import { motion } from "$lib/motion.svelte.js";
  import { errorMessage } from "$lib/repository/state/shared.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const THEMES: { value: ThemePreference; label: string }[] = [
    { value: "system", label: "System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];

  const app = useAppState();
  const theme = $derived(app.authStatus?.user?.theme_preference ?? "system");
  const reduceMotion = $derived(
    app.authStatus?.user?.motion_preference === "reduce",
  );
  let saving = $state(false);

  async function save(update: () => Promise<void>): Promise<void> {
    saving = true;
    try {
      await update();
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      saving = false;
    }
  }
</script>

<section
  class="grid gap-5 py-(--card-spacing) md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10"
  aria-labelledby="appearance-heading"
>
  <Card.Header class="flex flex-row items-start gap-3">
    <Palette class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
    <div>
      <Card.Title id="appearance-heading" role="heading" aria-level={2}>
        Appearance
      </Card.Title>
      <Card.Description class="mt-1 max-w-xs leading-5">
        Choose a theme and whether the interface animates.
      </Card.Description>
    </div>
  </Card.Header>

  <Card.Content class="grid max-w-2xl gap-5">
    <Field.Field>
      <Field.Label for="account-theme">Theme</Field.Label>
      <Select.Root
        type="single"
        value={theme}
        onValueChange={(value) =>
          void save(() => app.updateThemePreference(value as ThemePreference))}
      >
        <Select.Trigger id="account-theme" class="w-full" disabled={saving}>
          {THEMES.find((option) => option.value === theme)?.label}
        </Select.Trigger>
        <Select.Content>
          {#each THEMES as option (option.value)}
            <Select.Item value={option.value}>{option.label}</Select.Item>
          {/each}
        </Select.Content>
      </Select.Root>
      <Field.Description>
        System follows your operating system's light or dark setting.
      </Field.Description>
    </Field.Field>

    <label
      class="flex items-center justify-between gap-4 rounded-lg border px-4 py-3"
      for="account-reduce-motion"
    >
      <span class="grid gap-0.5">
        <span class="text-sm font-medium">Reduce motion</span>
        <span class="text-xs leading-5 text-muted-foreground">
          {#if reduceMotion}
            Animations and transitions are off.
          {:else if motion.systemReduced}
            Off here, but your browser asks for reduced motion, so animations
            are off anyway.
          {:else}
            Turn off page transitions and animations. When this is off, Gitadel
            still follows your browser's reduced-motion setting.
          {/if}
        </span>
      </span>
      <Switch
        id="account-reduce-motion"
        checked={reduceMotion}
        disabled={saving}
        onCheckedChange={(checked) =>
          void save(() =>
            app.updateMotionPreference(checked ? "reduce" : "system"),
          )}
      />
    </label>
  </Card.Content>
</section>
