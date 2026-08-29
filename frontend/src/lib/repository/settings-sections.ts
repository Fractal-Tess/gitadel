import {
  MapPin,
  Plug,
  RefreshCw,
  Settings2,
  TriangleAlert,
  Webhook,
} from "lucide-svelte";

import type { ShellIcon } from "$lib/state/shell-state.svelte.js";

/**
 * Repository settings is a section per page rather than a strip of tabs, so the
 * rail and the page itself have to agree on the same list.
 */
export const repositorySettingsSections = [
  { id: "general", label: "General", icon: Settings2 },
  { id: "location", label: "Location", icon: MapPin },
  { id: "mirror", label: "Mirror", icon: RefreshCw },
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
