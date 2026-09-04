import { getContext, setContext } from "svelte";
import type { Component } from "svelte";

const SHELL_STATE = Symbol("gitadel-shell-state");
const RAIL_STORAGE_KEY = "gitadel:rail-open";

export type ShellIcon = Component;

export type ActiveRepositoryNavigation = {
  namespace: string;
  name: string;
  canManage: boolean;
  mirrored: boolean;
};

export class ShellState {
  // Collapsed by default so first-time visitors get the widest possible content
  // column; the stored preference takes over from the second visit onwards.
  railOpen = $state(false);
  paletteOpen = $state(false);
  createOpen = $state(false);
  railHidden = $state(false);
  activeRepository = $state.raw<ActiveRepositoryNavigation | null>(null);

  constructor() {
    const stored = globalThis.localStorage?.getItem(RAIL_STORAGE_KEY);
    if (stored !== null) this.railOpen = stored === "true";
  }

  setRailOpen(open: boolean): void {
    this.railOpen = open;
    globalThis.localStorage?.setItem(RAIL_STORAGE_KEY, String(open));
  }

  setActiveRepository(repository: ActiveRepositoryNavigation | null): void {
    this.activeRepository = repository;
  }

  setRailHidden(hidden: boolean): void {
    this.railHidden = hidden;
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
