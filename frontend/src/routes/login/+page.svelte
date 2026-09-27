<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { page } from "$app/state";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import LogIn from "@lucide/svelte/icons/log-in";
  import { toast } from "svelte-sonner";

  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { ApiFailure, jsonBody, requestJson } from "$lib/api/transport.js";
  import {
    authResponseSchema,
    passwordLoginResponseSchema,
    webauthnRequestSchema,
  } from "$lib/api/auth.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import { getCredential, requestOptions } from "$lib/webauthn.js";

  type LoginMethod = "password" | "passkey" | `sso:${string}`;

  const LAST_LOGIN_METHOD_KEY = "gitadel:last-login-method";
  const app = useAppState();
  const authentication = $derived(app.authStatus?.authentication);
  const hasAlternativeLogin = $derived(
    authentication?.passkey_enabled ||
      (authentication?.providers.length ?? 0) > 0,
  );
  let username = $state("");
  let password = $state("");
  let working = $state(false);
  // Set once the password was accepted for an account with two-factor
  // authentication; the next request completes the sign-in.
  let twoFactorChallengeId = $state<string | null>(null);
  let twoFactorCode = $state("");
  let lastLoginMethod = $state<LoginMethod | null>(readLastLoginMethod());

  function readLastLoginMethod(): LoginMethod | null {
    try {
      const stored = globalThis.localStorage?.getItem(LAST_LOGIN_METHOD_KEY);
      return stored === "password" ||
        stored === "passkey" ||
        stored?.startsWith("sso:")
        ? (stored as LoginMethod)
        : null;
    } catch {
      return null;
    }
  }

  function recordLastLoginMethod(method: LoginMethod): void {
    lastLoginMethod = method;
    try {
      globalThis.localStorage?.setItem(LAST_LOGIN_METHOD_KEY, method);
    } catch {
      // Login must still succeed when browser storage is unavailable.
    }
  }

  async function runLogin(
    task: () => Promise<LoginMethod | null>,
  ): Promise<void> {
    working = true;
    try {
      const method = await task();
      if (!method) return;
      recordLastLoginMethod(method);
      await app.refreshAuth();
      const returnTo = page.url.searchParams.get("returnTo");
      if (
        returnTo?.startsWith("/login/oauth/authorize?") ||
        returnTo?.startsWith("/settings")
      ) {
        window.location.assign(returnTo);
      } else {
        await goto(resolve("/"));
      }
    } catch (caught) {
      toast.error(
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Sign in failed.",
      );
    } finally {
      working = false;
    }
  }

  async function loginWithPassword(): Promise<void> {
    await runLogin(async () => {
      const response = await requestJson(
        "/api/v1/auth/login",
        passwordLoginResponseSchema,
        {
          method: "POST",
          body: jsonBody({ username, password }),
        },
      );
      password = "";
      if ("two_factor_required" in response) {
        twoFactorChallengeId = response.challenge_id;
        twoFactorCode = "";
        return null;
      }
      return "password";
    });
  }

  async function submitTwoFactorCode(): Promise<void> {
    const challengeId = twoFactorChallengeId;
    if (!challengeId) return;
    await runLogin(async () => {
      try {
        await requestJson("/api/v1/auth/login/two-factor", authResponseSchema, {
          method: "POST",
          body: jsonBody({ challenge_id: challengeId, code: twoFactorCode }),
        });
      } catch (caught) {
        twoFactorCode = "";
        // An expired or exhausted attempt cannot be retried; start over.
        if (caught instanceof ApiFailure && caught.code === "bad_request") {
          twoFactorChallengeId = null;
        }
        throw caught;
      }
      twoFactorChallengeId = null;
      return "password";
    });
  }

  async function loginWithPasskey(): Promise<void> {
    await runLogin(async () => {
      console.info("[passkey] requesting discoverable login challenge", {
        secureContext: globalThis.isSecureContext,
        publicKeyCredentialAvailable:
          typeof PublicKeyCredential !== "undefined",
      });
      const challenge = await requestJson(
        "/api/v1/auth/passkeys/login/start",
        webauthnRequestSchema,
        { method: "POST" },
      );
      const options = requestOptions(challenge.options.publicKey);
      console.info("[passkey] discoverable login challenge received", {
        allowedCredentialCount: options.allowCredentials?.length ?? 0,
        rpId: options.rpId,
      });
      const credential = await getCredential(options);
      await requestJson(
        "/api/v1/auth/passkeys/login/finish",
        authResponseSchema,
        {
          method: "POST",
          body: jsonBody({ challenge_id: challenge.challenge_id, credential }),
        },
      );
      console.info("[passkey] discoverable login accepted");
      return "passkey";
    });
  }

  function ssoHref(providerId: string): string {
    const returnTo = page.url.searchParams.get("returnTo") ?? "/";
    const query = new URLSearchParams({ returnTo });
    return `/api/v1/auth/oidc/${encodeURIComponent(providerId)}/start?${query}`;
  }
</script>

<svelte:head>
  <title>Sign in · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<main class="grid min-h-screen place-items-center bg-background px-5 py-12">
  <section class="w-full max-w-md rounded-md border bg-card/25 p-6 shadow-sm">
    <a class="text-sm font-bold tracking-[-0.035em]" href={resolve("/")}
      >{app.instance?.site_name ?? "GITADEL"}</a
    >
    <p
      class="mt-8 text-xs font-medium uppercase tracking-wider text-muted-foreground"
    >
      Account access
    </p>
    {#if twoFactorChallengeId}
      <h1 class="mt-2 text-2xl font-semibold">Two-factor authentication</h1>
      <p class="mt-2 text-sm text-muted-foreground">
        Enter the 6-digit code from your authenticator app, or one of your
        recovery codes.
      </p>
      <form
        class="mt-6 grid gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void submitTwoFactorCode();
        }}
      >
        <Field.Field>
          <Field.Label for="login-two-factor-code">Authentication code</Field.Label>
          <Input
            id="login-two-factor-code"
            bind:value={twoFactorCode}
            autocomplete="one-time-code"
            autocapitalize="off"
            spellcheck={false}
            maxlength={32}
            required
          />
        </Field.Field>
        <Button type="submit" disabled={working || !twoFactorCode.trim()}>
          Verify
        </Button>
        <Button
          type="button"
          variant="ghost"
          disabled={working}
          onclick={() => {
            twoFactorChallengeId = null;
            twoFactorCode = "";
          }}
        >
          Start over
        </Button>
      </form>
    {:else}
      <h1 class="mt-2 text-2xl font-semibold">Sign in</h1>
      <p class="mt-2 text-sm text-muted-foreground">
        Use your account to access private repositories and settings.
      </p>

      {#if authentication?.password_enabled}
        <form
          class="mt-6 grid gap-4"
          onsubmit={(event) => {
            event.preventDefault();
            void loginWithPassword();
          }}
        >
          <Field.Field>
            <Field.Label for="login-username">Username</Field.Label>
            <Input
              id="login-username"
              bind:value={username}
              autocomplete="username"
              required
            />
          </Field.Field>
          <Field.Field>
            <Field.Label for="login-password">Password</Field.Label>
            <Input
              id="login-password"
              type="password"
              bind:value={password}
              autocomplete="current-password"
              required
            />
          </Field.Field>
          <Button class="relative" type="submit" disabled={working}>
            Sign in
            {#if lastLoginMethod === "password"}
              <Badge
                variant="secondary"
                class="absolute -right-2 -bottom-2 z-10 h-4 border border-border bg-background px-1.5 text-[9px] leading-none text-foreground shadow-sm"
              >
                Last used
              </Badge>
            {/if}
          </Button>
        </form>
      {/if}

      {#if authentication?.password_enabled && hasAlternativeLogin}
        <div class="my-5 flex items-center gap-3 text-xs text-muted-foreground">
          <span class="h-px flex-1 bg-border"></span>or<span
            class="h-px flex-1 bg-border"
          ></span>
        </div>
      {/if}

      <div class:mt-6={!authentication?.password_enabled} class="grid gap-3">
        {#if authentication?.passkey_enabled}
          <div>
            <Button
              class="relative w-full gap-2"
              variant="outline"
              disabled={working}
              aria-describedby="passkey-login-help"
              onclick={() => void loginWithPasskey()}
            >
              <KeyRound data-icon="inline-start" />Use a passkey
              {#if lastLoginMethod === "passkey"}
                <Badge
                  class="absolute -right-2 -bottom-2 z-10 h-4 border border-background bg-primary px-1.5 text-[9px] leading-none text-primary-foreground shadow-sm"
                >
                  Last used
                </Badge>
              {/if}
            </Button>
            <p
              id="passkey-login-help"
              class="mt-3 text-center text-xs text-muted-foreground"
            >
              Choose a passkey saved for this site. No username required.
            </p>
          </div>
        {/if}

        {#each authentication?.providers ?? [] as provider (provider.id)}
          <Button
            class="relative w-full gap-2"
            variant="outline"
            href={ssoHref(provider.id)}
            disabled={working}
          >
            <LogIn data-icon="inline-start" />Continue with {provider.name}
            {#if lastLoginMethod === `sso:${provider.id}`}
              <Badge
                class="absolute -right-2 -bottom-2 z-10 h-4 border border-background bg-primary px-1.5 text-[9px] leading-none text-primary-foreground shadow-sm"
              >
                Last used
              </Badge>
            {/if}
          </Button>
        {/each}
      </div>
    {/if}
  </section>
</main>
