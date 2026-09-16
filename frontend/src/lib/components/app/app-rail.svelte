<script lang="ts">
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { cubicOut } from "svelte/easing";
  import { prefersReducedMotion } from "svelte/motion";
  import { crossfade } from "svelte/transition";
  import Building2 from "@lucide/svelte/icons/building-2";
  import CircleDot from "@lucide/svelte/icons/circle-dot";
  import Compass from "@lucide/svelte/icons/compass";
  import FileCode2 from "@lucide/svelte/icons/file-code-2";
  import Heart from "@lucide/svelte/icons/heart";
  import History from "@lucide/svelte/icons/history";
  import Package from "@lucide/svelte/icons/package";
  import PanelLeftClose from "@lucide/svelte/icons/panel-left-close";
  import PanelLeftOpen from "@lucide/svelte/icons/panel-left-open";
  import ScrollText from "@lucide/svelte/icons/scroll-text";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import Tag from "@lucide/svelte/icons/tag";
  import UserRound from "@lucide/svelte/icons/user-round";
  import Workflow from "@lucide/svelte/icons/workflow";
  import { organizationAvatarUrl } from "$lib/api/organizations.js";

  import * as Avatar from "$lib/components/ui/avatar/index.js";
  import * as Sidebar from "$lib/components/ui/sidebar/index.js";
  import { preloadExplore } from "$lib/navigation-cache.js";
  import { repositorySettingsSections } from "$lib/repository/settings-sections.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import {
    useShellState,
    type ShellIcon,
  } from "$lib/state/shell-state.svelte.js";

  const app = useAppState();
  const sidebar = Sidebar.useSidebar();
  const shell = useShellState();
  const viewer = $derived(app.authStatus?.user?.username);
  const scope = $derived(app.authorizationScope);
  // width, so it never needs the label as a tooltip.
  const collapsed = $derived(
    sidebar.state === "collapsed" && !sidebar.isMobile,
  );

  // Where the rail sits is the one thing it has to say clearly, so an idle row
  // is dimmed, hovering restores full contrast, and the current page also keeps
  // a white marker: two signals rather than one faint tint.
  const row =
    "relative isolate gap-3 px-3 text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground data-active:bg-transparent! data-active:font-normal data-active:text-foreground data-active:shadow-none! group-data-[collapsible=icon]:size-10! group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:gap-0 group-data-[collapsible=icon]:p-0! group-data-[collapsible=icon]:[&>span:not(.rail-highlight)]:hidden";
  const menuRow = `h-10 ${row}`;
  const subRow = `h-9 w-full justify-start text-left ${row}`;
  const [sendHighlight, receiveHighlight] = crossfade({
    duration: () => (prefersReducedMotion.current ? 0 : 220),
    easing: cubicOut,
    fallback: () => ({ duration: 0 }),
  });

  type RailSubLink = {
    label: string;
    href: string;
    icon: ShellIcon;
    active: boolean;
  };

  type RailLink = {
    label: string;
    href: string;
    icon: ShellIcon;
    active: boolean;
    avatarUrl?: string | null;
    preload?: () => void;
    items?: RailSubLink[];
  };

  const exploreLinks = $derived.by<RailLink[]>(() => {
    const onExplore = page.url.pathname === "/";
    return [
      {
        label: "Explore",
        href: resolve("/"),
        icon: Compass,
        active: onExplore && !page.url.searchParams.has("tab"),
        preload: () => preloadExplore(scope),
      },
    ];
  });

  const repositoryLinks = $derived.by<RailLink[]>(() => {
    const namespace = page.params.namespace;
    const repositoryName = page.params.name;
    if (!namespace || !repositoryName) return [];
    const base = resolve("/[namespace]/[name]", {
      namespace,
      name: repositoryName,
    });
    const view = page.url.searchParams.get("view") ?? "overview";
    const current =
      shell.activeRepository?.namespace === namespace &&
      shell.activeRepository.name === repositoryName
        ? shell.activeRepository
        : null;
    return [
      {
        label: "Code",
        href: base,
        icon: FileCode2,
        active: view === "overview",
      },
      {
        label: "History",
        href: `${base}?view=history`,
        icon: History,
        active: view === "history" || view === "commit",
      },
      {
        label: "Actions",
        href: `${base}?view=actions`,
        icon: Workflow,
        active: view === "actions",
      },
      {
        label: "Issues",
        href: `${base}?view=issues`,
        icon: CircleDot,
        active: view === "issues",
      },
      {
        label: "Releases",
        href: `${base}?view=releases`,
        icon: Package,
        active: view === "releases",
      },
      {
        label: "Tags",
        href: `${base}?view=tags`,
        icon: Tag,
        active: view === "tags",
      },
      ...(current?.canManage
        ? [
            {
              label: "Settings",
              href: `${base}?view=settings`,
              icon: Settings2,
              active: view === "settings" || view === "integrations",
              items: repositorySettingsSections
                .filter(
                  (section) => section.id !== "mirror" || current.mirrored,
                )
                .map((section) => ({
                  label: section.label,
                  href: `${base}?view=settings${
                    section.id === "general" ? "" : `&tab=${section.id}`
                  }`,
                  icon: section.icon,
                  active:
                    view === "settings" &&
                    (page.url.searchParams.get("tab") ?? "general") ===
                      section.id,
                })),
            },
          ]
        : []),
    ];
  });

  const personalLinks = $derived.by<RailLink[]>(() => {
    if (!viewer) return [];
    return [
      {
        label: viewer,
        href: resolve("/[namespace]", { namespace: viewer }),
        icon: UserRound,
        active: page.params.namespace === viewer,
        preload: () => preloadExplore(scope, viewer),
      },
      {
        label: "Favorites",
        href: `${resolve("/")}?tab=favorites`,
        icon: Heart,
        active:
          page.url.pathname === "/" &&
          page.url.searchParams.get("tab") === "favorites",
        preload: () => preloadExplore(scope),
      },
    ];
  });

  const organizationLinks = $derived.by<RailLink[]>(() =>
    app.organizations.map((organization) => ({
      label: organization.display_name || organization.slug,
      href: resolve("/[namespace]", { namespace: organization.slug }),
      icon: Building2,
      avatarUrl: organizationAvatarUrl(
        organization.slug,
        organization.avatar_updated_at,
      ),
      active: page.params.namespace === organization.slug,
      preload: () => preloadExplore(scope, organization.slug),
    })),
  );

  const manageLinks = $derived.by<RailLink[]>(() => {
    if (!app.authStatus?.authenticated) {
      return [
        {
          label: "Changelog",
          href: resolve("/changelog"),
          icon: ScrollText,
          active: page.url.pathname === "/changelog",
        },
      ];
    }
    const links: RailLink[] = [
      {
        label: "Account settings",
        href: resolve("/-/account/[view]", { view: "profile" }),
        icon: Settings2,
        active: page.url.pathname.startsWith("/-/account"),
      },
    ];
    if (app.authStatus.user?.is_admin) {
      links.push({
        label: "Administration",
        href: resolve("/-/administration/[view]", { view: "appearance" }),
        icon: ShieldCheck,
        active: page.url.pathname.startsWith("/-/administration"),
      });
    }
    links.push({
      label: "Changelog",
      href: resolve("/changelog"),
      icon: ScrollText,
      active: page.url.pathname === "/changelog",
    });
    return links;
  });

  function dismissMobile(): void {
    sidebar.setOpenMobile(false);
  }
</script>

{#snippet activeHighlight(key: string)}
  <span
    class="rail-highlight pointer-events-none absolute inset-0 -z-10 rounded-md bg-sidebar-accent shadow-[inset_3px_0_0_0_white]"
    aria-hidden="true"
    in:receiveHighlight={{ key }}
    out:sendHighlight={{ key }}
  ></span>
{/snippet}

{#snippet linkMenu(links: RailLink[], label: string, highlightKey: string)}
  <Sidebar.Menu aria-label={label}>
    {#each links as link (link.label)}
      <Sidebar.MenuItem>
        <Sidebar.MenuButton isActive={link.active} class={menuRow}>
          {#snippet child({ props })}
            <a
              {...props}
              href={link.href}
              aria-current={link.active ? "page" : undefined}
              title={collapsed ? link.label : undefined}
              onpointerenter={link.preload}
              onfocus={link.preload}
              onclick={dismissMobile}
            >
              {#if link.active}
                {@render activeHighlight(highlightKey)}
              {/if}
              {#if link.avatarUrl}
                <Avatar.Root class="size-4 shrink-0">
                  <Avatar.Image src={link.avatarUrl} alt="" />
                  <Avatar.Fallback>
                    <Building2 class="size-3" />
                  </Avatar.Fallback>
                </Avatar.Root>
              {:else}
                <link.icon />
              {/if}
              <span>{link.label}</span>
            </a>
          {/snippet}
        </Sidebar.MenuButton>
        {#if link.active && link.items?.length}
          <Sidebar.MenuSub>
            {#each link.items as item (item.label)}
              <Sidebar.MenuSubItem>
                <Sidebar.MenuSubButton isActive={item.active} class={subRow}>
                  {#snippet child({ props })}
                    <a
                      {...props}
                      href={item.href}
                      aria-current={item.active ? "page" : undefined}
                      onclick={dismissMobile}
                    >
                      <item.icon />
                      <span>{item.label}</span>
                    </a>
                  {/snippet}
                </Sidebar.MenuSubButton>
              </Sidebar.MenuSubItem>
            {/each}
          </Sidebar.MenuSub>
        {/if}
      </Sidebar.MenuItem>
    {/each}
  </Sidebar.Menu>
{/snippet}

<!-- The shell keeps its own full-width header, so the rail starts below it
     instead of taking the whole viewport height the way shadcn's default
     dashboard layout does. -->
<Sidebar.Root collapsible="icon" class="md:top-16 md:h-[calc(100svh-4rem)]">
  <!-- Inside a repository the rail is a repository menu and nothing else: the
       places groups would push the views most of the way down the column, and
       the header breadcrumb already leads back out to the namespace. -->
  <Sidebar.Content class="py-2">
    {#if repositoryLinks.length}
      <Sidebar.Group>
        <Sidebar.GroupLabel>Repository</Sidebar.GroupLabel>
        <Sidebar.GroupContent>
          {@render linkMenu(repositoryLinks, "Repository", "repository")}
        </Sidebar.GroupContent>
      </Sidebar.Group>
    {:else}
      <Sidebar.Group>
        <Sidebar.GroupContent>
          {@render linkMenu(exploreLinks, "Explore", "places")}
        </Sidebar.GroupContent>
      </Sidebar.Group>

      {#if personalLinks.length}
        <Sidebar.Group class="mt-3 border-t pt-3">
          <Sidebar.GroupLabel>Personal</Sidebar.GroupLabel>
          <Sidebar.GroupContent>
            {@render linkMenu(personalLinks, "Personal", "places")}
          </Sidebar.GroupContent>
        </Sidebar.Group>
      {/if}

      {#if organizationLinks.length}
        <Sidebar.Group class="mt-3 border-t pt-3">
          <Sidebar.GroupLabel>Organizations</Sidebar.GroupLabel>
          <Sidebar.GroupContent>
            {@render linkMenu(organizationLinks, "Organizations", "places")}
          </Sidebar.GroupContent>
        </Sidebar.Group>
      {/if}
    {/if}
  </Sidebar.Content>

  <Sidebar.Footer class="mt-auto border-t">
    <Sidebar.Menu>
      <Sidebar.MenuItem class="hidden md:block">
        <Sidebar.MenuButton
          class={menuRow}
          aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
          aria-expanded={!collapsed}
          title={collapsed ? "Expand navigation" : undefined}
          onclick={() => sidebar.toggle()}
        >
          {#if collapsed}
            <PanelLeftOpen />
          {:else}
            <PanelLeftClose />
            <span>Collapse sidebar</span>
          {/if}
        </Sidebar.MenuButton>
      </Sidebar.MenuItem>
    </Sidebar.Menu>
    {@render linkMenu(manageLinks, "Manage", "manage")}
  </Sidebar.Footer>

  <Sidebar.Rail />
</Sidebar.Root>
