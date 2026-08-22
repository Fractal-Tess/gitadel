<script lang="ts">
  import "../app.css";

  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { resolve } from "$app/paths";

  import { Toaster } from "$lib/components/ui/sonner/index.js";
  import { provideAppState } from "$lib/state/app-state.svelte.js";

  let { children } = $props();
  const app = provideAppState();
  let ready = $state(false);
  let guardSequence = 0;
  let faviconVersion = $derived(
    encodeURIComponent(app.instance?.updated_at ?? "default"),
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

{#if ready}
  {@render children()}
{:else}
  <div
    class="grid min-h-screen place-items-center bg-background text-sm text-muted-foreground"
  >
    Loading Gitadel…
  </div>
{/if}
