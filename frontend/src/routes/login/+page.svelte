<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { page } from "$app/state";
  import { KeyRound } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import {
    ApiFailure,
    authResponseSchema,
    jsonBody,
    requestJson,
    webauthnRequestSchema,
  } from "$lib/api.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import { getCredential, requestOptions } from "$lib/webauthn.js";

  const app = useAppState();
  let username = $state("");
  let password = $state("");
  let error = $state<string | null>(null);
  let working = $state(false);

  async function runLogin(task: () => Promise<void>): Promise<void> {
    working = true;
    error = null;
    try {
      await task();
      await app.refreshAuth();
      const returnTo = page.url.searchParams.get("returnTo");
      if (returnTo?.startsWith("/admin")) {
        await goto(resolve("/admin"));
      } else if (returnTo?.startsWith("/settings")) {
        await goto(resolve("/settings"));
      } else {
        await goto(resolve("/"));
      }
    } catch (caught) {
      error =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Sign in failed.";
    } finally {
      working = false;
    }
  }

  async function loginWithPassword(): Promise<void> {
    await runLogin(async () => {
      await requestJson("/api/v1/auth/login", authResponseSchema, {
        method: "POST",
        body: jsonBody({ username, password }),
      });
      password = "";
    });
  }

  async function loginWithPasskey(): Promise<void> {
    await runLogin(async () => {
      const challenge = await requestJson(
        "/api/v1/auth/passkeys/login/start",
        webauthnRequestSchema,
        { method: "POST", body: jsonBody({ username }) },
      );
      const credential = await getCredential(
        requestOptions(challenge.options.publicKey),
      );
      await requestJson(
        "/api/v1/auth/passkeys/login/finish",
        authResponseSchema,
        {
          method: "POST",
          body: jsonBody({ challenge_id: challenge.challenge_id, credential }),
        },
      );
    });
  }
</script>

<svelte:head>
  <title>Sign in · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<main class="grid min-h-screen place-items-center bg-background px-5 py-12">
  <section class="w-full max-w-md rounded-md border bg-card/25 p-6 shadow-sm">
    <a class="text-sm font-bold tracking-[-0.035em]" href={resolve("/")}>{app.instance?.site_name ?? "GITADEL"}</a>
    <p class="mt-8 text-xs font-medium uppercase tracking-wider text-muted-foreground">
      Account access
    </p>
    <h1 class="mt-2 text-2xl font-semibold">Sign in</h1>
    <p class="mt-2 text-sm text-muted-foreground">
      Use your account to access private repositories and settings.
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
        void loginWithPassword();
      }}
    >
      <label class="grid gap-1.5 text-sm font-medium">
        Username
        <input
          class="rounded-md border bg-background px-3 py-2 outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
          bind:value={username}
          autocomplete="username"
          required
        />
      </label>
      <label class="grid gap-1.5 text-sm font-medium">
        Password
        <input
          type="password"
          class="rounded-md border bg-background px-3 py-2 outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
          bind:value={password}
          autocomplete="current-password"
          required
        />
      </label>
      <Button type="submit" disabled={working}>Sign in</Button>
    </form>

    <div class="my-5 flex items-center gap-3 text-xs text-muted-foreground">
      <span class="h-px flex-1 bg-border"></span>or<span class="h-px flex-1 bg-border"></span>
    </div>
    <Button
      class="w-full gap-2"
      variant="outline"
      disabled={working || !username}
      onclick={() => void loginWithPasskey()}
    >
      <KeyRound class="size-4" />Use a passkey
    </Button>
  </section>
</main>
