import AppWindow from "@lucide/svelte/icons/app-window";
import ArchiveRestore from "@lucide/svelte/icons/archive-restore";
import Activity from "@lucide/svelte/icons/activity";
import Database from "@lucide/svelte/icons/database";
import HardDrive from "@lucide/svelte/icons/hard-drive";
import KeySquare from "@lucide/svelte/icons/key-square";
import LockKeyhole from "@lucide/svelte/icons/lock-keyhole";
import Palette from "@lucide/svelte/icons/palette";
import ShieldCheck from "@lucide/svelte/icons/shield-check";
import Terminal from "@lucide/svelte/icons/terminal";
import UserPlus from "@lucide/svelte/icons/user-plus";
import UserRound from "@lucide/svelte/icons/user-round";
import Workflow from "@lucide/svelte/icons/workflow";

import type { ShellIcon } from "$lib/state/shell-state.svelte.js";

export const accountSettingsSections = [
  { id: "profile", label: "Profile", icon: UserRound },
  { id: "authentication", label: "Authentication", icon: LockKeyhole },
  { id: "ssh-keys", label: "SSH keys", icon: Terminal },
  { id: "api-tokens", label: "API tokens", icon: KeySquare },
  {
    id: "oauth-applications",
    label: "OAuth applications",
    icon: AppWindow,
  },
] as const satisfies readonly {
  id: string;
  label: string;
  icon: ShellIcon;
}[];

export const adminSettingsSections = [
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "access", label: "Access", icon: UserPlus },
  { id: "runners", label: "Runners", icon: Workflow },
  { id: "storage", label: "Storage", icon: Database },
  { id: "lfs", label: "Git LFS", icon: HardDrive },
  { id: "backups", label: "Backups", icon: ArchiveRestore },
  { id: "maintenance", label: "Maintenance", icon: ShieldCheck },
  { id: "activity", label: "Activity", icon: Activity },
] as const satisfies readonly {
  id: string;
  label: string;
  icon: ShellIcon;
}[];
export type AccountSettingsView =
  (typeof accountSettingsSections)[number]["id"];

export type AdminSettingsView = (typeof adminSettingsSections)[number]["id"];
