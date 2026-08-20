<script lang="ts">
  import "../app.css";

  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { resolve } from "$app/paths";

  import { provideAppState } from "$lib/state/app-state.svelte.js";

  let { children } = $props();
  const app = provideAppState();
  let ready = $state(false);
  let guardSequence = 0;

  $effect(() => {
    const url = page.url;
    const sequence = ++guardSequence;
    ready = false;
    void guardRoute(url, sequence);
  });

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

{#if ready}
  {@render children()}
{:else}
  <div class="grid min-h-screen place-items-center bg-background text-sm text-muted-foreground">
    Loading Gitadel…
  </div>
{/if}
