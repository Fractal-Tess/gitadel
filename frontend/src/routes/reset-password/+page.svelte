<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { page } from "$app/state";
  import { toast } from "svelte-sonner";

  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { passwordResetAcceptedSchema } from "$lib/api/email.js";
  import {
    ApiFailure,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const token = $derived(page.url.searchParams.get("token") ?? "");
  const available = $derived(
    (app.authStatus?.email_enabled ?? false) &&
      (app.authStatus?.authentication.password_enabled ?? false),
  );
  let login = $state("");
  let password = $state("");
  let confirmation = $state("");
  let working = $state(false);
  let requested = $state<string | null>(null);
  let validationError = $state<string | null>(null);

  function failure(caught: unknown, fallback: string): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  async function requestReset(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    working = true;
    try {
      const accepted = await requestJson(
        "/api/v1/auth/password-reset",
        passwordResetAcceptedSchema,
        { method: "POST", body: jsonBody({ login }) },
      );
      requested = accepted.message;
    } catch (caught) {
      toast.error(failure(caught, "Could not request a reset link."));
    } finally {
      working = false;
    }
  }

  async function confirmReset(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    validationError = null;
    if (password !== confirmation) {
      validationError = "Passwords do not match.";
      return;
    }
    working = true;
    try {
      await requestEmpty("/api/v1/auth/password-reset/confirm", {
        method: "POST",
        body: jsonBody({ token, password }),
      });
      password = "";
      confirmation = "";
      toast.success("Password changed. Sign in with your new password.");
      await app.refreshAuth();
      await goto(resolve("/login"));
    } catch (caught) {
      toast.error(failure(caught, "Could not reset the password."));
    } finally {
      working = false;
    }
  }
</script>

<svelte:head>
  <title>Reset password · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<main class="grid min-h-screen place-items-center bg-background px-5 py-12">
  <section
    class="motion-rise w-full max-w-md rounded-md border bg-card/25 p-6 shadow-sm"
  >
    <a class="text-sm font-bold tracking-[-0.035em]" href={resolve("/")}
      >{app.instance?.site_name ?? "GITADEL"}</a
    >
    <p
      class="mt-8 text-xs font-medium uppercase tracking-wider text-muted-foreground"
    >
      Account access
    </p>
    <h1 class="mt-2 text-2xl font-semibold">
      {token ? "Choose a new password" : "Reset your password"}
    </h1>

    {#if !available}
      <Alert.Root class="mt-6">
        <Alert.Title>Password reset is unavailable</Alert.Title>
        <Alert.Description>
          This instance cannot send email. Ask an administrator to reset your
          password.
        </Alert.Description>
      </Alert.Root>
    {:else if token}
      <p class="mt-2 text-sm text-muted-foreground">
        Use at least 12 characters. All sessions for the account will be signed
        out.
      </p>
      <form class="mt-6 grid gap-4" onsubmit={confirmReset}>
        <Field.Field>
          <Field.Label for="reset-password">New password</Field.Label>
          <Input
            id="reset-password"
            type="password"
            bind:value={password}
            autocomplete="new-password"
            minlength={12}
            maxlength={1024}
            required
          />
        </Field.Field>
        <Field.Field data-invalid={validationError ? true : undefined}>
          <Field.Label for="reset-password-confirmation"
            >Confirm password</Field.Label
          >
          <Input
            id="reset-password-confirmation"
            type="password"
            bind:value={confirmation}
            autocomplete="new-password"
            aria-invalid={validationError ? true : undefined}
            required
          />
          {#if validationError}
            <Field.Error>{validationError}</Field.Error>
          {/if}
        </Field.Field>
        <Button type="submit" disabled={working}>
          {working ? "Saving…" : "Change password"}
        </Button>
      </form>
    {:else if requested}
      <Alert.Root class="mt-6">
        <Alert.Title>Check your email</Alert.Title>
        <Alert.Description>{requested}</Alert.Description>
      </Alert.Root>
    {:else}
      <p class="mt-2 text-sm text-muted-foreground">
        Enter your username or verified email address and we will send a reset
        link.
      </p>
      <form class="mt-6 grid gap-4" onsubmit={requestReset}>
        <Field.Field>
          <Field.Label for="reset-login">Username or email</Field.Label>
          <Input
            id="reset-login"
            bind:value={login}
            autocomplete="username"
            maxlength={254}
            required
          />
        </Field.Field>
        <Button type="submit" disabled={working}>
          {working ? "Sending…" : "Send reset link"}
        </Button>
      </form>
    {/if}

    <p class="mt-6 text-center text-sm text-muted-foreground">
      <a class="underline underline-offset-4" href={resolve("/login")}
        >Back to sign in</a
      >
    </p>
  </section>
</main>
