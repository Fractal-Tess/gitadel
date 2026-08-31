<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { page } from "$app/state";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import UserPlus from "@lucide/svelte/icons/user-plus";
  import { toast } from "svelte-sonner";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import {
    ApiFailure,
    jsonBody,
    requestJson,
  } from "$lib/api/transport.js";
  import { authResponseSchema } from "$lib/api/auth.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const setupRequired = $derived(app.authStatus?.setup_required ?? false);
  const invitationToken = $derived(page.url.searchParams.get("token") ?? "");
  let username = $state("");
  let password = $state("");
  let confirmation = $state("");
  let validationError = $state<string | null>(null);
  let working = $state(false);

  async function createAccount(): Promise<void> {
    validationError = null;
    if (password !== confirmation) {
      validationError = "Passwords do not match.";
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
        await goto(resolve("/-/administration/[view]", { view: "appearance" }));
      } else {
        await goto(resolve("/"));
      }
    } catch (caught) {
      toast.error(
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not create the account.",
      );
    } finally {
      working = false;
    }
  }
</script>

<svelte:head>
  <title
    >{setupRequired ? "Set up" : "Join"} · {app.instance?.site_name ??
      "Gitadel"}</title
  >
</svelte:head>

<main class="grid min-h-screen place-items-center bg-background px-5 py-12">
  <section class="w-full max-w-md rounded-md border bg-card/25 p-6 shadow-sm">
    <div
      class="flex size-10 items-center justify-center rounded-md border bg-background"
    >
      {#if setupRequired}
        <ShieldCheck class="size-5" />
      {:else}
        <UserPlus class="size-5" />
      {/if}
    </div>
    <p
      class="mt-7 text-xs font-medium uppercase tracking-wider text-muted-foreground"
    >
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


    <form
      class="mt-6 grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void createAccount();
      }}
    >
      <Field.Field>
        <Field.Label for="register-username">Username</Field.Label>
        <Input
          id="register-username"
          bind:value={username}
          autocomplete="username"
          minlength={3}
          required
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="register-password">Password</Field.Label>
        <Input
          id="register-password"
          type="password"
          bind:value={password}
          autocomplete="new-password"
          minlength={12}
          required
        />
      </Field.Field>
      <Field.Field data-invalid={validationError !== null}>
        <Field.Label for="register-confirmation">Confirm password</Field.Label>
        <Input
          id="register-confirmation"
          type="password"
          bind:value={confirmation}
          autocomplete="new-password"
          minlength={12}
          aria-invalid={validationError !== null}
          required
        />
        {#if validationError}
          <Field.Error>{validationError}</Field.Error>
        {/if}
      </Field.Field>
      <Button
        class="mt-2"
        type="submit"
        disabled={working || (!setupRequired && !invitationToken)}
      >
        {setupRequired ? "Create administrator" : "Create account"}
      </Button>
    </form>
  </section>
</main>
