import { getContext, setContext } from "svelte";
import type { Icon } from "lucide-svelte";

const SHELL_STATE = Symbol("gitadel-shell-state");
const RAIL_STORAGE_KEY = "gitadel:rail-open";

/**
 * lucide-svelte still ships legacy component classes, so icons are typed by the
 * package's own base component rather than Svelte 5's `Component`.
 */
export type ShellIcon = typeof Icon;

export type ShellNavItem = {
  id: string;
  label: string;
  icon: ShellIcon;
  active: boolean;
  select: () => void;
};

/**
 * The area-specific navigation the rail shows beneath the global links. Pages
 * own their sub-views (repository tabs, settings tabs), so they publish them
 * here instead of the rail reaching into page state.
 */
export type ShellNavGroup = {
  label: string;
  items: ShellNavItem[];
};

export class ShellState {
  navGroup = $state.raw<ShellNavGroup | null>(null);
  // Collapsed by default so first-time visitors get the widest possible content
  // column; the stored preference takes over from the second visit onwards.
  railOpen = $state(false);
  railMobileOpen = $state(false);
  paletteOpen = $state(false);
  createOpen = $state(false);

  constructor() {
    const stored = globalThis.localStorage?.getItem(RAIL_STORAGE_KEY);
    if (stored !== null) this.railOpen = stored === "true";
  }

  toggleRail(): void {
    this.railOpen = !this.railOpen;
    globalThis.localStorage?.setItem(RAIL_STORAGE_KEY, String(this.railOpen));
  }

  /**
   * Publishes a navigation group for as long as the calling component lives.
   * Call from an `$effect` so the group follows the page's active view and is
   * torn down when the page unmounts.
   */
  publishNavGroup(group: ShellNavGroup): () => void {
    this.navGroup = group;
    return () => {
      if (this.navGroup === group) this.navGroup = null;
    };
  }
}

export function provideShellState(): ShellState {
  const state = new ShellState();
  setContext(SHELL_STATE, state);
  return state;
}

export function useShellState(): ShellState {
  return getContext<ShellState>(SHELL_STATE);
}
