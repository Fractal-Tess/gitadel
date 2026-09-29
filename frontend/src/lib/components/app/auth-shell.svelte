<!--
  Frame for sign-in and account creation: an always-dark page split in two.
  An animated field fills a rounded panel on the left, carrying the instance
  mark and tagline, and the page's form sits beside it. Below the `lg`
  breakpoint the panel shrinks to a banner above the form.
-->
<script lang="ts" module>
  export type AuthBackground = "ravine" | "squares";

  /** The field drawn in the panel beside the sign-in and registration forms. */
  export const AUTH_BACKGROUND: AuthBackground = "ravine";

  /** Shared sizing for inputs and full-width actions in the form. */
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

<div class="dark">
  <main class="min-h-svh bg-background p-3 text-foreground sm:p-4">
    <div
      class="mx-auto grid min-h-[calc(100svh-1.5rem)] max-w-[1400px] gap-3 overflow-hidden rounded-[28px] border border-white/10 p-3 sm:min-h-[calc(100svh-2rem)] lg:grid-cols-[minmax(0,0.95fr)_minmax(0,1.05fr)]"
    >
      <section
        class="relative isolate flex min-h-44 flex-col justify-between overflow-hidden rounded-[18px] bg-black p-5 ring-1 ring-white/10 text-white sm:p-7 lg:min-h-0 lg:p-9"
      >
        <div class="motion-fade absolute inset-0 -z-10" aria-hidden="true">
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
            <Ravine class="size-full" timeOffset={10} />
          {/if}
        </div>

        <a
          class="flex w-fit items-center gap-3 rounded-lg outline-none focus-visible:ring-3 focus-visible:ring-white/50"
          href={resolve("/")}
        >
          <span
            class="grid size-9 place-items-center rounded-xl bg-white/[0.07] ring-1 ring-white/15 backdrop-blur-sm"
          >
            <BrandMark theme="dark" class="size-[18px]" />
          </span>
          <span class="text-base font-semibold tracking-[-0.02em]"
            >{siteName}</span
          >
        </a>

        <p
          class="motion-rise hidden text-[2.25rem] leading-[1.05] font-semibold tracking-[-0.035em] lg:block"
        >
          Your code,<br /><span class="text-white/50">kept close.</span>
        </p>
      </section>

      <div class="flex items-center justify-center px-2 py-8 sm:px-8 lg:py-10">
        <div class="motion-rise w-full max-w-[380px]" style:--stagger={3}>
          {@render children()}
        </div>
      </div>
    </div>
  </main>
</div>
