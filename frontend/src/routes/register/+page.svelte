<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { page } from "$app/state";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import UserPlus from "@lucide/svelte/icons/user-plus";
  import { toast } from "svelte-sonner";

  import AuthShell, {
    authButtonClass,
    authInputClass,
  } from "$lib/components/app/auth-shell.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { ApiFailure, jsonBody, requestJson } from "$lib/api/transport.js";
  import { authResponseSchema } from "$lib/api/auth.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const setupRequired = $derived(app.authStatus?.setup_required ?? false);
  const invitationToken = $derived(page.url.searchParams.get("token") ?? "");
  const setupToken = $derived(page.url.searchParams.get("setup") ?? "");
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
      await requestJson(
        setupRequired ? "/api/v1/setup" : "/api/v1/register",
        authResponseSchema,
        {
          method: "POST",
          body: jsonBody({
            token: setupRequired ? setupToken : invitationToken,
            username,
            password,
          }),
        },
      );
      await app.refreshAuth();
      await goto(resolve("/"));
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

<AuthShell>
  <p
    class="flex items-center gap-2 text-xs font-medium tracking-wider text-muted-foreground uppercase"
  >
    {#if setupRequired}
      <ShieldCheck class="size-4 text-orange-600 dark:text-orange-400" />
    {:else}
      <UserPlus class="size-4 text-orange-600 dark:text-orange-400" />
    {/if}
    {setupRequired ? "Initial setup" : "Invitation"}
  </p>
  <h1 class="mt-3 text-3xl font-semibold tracking-[-0.03em]">
    {setupRequired ? "Create the administrator" : "Create your account"}
  </h1>
  <p class="mt-2 text-sm leading-6 text-muted-foreground">
    {#if !setupRequired}
      This private invitation grants access to this Gitadel instance.
    {:else if setupToken}
      This one-time setup link creates the first administrator.
    {:else}
      Open the one-time setup link that Gitadel printed in its log at startup.
      With Docker Compose, run <code>docker compose logs gitadel</code>.
    {/if}
  </p>

  {#if setupRequired && !setupToken}
    <pre
      class="mt-8 overflow-x-auto rounded-xl border bg-muted/40 p-4 text-xs leading-5"><code
        >docker compose logs gitadel | grep register?setup=</code
      ></pre>
  {:else}
    <form
      class="mt-8 grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void createAccount();
      }}
    >
      <Field.Field>
        <Field.Label for="register-username">Username</Field.Label>
        <Input
          id="register-username"
          class={authInputClass}
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
          class={authInputClass}
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
          class={authInputClass}
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
        class={[authButtonClass, "mt-2 hover:bg-primary/90"]}
        type="submit"
        disabled={working || !(setupRequired ? setupToken : invitationToken)}
      >
        {setupRequired ? "Create administrator" : "Create account"}
      </Button>
    </form>
  {/if}
</AuthShell>
