<script module lang="ts">
  export type OrganizationView =
    | "repositories"
    | "members"
    | "runners"
    | "integrations"
    | "mirror-credentials"
    | "settings";
</script>

<script lang="ts">
  import { resolve } from "$app/paths";
  import {
    GitBranch,
    KeyRound,
    Rocket,
    Server,
    Settings2,
    Users,
  } from "lucide-svelte";

  import ContextNav, {
    type ContextNavItem,
  } from "$lib/components/app/context-nav.svelte";
  import {
    preloadNamespaceTabs,
    refreshNamespaceTab,
  } from "$lib/namespace-preload.js";

  let {
    slug,
    label,
    active,
    showMembers = true,
    canManage = false,
    scope = "organization",
  }: {
    slug: string;
    label: string;
    active: OrganizationView;
    showMembers?: boolean;
    canManage?: boolean;
    scope?: "organization" | "personal";
  } = $props();

  $effect(() => {
    preloadNamespaceTabs(slug, {
      members: showMembers,
      management: canManage,
    });
  });

  const navigation = $derived.by<ContextNavItem[]>(() => [
    {
      id: "repositories",
      label: "Repositories",
      icon: GitBranch,
      href: resolve("/[namespace]", { namespace: slug }),
      active: active === "repositories",
    },
    ...(showMembers
      ? [
          {
            id: "members",
            label: "Members",
            icon: Users,
            href: resolve("/[namespace]/members", { namespace: slug }),
            active: active === "members",
            preload: () => refreshNamespaceTab(slug, "members"),
          },
        ]
      : []),
    ...(canManage
      ? [
          {
            id: "runners",
            label: "Runners",
            icon: Server,
            href: resolve("/[namespace]/runners", { namespace: slug }),
            active: active === "runners",
            preload: () => refreshNamespaceTab(slug, "runners"),
          },
          {
            id: "integrations",
            label: "Integrations",
            icon: Rocket,
            href: resolve("/[namespace]/integrations", { namespace: slug }),
            active: active === "integrations",
            preload: () => refreshNamespaceTab(slug, "integrations"),
          },
          {
            id: "mirror-credentials",
            label: "Mirror identities",
            icon: KeyRound,
            href: resolve("/[namespace]/mirror-credentials", {
              namespace: slug,
            }),
            active: active === "mirror-credentials",
            preload: () => refreshNamespaceTab(slug, "mirror-credentials"),
          },
        ]
      : []),
    ...(canManage && scope === "organization"
      ? [
          {
            id: "settings",
            label: "Settings",
            icon: Settings2,
            href: resolve("/[namespace]/settings", { namespace: slug }),
            active: active === "settings",
          },
        ]
      : []),
  ]);
</script>

<ContextNav
  label={`${label} ${scope === "personal" ? "personal namespace" : "organization"}`}
  items={navigation}
/>
