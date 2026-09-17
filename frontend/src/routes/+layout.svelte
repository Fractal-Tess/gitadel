<script lang="ts">
  import "../app.css";

  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { resolve } from "$app/paths";
  import { ModeWatcher, mode } from "mode-watcher";

  import AppHeader from "$lib/components/app/app-header.svelte";
  import AppRail from "$lib/components/app/app-rail.svelte";
  import CommandPalette from "$lib/components/app/command-palette.svelte";
  import CreateDialog from "$lib/components/app/create-dialog.svelte";
  import FileCreateDialog from "$lib/components/app/file-create-dialog.svelte";
  import OrganizationContextNav from "$lib/components/app/organization-context-nav.svelte";
  import * as Sidebar from "$lib/components/ui/sidebar/index.js";
  import { Toaster } from "$lib/components/ui/sonner/index.js";
  import { provideAppState } from "$lib/state/app-state.svelte.js";
  import { provideShellState } from "$lib/state/shell-state.svelte.js";

  let { children } = $props();
  const app = provideAppState();
  const shell = provideShellState();
  let ready = $state(false);
  let guardSequence = 0;
  let faviconVersion = $derived(
    encodeURIComponent(app.instance?.updated_at ?? "default"),
  );
  // Sign-in, registration and OAuth consent render without the app shell:
  // each is a single decision the visitor must finish before navigating.
  const bareRoutes = new Set(["/login", "/register", "/oauth/consent"]);
  let bare = $derived(bareRoutes.has(page.url.pathname));

  function isProtectedPath(pathname: string): boolean {
    return (
      pathname === "/settings" ||
      pathname.startsWith("/-/") ||
      managedNamespace(pathname) !== null
    );
  }

  function managedNamespace(pathname: string): string | null {
    const match = pathname.match(
      /^\/([^/]+)\/(?:members|runners|integrations|mirror-credentials)\/?$/u,
    );
    return match?.[1] ? decodeURIComponent(match[1]) : null;
  }

  $effect(() => {
    const url = page.url;
    const sequence = ++guardSequence;
    ready = canRenderWhileGuarding(url);
    void guardRoute(url, sequence);
  });

  function canRenderWhileGuarding(url: URL) {
    const status = app.authStatus;
    if (!status || !app.instance) return false;
    if (status.setup_required) return url.pathname === "/register";
    if (url.pathname === "/register" && !url.searchParams.has("token")) {
      return false;
    }
    if (url.pathname === "/login" && status.authenticated) return false;
    if (isProtectedPath(url.pathname) && !status.authenticated) return false;
    if (
      url.pathname.startsWith("/-/administration/") &&
      !status.user?.is_admin
    ) {
      return false;
    }
    const namespace = managedNamespace(url.pathname);
    if (namespace && namespace !== status.user?.username) {
      return app.organizations.some(
        (organization) => organization.slug === namespace,
      );
    }
    return true;
  }

  function recordCompletedLoginMethod(url: URL): void {
    const method = url.searchParams.get("loginMethod");
    if (!method?.startsWith("sso:")) return;
    try {
      globalThis.localStorage?.setItem("gitadel:last-login-method", method);
    } catch {
      // Authentication does not depend on browser storage.
    }
  }
  async function guardRoute(url: URL, sequence: number): Promise<void> {
    recordCompletedLoginMethod(url);
    try {
      const status = await app.initialize();
      if (sequence !== guardSequence) return;

      if (status.setup_required && url.pathname !== "/register") {
        await goto(resolve("/register"), { replaceState: true });
        return;
      }
      if (
        !status.setup_required &&
        url.pathname === "/register" &&
        !url.searchParams.has("token")
      ) {
        await goto(resolve("/login"), { replaceState: true });
        return;
      }

      if (isProtectedPath(url.pathname) && !status.authenticated) {
        const returnTo = encodeURIComponent(`${url.pathname}${url.search}`);
        await goto(resolve(`/login?returnTo=${returnTo}`), {
          replaceState: true,
        });
        return;
      }
      if (
        url.pathname.startsWith("/-/administration/") &&
        !status.user?.is_admin
      ) {
        await goto(resolve("/-/account/[view]", { view: "profile" }), {
          replaceState: true,
        });
        return;
      }
      const namespace = managedNamespace(url.pathname);
      if (namespace && namespace !== status.user?.username) {
        const organizations = await app.refreshOrganizations();
        if (sequence !== guardSequence) return;
        const canAccess = organizations.some(
          (organization) => organization.slug === namespace,
        );
        if (!canAccess) {
          await goto(resolve("/[namespace]", { namespace }), {
            replaceState: true,
          });
          return;
        }
      }
      if (url.pathname === "/login" && status.authenticated) {
        const returnTo = url.searchParams.get("returnTo");
        if (returnTo?.startsWith("/login/oauth/authorize?")) {
          // Server-side route: a client-side goto would hand the OAuth
          // authorize path to the SPA router, which has no such page.
          window.location.assign(returnTo);
        } else if (
          returnTo?.startsWith("/settings") ||
          returnTo?.startsWith("/-/") ||
          (returnTo && managedNamespace(returnTo.split("?")[0] ?? "") !== null)
        ) {
          window.location.assign(returnTo);
        } else {
          await goto(resolve("/"), { replaceState: true });
        }
        return;
      }
      ready = true;
    } catch {
      if (sequence === guardSequence) ready = true;
    }
  }
</script>

<ModeWatcher />

<svelte:head>
  <link
    rel="icon"
    href={`/api/v1/instance/favicon/${mode.current ?? "light"}?v=${faviconVersion}&r=2`}
  />
</svelte:head>

<Toaster theme={mode.current ?? "system"} position="bottom-right" />

{#if ready && bare}
  {@render children()}
{:else if ready}
  <!-- The rail is a shadcn sidebar, so the whole shell lives inside its
       provider: the header's mobile trigger and the rail itself both read the
       same open state from context. -->
  <Sidebar.Provider
    bind:open={shell.railOpen}
    onOpenChange={(open) => shell.setRailOpen(open)}
    style="--sidebar-width: 15rem; --sidebar-width-icon: 3.5rem;"
    class="h-svh min-h-0 flex-col overflow-hidden bg-background"
  >
    <AppHeader />
    <div class="flex min-h-0 w-full flex-1">
      {#if !shell.railHidden}
        <AppRail />
      {/if}
      <main
        class="min-h-0 flex-1 overflow-y-auto overscroll-contain"
        data-scroll-region
      >
        <OrganizationContextNav />
        {@render children()}
      </main>
    </div>
  </Sidebar.Provider>
  <CommandPalette />
  <CreateDialog />
  <FileCreateDialog />
{:else}
  <div
    class="grid min-h-screen place-items-center bg-background text-sm text-muted-foreground"
  >
    Loading Gitadel…
  </div>
{/if}
