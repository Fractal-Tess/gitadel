<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { page } from "$app/state";
  import { ShieldCheck, UserPlus } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import {
    ApiFailure,
    authResponseSchema,
    jsonBody,
    requestJson,
  } from "$lib/api.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const setupRequired = $derived(app.authStatus?.setup_required ?? false);
  const invitationToken = $derived(page.url.searchParams.get("token") ?? "");
  let username = $state("");
  let password = $state("");
  let confirmation = $state("");
  let error = $state<string | null>(null);
  let working = $state(false);

  async function createAccount(): Promise<void> {
    error = null;
    if (password !== confirmation) {
      error = "Passwords do not match.";
      return;
    }
    working = true;
    try {
      const creatingAdministrator = setupRequired;
      await requestJson(
        creatingAdministrator ? "/api/v1/setup" : "/api/v1/register",
        authResponseSchema,
        {
          method: "POST",
          body: creatingAdministrator
            ? jsonBody({ username, password })
            : jsonBody({ token: invitationToken, username, password }),
        },
      );
      await app.refreshAuth();
      if (creatingAdministrator) {
        await goto(resolve("/admin"));
      } else {
        await goto(resolve("/"));
      }
    } catch (caught) {
      error =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not create the account.";
    } finally {
      working = false;
    }
  }
</script>

<svelte:head>
  <title>{setupRequired ? "Set up" : "Join"} · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<main class="grid min-h-screen place-items-center bg-background px-5 py-12">
  <section class="w-full max-w-md rounded-md border bg-card/25 p-6 shadow-sm">
    <div class="flex size-10 items-center justify-center rounded-md border bg-background">
      {#if setupRequired}
        <ShieldCheck class="size-5" />
      {:else}
        <UserPlus class="size-5" />
      {/if}
    </div>
    <p class="mt-7 text-xs font-medium uppercase tracking-wider text-muted-foreground">
      {setupRequired ? "Initial setup" : "Invitation"}
    </p>
    <h1 class="mt-2 text-2xl font-semibold">
      {setupRequired ? "Create the administrator" : "Create your account"}
    </h1>
    <p class="mt-2 text-sm leading-6 text-muted-foreground">
      {setupRequired
        ? "This is the only open registration. Further accounts require an administrator invitation."
        : "This private invitation grants access to this Gitadel instance."}
    </p>

    {#if error}
      <p class="mt-5 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
        {error}
      </p>
    {/if}

    <form
      class="mt-6 grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void createAccount();
      }}
    >
      <label class="grid gap-1.5 text-sm font-medium">
        Username
        <input
          class="rounded-md border bg-background px-3 py-2 outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
          bind:value={username}
          autocomplete="username"
          minlength="3"
          required
        />
      </label>
      <label class="grid gap-1.5 text-sm font-medium">
        Password
        <input
          type="password"
          class="rounded-md border bg-background px-3 py-2 outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
          bind:value={password}
          autocomplete="new-password"
          minlength="12"
          required
        />
      </label>
      <label class="grid gap-1.5 text-sm font-medium">
        Confirm password
        <input
          type="password"
          class="rounded-md border bg-background px-3 py-2 outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
          bind:value={confirmation}
          autocomplete="new-password"
          minlength="12"
          required
        />
      </label>
      <Button class="mt-2" type="submit" disabled={working || (!setupRequired && !invitationToken)}>
        {setupRequired ? "Create administrator" : "Create account"}
      </Button>
    </form>
  </section>
</main>
