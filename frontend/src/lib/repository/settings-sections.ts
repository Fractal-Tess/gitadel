import GitBranch from "@lucide/svelte/icons/git-branch";
import KeyRound from "@lucide/svelte/icons/key-round";
import MapPin from "@lucide/svelte/icons/map-pin";
import Plug from "@lucide/svelte/icons/plug";
import RefreshCw from "@lucide/svelte/icons/refresh-cw";
import Settings2 from "@lucide/svelte/icons/settings-2";
import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
import Webhook from "@lucide/svelte/icons/webhook";

import type { ShellIcon } from "$lib/state/shell-state.svelte.js";

/**
 * Repository settings is a section per page rather than a strip of tabs, so the
 * rail and the page itself have to agree on the same list.
 */
export const repositorySettingsSections = [
  { id: "general", label: "General", icon: Settings2 },
  { id: "location", label: "Location", icon: MapPin },
  { id: "mirror", label: "Mirror", icon: RefreshCw },
  { id: "branches", label: "Branches", icon: GitBranch },
  { id: "deploy-keys", label: "Deploy keys", icon: KeyRound },
  { id: "webhooks", label: "Webhooks", icon: Webhook },
  { id: "integrations", label: "Integrations", icon: Plug },
  { id: "danger", label: "Danger zone", icon: TriangleAlert },
] as const satisfies readonly {
  id: string;
  label: string;
  icon: ShellIcon;
}[];

export type RepositorySettingsSection =
  (typeof repositorySettingsSections)[number]["id"];

export function isRepositorySettingsSection(
  value: string,
): value is RepositorySettingsSection {
  return repositorySettingsSections.some((section) => section.id === value);
}
