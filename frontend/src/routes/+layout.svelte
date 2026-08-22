<script lang="ts">
  import "../app.css";

  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { resolve } from "$app/paths";

  import AppHeader from "$lib/components/app/app-header.svelte";
  import AppRail from "$lib/components/app/app-rail.svelte";
  import CommandPalette from "$lib/components/app/command-palette.svelte";
  import CreateRepositoryDialog from "$lib/components/app/create-repository-dialog.svelte";
  import { Toaster } from "$lib/components/ui/sonner/index.js";
  import { provideAppState } from "$lib/state/app-state.svelte.js";
  import { provideShellState } from "$lib/state/shell-state.svelte.js";

  let { children } = $props();
  const app = provideAppState();
  provideShellState();
  let ready = $state(false);
  let guardSequence = 0;
  let faviconVersion = $derived(
    encodeURIComponent(app.instance?.updated_at ?? "default"),
  );
  // Sign-in and registration are the only routes without the app shell: there
  // is nothing to navigate to until the visitor is through them.
  let bare = $derived(
    page.url.pathname === "/login" || page.url.pathname === "/register",
  );

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
    if (url.pathname === "/settings" && !status.authenticated) return false;
    if (url.pathname.startsWith("/admin"))
      return Boolean(status.user?.is_admin);
    return true;
  }

  async function guardRoute(url: URL, sequence: number): Promise<void> {
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

      const protectedRoute =
        url.pathname === "/settings" || url.pathname.startsWith("/admin");
      if (protectedRoute && !status.authenticated) {
        const returnTo = encodeURIComponent(`${url.pathname}${url.search}`);
        await goto(resolve(`/login?returnTo=${returnTo}`), {
          replaceState: true,
        });
        return;
      }
      if (url.pathname.startsWith("/admin") && !status.user?.is_admin) {
        await goto(resolve("/"), { replaceState: true });
        return;
      }
      if (url.pathname === "/login" && status.authenticated) {
        const returnTo = url.searchParams.get("returnTo");
        if (returnTo?.startsWith("/admin")) {
          await goto(resolve("/admin"), { replaceState: true });
        } else if (returnTo?.startsWith("/settings")) {
          await goto(resolve("/settings"), { replaceState: true });
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

<svelte:head>
  <link
    rel="icon"
    href={`/api/v1/instance/favicon/light?v=${faviconVersion}&r=2`}
    media="(prefers-color-scheme: light)"
  />
  <link
    rel="icon"
    href={`/api/v1/instance/favicon/dark?v=${faviconVersion}&r=2`}
    media="(prefers-color-scheme: dark)"
  />
</svelte:head>

<!-- The theme is dark-only, so pin it rather than reading mode-watcher. -->
<Toaster theme="dark" position="bottom-right" />

{#if ready && bare}
  {@render children()}
{:else if ready}
  <div class="flex h-svh flex-col overflow-hidden bg-background">
    <AppHeader />
    <div class="flex min-h-0 flex-1">
      <AppRail />
      <main
        class="min-h-0 flex-1 overflow-y-auto overscroll-contain"
        data-scroll-region
      >
        {@render children()}
      </main>
    </div>
  </div>
  <CommandPalette />
  <CreateRepositoryDialog />
{:else}
  <div
    class="grid min-h-screen place-items-center bg-background text-sm text-muted-foreground"
  >
    Loading Gitadel…
  </div>
{/if}
