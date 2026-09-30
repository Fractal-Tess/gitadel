import { getContext, setContext } from "svelte";
import type { Component } from "svelte";

const SHELL_STATE = Symbol("gitadel-shell-state");
const RAIL_STORAGE_KEY = "gitadel:rail-open";
const DETAILS_STORAGE_KEY = "gitadel:repository-details-open";

export type CreateMode = "choose" | "repository" | "mirror" | "organization";
export type FileCreateMode = "file" | "directory";

export type ShellIcon = Component;

export type ActiveRepositoryNavigation = {
  namespace: string;
  name: string;
  canManage: boolean;
  canWrite: boolean;
  mirrored: boolean;
  personal: boolean;
};

export class ShellState {
  // Collapsed by default so first-time visitors get the widest possible content
  // column; the stored preference takes over from the second visit onwards.
  railOpen = $state(false);
  paletteOpen = $state(false);
  createOpen = $state(false);
  createMode = $state<CreateMode>("choose");
  fileCreateOpen = $state(false);
  fileCreateMode = $state<FileCreateMode>("file");
  railHidden = $state(false);
  /** Whether a repository page shows its details panel on the right. */
  detailsOpen = $state(true);
  activeRepository = $state.raw<ActiveRepositoryNavigation | null>(null);

  constructor() {
    const stored = globalThis.localStorage?.getItem(RAIL_STORAGE_KEY);
    if (stored !== null) this.railOpen = stored === "true";
    const details = globalThis.localStorage?.getItem(DETAILS_STORAGE_KEY);
    if (details !== null && details !== undefined)
      this.detailsOpen = details === "true";
  }

  setDetailsOpen(open: boolean): void {
    this.detailsOpen = open;
    globalThis.localStorage?.setItem(DETAILS_STORAGE_KEY, String(open));
  }

  setRailOpen(open: boolean): void {
    this.railOpen = open;
    globalThis.localStorage?.setItem(RAIL_STORAGE_KEY, String(open));
  }

  openCreate(mode: CreateMode = "choose"): void {
    this.createMode = mode;
    this.createOpen = true;
  }
  openFileCreate(mode: FileCreateMode = "file"): void {
    this.fileCreateMode = mode;
    this.fileCreateOpen = true;
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
