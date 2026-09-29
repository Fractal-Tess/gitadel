<script module lang="ts">
  import type { ShellIcon } from "$lib/state/shell-state.svelte.js";

  export type ContextNavSubItem = {
    id: string;
    label: string;
    icon: ShellIcon;
    href: string;
    active: boolean;
    select?: () => void;
    preload?: () => void;
  };

  export type ContextNavItem = {
    id: string;
    label: string;
    icon: ShellIcon;
    href: string;
    active: boolean;
    select?: () => void;
    preload?: () => void;
    items?: ContextNavSubItem[];
  };
</script>

<script lang="ts">
  import { tick } from "svelte";
  let {
    label,
    items,
    wide = false,
  }: {
    label: string;
    items: ContextNavItem[];
    wide?: boolean;
  } = $props();

  const activeItem = $derived(items.find((item) => item.active));
  let primaryList = $state<HTMLDivElement>();
  let secondaryList = $state<HTMLDivElement>();

  $effect(() => {
    const currentId = activeItem?.id;
    const currentSubId = activeItem?.items?.find((item) => item.active)?.id;
    void tick().then(() => {
      if (currentId) {
        primaryList
          ?.querySelector<HTMLElement>("[aria-current=page]")
          ?.scrollIntoView({ block: "nearest", inline: "center" });
      }
      if (currentSubId) {
        secondaryList
          ?.querySelector<HTMLElement>("[aria-current=page]")
          ?.scrollIntoView({ block: "nearest", inline: "center" });
      }
    });
  });

  // The active tab's underline is one element that slides between tabs
  // rather than one per tab, so switching reads as movement along the bar.
  // The secondary row's highlighted pill slides the same way.
  let indicator = $state<{ left: number; width: number } | null>(null);
  let pill = $state<{ left: number; width: number } | null>(null);
  let indicatorReady = $state(false);

  function measure(
    list: HTMLElement | undefined,
    inset: number,
  ): { left: number; width: number } | null {
    const current = list?.querySelector<HTMLElement>("[aria-current=page]");
    return current
      ? {
          left: current.offsetLeft + inset,
          width: current.offsetWidth - inset * 2,
        }
      : null;
  }

  function placeIndicator(): void {
    indicator = measure(primaryList, 12);
    pill = measure(secondaryList, 0);
  }

  $effect(() => {
    void activeItem?.id;
    void activeItem?.items?.find((item) => item.active)?.id;
    void items.length;
    void tick().then(() => {
      placeIndicator();
      // The first placement lands without sliding in from the left edge.
      requestAnimationFrame(() => (indicatorReady = true));
    });
  });

  $effect(() => {
    if (!primaryList) return;
    const observer = new ResizeObserver(placeIndicator);
    observer.observe(primaryList);
    if (secondaryList) observer.observe(secondaryList);
    return () => observer.disconnect();
  });

  function follow(
    event: MouseEvent,
    item: ContextNavItem | ContextNavSubItem,
  ): void {
    if (
      item.select &&
      event.button === 0 &&
      !event.metaKey &&
      !event.ctrlKey &&
      !event.shiftKey &&
      !event.altKey
    ) {
      event.preventDefault();
      item.select();
    }
  }
</script>

<nav
  class="border-b bg-background [view-transition-name:context-nav]"
  aria-label={label}
>
  <div class={wide ? "min-w-0" : "mx-auto max-w-5xl"}>
    <div
      class="scrollbar-none relative flex overflow-x-auto px-3 sm:px-5 lg:px-8"
      bind:this={primaryList}
    >
      {#if indicator}
        <span
          class={[
            "pointer-events-none absolute bottom-0 left-0 h-0.5 bg-foreground",
            indicatorReady &&
              "transition-[transform,width] duration-300 ease-[var(--ease-out-quint)]",
          ]}
          style:width={`${indicator.width}px`}
          style:transform={`translateX(${indicator.left}px)`}
          aria-hidden="true"
        ></span>
      {/if}
      {#each items as item (item.id)}
        <a
          href={item.href}
          aria-current={item.active ? "page" : undefined}
          class={[
            "relative flex h-12 shrink-0 items-center gap-2 px-3 text-sm text-muted-foreground transition-colors hover:text-foreground",
            item.active && "font-medium text-foreground",
          ]}
          onclick={(event) => follow(event, item)}
          onpointerenter={item.preload}
          onpointerdown={item.preload}
          onfocus={item.preload}
        >
          <item.icon class="size-4" />
          <span>{item.label}</span>
        </a>
      {/each}
    </div>

    {#if activeItem?.items?.length}
      <div
        class="scrollbar-none relative flex overflow-x-auto border-t px-3 sm:px-5 lg:px-8"
        bind:this={secondaryList}
      >
        {#if pill}
          <span
            class={[
              "pointer-events-none absolute top-1/2 left-0 h-10 rounded-md bg-muted",
              indicatorReady &&
                "transition-[translate,width] duration-300 ease-[var(--ease-out-quint)]",
            ]}
            style:width={`${pill.width}px`}
            style:translate={`${pill.left}px -50%`}
            aria-hidden="true"
          ></span>
        {/if}
        {#each activeItem.items as item (item.id)}
          <a
            href={item.href}
            aria-current={item.active ? "page" : undefined}
            class={[
              "relative flex h-10 shrink-0 items-center gap-2 rounded-md px-3 text-xs text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground",
              item.active && "text-foreground",
            ]}
            onclick={(event) => follow(event, item)}
            onpointerenter={item.preload}
            onpointerdown={item.preload}
            onfocus={item.preload}
          >
            <item.icon class="size-3.5" />
            <span>{item.label}</span>
          </a>
        {/each}
      </div>
    {/if}
  </div>
</nav>
