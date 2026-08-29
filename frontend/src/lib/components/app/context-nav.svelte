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

<nav class="border-b bg-background" aria-label={label}>
  <div class={wide ? "min-w-0" : "mx-auto max-w-5xl"}>
    <div
      class="scrollbar-none flex overflow-x-auto px-3 sm:px-5 lg:px-8"
      bind:this={primaryList}
    >
      {#each items as item (item.id)}
        <a
          href={item.href}
          aria-current={item.active ? "page" : undefined}
          class={[
            "relative flex h-12 shrink-0 items-center gap-2 px-3 text-sm text-muted-foreground transition-colors hover:text-foreground",
            item.active &&
              "font-medium text-foreground after:absolute after:inset-x-3 after:bottom-0 after:h-0.5 after:bg-foreground",
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
        class="scrollbar-none flex overflow-x-auto border-t px-3 sm:px-5 lg:px-8"
        bind:this={secondaryList}
      >
        {#each activeItem.items as item (item.id)}
          <a
            href={item.href}
            aria-current={item.active ? "page" : undefined}
            class={[
              "flex h-10 shrink-0 items-center gap-2 rounded-md px-3 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground",
              item.active && "bg-muted text-foreground",
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
