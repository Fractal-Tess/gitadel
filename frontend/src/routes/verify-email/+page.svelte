<script lang="ts">
  import { resolve } from "$app/paths";
  import { page } from "$app/state";

  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { ApiFailure, jsonBody, requestEmpty } from "$lib/api/transport.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const token = page.url.searchParams.get("token") ?? "";
  let outcome = $state<"pending" | "verified" | "failed">(
    token ? "pending" : "failed",
  );
  let message = $state(token ? "" : "The verification link is incomplete.");
  let working = $state(false);

  // Verification is an explicit click: mail scanners that prefetch links
  // must not consume the single-use token.
  async function verify(): Promise<void> {
    working = true;
    try {
      await requestEmpty("/api/v1/auth/email/verify", {
        method: "POST",
        body: jsonBody({ token }),
      });
      outcome = "verified";
    } catch (caught) {
      outcome = "failed";
      message =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "The address could not be verified.";
    } finally {
      working = false;
    }
  }
</script>

<svelte:head>
  <title>Verify email · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<main class="grid min-h-screen place-items-center bg-background px-5 py-12">
  <section
    class="motion-rise w-full max-w-md rounded-md border bg-card/25 p-6 shadow-sm"
  >
    <a class="text-sm font-bold tracking-[-0.035em]" href={resolve("/")}
      >{app.instance?.site_name ?? "GITADEL"}</a
    >
    <h1 class="mt-8 text-2xl font-semibold">Verify your email address</h1>

    {#if outcome === "pending"}
      <p class="mt-2 text-sm text-muted-foreground">
        Confirm that this address belongs to your Gitadel account.
      </p>
      <Button
        class="mt-6 w-full"
        disabled={working}
        onclick={() => void verify()}
      >
        {working ? "Verifying…" : "Verify email address"}
      </Button>
    {:else if outcome === "verified"}
      <Alert.Root class="mt-6">
        <Alert.Title>Email address verified</Alert.Title>
        <Alert.Description>
          It can now receive password reset links and notifications.
        </Alert.Description>
      </Alert.Root>
    {:else}
      <Alert.Root class="mt-6" variant="destructive">
        <Alert.Title>Verification failed</Alert.Title>
        <Alert.Description>
          {message} Request a new link from your account settings.
        </Alert.Description>
      </Alert.Root>
    {/if}

    <p class="mt-6 text-center text-sm text-muted-foreground">
      {#if app.authStatus?.authenticated}
        <a
          class="underline underline-offset-4"
          href={resolve("/-/account/[view]", { view: "profile" })}
          >Account settings</a
        >
      {:else}
        <a class="underline underline-offset-4" href={resolve("/login")}
          >Sign in</a
        >
      {/if}
    </p>
  </section>
</main>
