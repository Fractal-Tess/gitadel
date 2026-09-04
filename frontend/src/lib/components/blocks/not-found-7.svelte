<script lang="ts">
  import { resolve } from "$app/paths";
  import { onDestroy } from "svelte";
  import ArrowRight from "@lucide/svelte/icons/arrow-right";
  import Home from "@lucide/svelte/icons/home";
  import Search from "@lucide/svelte/icons/search";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as InputGroup from "$lib/components/ui/input-group/index.js";
  import { useShellState } from "$lib/state/shell-state.svelte.js";

  let {
    title = "Nothing on the scope.",
    description = "We swept every route twice and could not find what you requested. Search the repositories, or head back to base.",
  }: {
    title?: string;
    description?: string;
  } = $props();

  const shell = useShellState();
  shell.setRailHidden(true);
  onDestroy(() => shell.setRailHidden(false));

  const tickRotations = Array.from({ length: 12 }, (_, index) => index * 30);
  const blips = [
    { left: "81%", top: "40%", delay: "1.2s" },
    { left: "61%", top: "69%", delay: "2.5s" },
    { left: "15%", top: "56%", delay: "4.33s" },
  ];
</script>

<section
  class="grid min-h-[calc(100svh-4rem)] w-full place-items-center px-5 py-12 sm:px-8 sm:py-16"
>
  <div
    class="grid w-full max-w-6xl items-center gap-12 lg:grid-cols-2 lg:gap-16"
  >
    <div
      class="radar-panel relative order-2 flex flex-col overflow-hidden rounded-xl border bg-muted/30 p-6 sm:p-8 lg:order-1 lg:p-10"
    >
      <div
        class="flex items-center justify-between font-mono text-[0.625rem] font-medium uppercase tracking-[0.18em] text-muted-foreground"
      >
        <span>Sweep 06.0s</span>
        <span>Sector NE-04</span>
      </div>

      <div
        class="relative mx-auto my-8 aspect-square w-full max-w-[26rem] sm:my-10"
        aria-hidden="true"
      >
        <div class="absolute inset-0 rounded-full border border-border"></div>
        <div
          class="absolute inset-[16%] rounded-full border border-border"
        ></div>
        <div
          class="absolute inset-[32%] rounded-full border border-border"
        ></div>
        <div
          class="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-border"
        ></div>
        <div
          class="absolute inset-x-0 top-1/2 h-px -translate-y-1/2 bg-border"
        ></div>

        {#each tickRotations as rotation (rotation)}
          <div
            class="absolute inset-0"
            style:transform={`rotate(${rotation}deg)`}
          >
            <span
              class="absolute left-1/2 top-0 h-2 w-px -translate-x-1/2 bg-muted-foreground/50"
            ></span>
          </div>
        {/each}

        <div class="radar-sweep absolute inset-0 overflow-hidden rounded-full">
          <div class="sweep-fill absolute inset-0"></div>
          <div
            class="absolute left-1/2 top-0 h-1/2 w-px -translate-x-1/2 bg-foreground/50"
          ></div>
        </div>

        {#each blips as blip (`${blip.left}-${blip.top}`)}
          <div
            class="absolute -translate-x-1/2 -translate-y-1/2"
            style:left={blip.left}
            style:top={blip.top}
          >
            <span class="block size-1.5 rounded-full bg-muted-foreground"
            ></span>
            <span
              class="contact-dot absolute inset-0 rounded-full bg-foreground"
              style:animation-delay={blip.delay}
            ></span>
            <span
              class="contact-ring absolute -inset-1.5 rounded-full border border-foreground/50"
              style:animation-delay={blip.delay}
            ></span>
          </div>
        {/each}

        <span
          class="absolute left-1/2 top-1/2 size-2 -translate-x-1/2 -translate-y-1/2 rounded-full bg-foreground"
        ></span>
      </div>

      <div
        class="flex items-center justify-between font-mono text-[0.625rem] font-medium uppercase tracking-[0.18em] text-muted-foreground"
      >
        <span>Contacts: 00</span>
        <span>Signal: lost</span>
      </div>
    </div>

    <div class="order-1 max-w-xl lg:order-2">
      <h1 class="reveal-item" style:--delay="80ms">
        <span
          class="block select-none text-[5.5rem] font-black leading-none tracking-[-0.04em] text-foreground sm:text-[7.5rem] lg:text-[9rem]"
        >
          4<span class="outline-numeral">0</span>4
        </span>
        <span
          class="mt-5 block text-balance text-2xl font-semibold tracking-tight text-foreground sm:text-3xl md:text-4xl"
        >
          {title}
        </span>
      </h1>

      <p
        class="reveal-item mt-4 max-w-lg text-pretty text-base leading-relaxed text-muted-foreground sm:text-lg"
        style:--delay="170ms"
      >
        {description}
      </p>

      <form
        action={resolve("/")}
        method="GET"
        class="reveal-item mt-8 max-w-md"
        style:--delay="260ms"
      >
        <InputGroup.Root class="h-12 rounded-full bg-background px-1.5">
          <InputGroup.Addon class="pl-3">
            <Search />
          </InputGroup.Addon>
          <InputGroup.Input
            type="search"
            name="q"
            placeholder="Search repositories…"
            aria-label="Search repositories"
          />
          <InputGroup.Addon align="inline-end">
            <InputGroup.Button type="submit" size="icon-sm" aria-label="Search">
              <ArrowRight />
            </InputGroup.Button>
          </InputGroup.Addon>
        </InputGroup.Root>
      </form>

      <div class="reveal-item mt-6" style:--delay="350ms">
        <Button href={resolve("/")} size="lg">
          <Home data-icon="inline-start" />
          Back to repositories
        </Button>
      </div>
    </div>
  </div>
</section>

<style>
  .radar-panel {
    animation: panel-in 800ms cubic-bezier(0.22, 1, 0.36, 1) 80ms both;
  }

  .radar-panel::before,
  .radar-panel::after {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    content: "";
    pointer-events: none;
  }

  .radar-panel::before {
    background-image: radial-gradient(
      circle,
      color-mix(in oklab, var(--foreground) 7%, transparent) 1px,
      transparent 1px
    );
    background-size: 22px 22px;
  }

  /* Keep the complete radar painted from the first frame. This solid curtain
     starts above it and recedes slowly, avoiding a late texture/shader paint. */
  .radar-panel::after {
    z-index: 20;
    background: color-mix(in oklab, var(--muted) 30%, var(--background));
    animation: curtain-reveal 1800ms cubic-bezier(0.22, 1, 0.36, 1) 180ms both;
  }

  .radar-sweep {
    animation: sweep 6s linear infinite;
  }

  .sweep-fill {
    background: conic-gradient(
      from 0deg,
      transparent 0deg,
      transparent 282deg,
      color-mix(in oklab, var(--foreground) 16%, transparent) 360deg
    );
  }

  .contact-dot {
    animation: contact-dot 6s ease-out infinite;
    opacity: 0;
  }

  .contact-ring {
    animation: contact-ring 6s ease-out infinite;
    opacity: 0;
  }

  .outline-numeral {
    color: transparent;
    -webkit-text-stroke: 2.5px var(--foreground);
  }

  .reveal-item {
    animation: reveal 700ms cubic-bezier(0.22, 1, 0.36, 1) var(--delay) both;
  }

  @keyframes sweep {
    to {
      transform: rotate(360deg);
    }
  }

  @keyframes contact-dot {
    0%,
    26.6%,
    100% {
      opacity: 0;
    }
    5%,
    12% {
      opacity: 1;
    }
  }

  @keyframes contact-ring {
    0%,
    26.6%,
    100% {
      opacity: 0;
      transform: scale(0.5);
    }
    5% {
      opacity: 0.6;
    }
    18% {
      opacity: 0;
      transform: scale(2.8);
    }
  }

  @keyframes panel-in {
    from {
      opacity: 0;
      transform: translateY(24px);
    }
  }

  @keyframes curtain-reveal {
    from {
      opacity: 1;
    }

    to {
      opacity: 0;
    }
  }

  @keyframes reveal {
    from {
      opacity: 0;
      transform: translateY(18px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .radar-panel,
    .reveal-item {
      animation: none;
    }

    .radar-panel::after {
      animation: none;
      opacity: 0;
    }

    .radar-sweep {
      animation: none;
      transform: rotate(24deg);
    }

    .contact-dot,
    .contact-ring {
      animation: none;
      opacity: 0;
    }
  }
</style>
