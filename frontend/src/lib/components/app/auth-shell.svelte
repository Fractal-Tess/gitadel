<!--
  Frame for sign-in and account creation: an always-dark animated field fills
  the page, with the instance mark above and the page's form in a centred card.
-->
<script lang="ts" module>
  export type AuthBackground = "ravine" | "squares";

  /** The field drawn behind the sign-in and registration cards. */
  export const AUTH_BACKGROUND: AuthBackground = "ravine";

  /** Shared sizing for inputs and full-width actions inside the card. */
  export const authInputClass =
    "h-11 rounded-xl bg-muted/60 px-3.5 md:text-sm dark:bg-input/20";
  export const authButtonClass = "relative h-11 w-full rounded-xl text-sm";
</script>

<script lang="ts">
  import type { Snippet } from "svelte";
  import { resolve } from "$app/paths";

  import BlinkingSquares from "$lib/components/app/blinking-squares.svelte";
  import Ravine from "$lib/components/app/ravine.svelte";
  import BrandMark from "$lib/components/brand-mark.svelte";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  let {
    background = AUTH_BACKGROUND,
    children,
  }: { background?: AuthBackground; children: Snippet } = $props();

  const app = useAppState();
  const siteName = $derived(app.instance?.site_name || "Gitadel");
</script>

<main class="relative isolate min-h-svh bg-[#050505] text-white">
  <div class="fixed inset-0 -z-10" aria-hidden="true">
    {#if background === "squares"}
      <BlinkingSquares
        class="size-full"
        direction="right"
        gridSize={48}
        squareColor="#f97316"
        backgroundColor="#0a0a0a"
        fadeStart={0.5}
        falloff={1.6}
        squareSize={0.56}
        minBrightness={0.35}
        twinkleSpeed={0.9}
        twinkleStrength={0.85}
      />
    {:else}
      <Ravine class="size-full" />
    {/if}
  </div>

  <div class="flex min-h-svh flex-col px-4 py-5 sm:px-8 sm:py-7">
    <a
      class="flex w-fit items-center gap-3 rounded-lg outline-none focus-visible:ring-3 focus-visible:ring-white/50"
      href={resolve("/")}
    >
      <span
        class="grid size-9 place-items-center rounded-xl bg-white/[0.07] ring-1 ring-white/15 backdrop-blur-sm"
      >
        <BrandMark theme="dark" class="size-[18px]" />
      </span>
      <span class="text-base font-semibold tracking-[-0.02em]">{siteName}</span>
    </a>

    <div class="flex flex-1 items-center justify-center py-8">
      <div
        class="w-full max-w-[420px] rounded-[28px] border border-white/10 bg-card p-6 text-card-foreground shadow-[0_32px_80px_-24px_rgb(0_0_0/0.8)] backdrop-blur-xl sm:p-9 dark:bg-card/80"
      >
        {@render children()}
      </div>
    </div>

    <!-- Only where it clears the centred card. -->
    <p
      class="pointer-events-none absolute bottom-8 left-8 hidden text-[2rem] leading-[1.05] font-semibold tracking-[-0.035em] xl:block"
    >
      Your code,<br /><span class="text-white/50">kept close.</span>
    </p>
  </div>
</main>
