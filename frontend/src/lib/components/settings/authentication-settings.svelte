<script lang="ts">
  import { onMount } from "svelte";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import LockKeyhole from "@lucide/svelte/icons/lock-keyhole";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import { toast } from "svelte-sonner";

  import {
    adminOidcProviderSchema,
    authenticationConfigurationSchema,
    type AdminOidcProvider,
    type AuthenticationConfiguration,
  } from "$lib/api/sso.js";
  import {
    ApiFailure,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import {
    loadAuthenticationConfiguration,
    loadOidcProviders,
    setAuthenticationConfiguration,
    setOidcProviders,
  } from "$lib/settings/settings-data-cache.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  let configuration = $state<AuthenticationConfiguration | null>(null);
  let providers = $state.raw<AdminOidcProvider[]>([]);
  let loading = $state(true);
  let working = $state(false);
  let error = $state<string | null>(null);
  let editorOpen = $state(false);
  let editing = $state<AdminOidcProvider | null>(null);
  let removeOpen = $state(false);
  let pendingRemove = $state<AdminOidcProvider | null>(null);

  let providerName = $state("");
  let issuerUrl = $state("");
  let clientId = $state("");
  let clientSecret = $state("");
  let providerEnabled = $state(true);
  let autoProvision = $state(true);

  onMount(() => {
    void load();
  });

  function message(caught: unknown, fallback: string): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  async function load(
    scope = app.authorizationScope,
  ): Promise<void> {
    loading = true;
    error = null;
    try {
      [configuration, providers] = await Promise.all([
        loadAuthenticationConfiguration(scope),
        loadOidcProviders(scope),
      ]);
    } catch (caught) {
      error = message(caught, "Could not load authentication settings.");
    } finally {
      loading = false;
    }
  }

  async function saveMethods(): Promise<void> {
    if (!configuration) return;
    working = true;
    try {
      configuration = await requestJson(
        "/api/v1/admin/authentication",
        authenticationConfigurationSchema,
        {
          method: "PUT",
          body: jsonBody({
            password_enabled: configuration.password_enabled,
            passkey_enabled: configuration.passkey_enabled,
          }),
        },
      );
      setAuthenticationConfiguration(app.authorizationScope, configuration);
      await app.refreshAuth();
      toast.success("Authentication methods updated.");
    } catch (caught) {
      toast.error(message(caught, "Could not update authentication methods."));
      await load();
    } finally {
      working = false;
    }
  }

  function openCreate(): void {
    editing = null;
    providerName = "";
    issuerUrl = "";
    clientId = "";
    clientSecret = "";
    providerEnabled = true;
    autoProvision = true;
    editorOpen = true;
  }

  function openEdit(provider: AdminOidcProvider): void {
    editing = provider;
    providerName = provider.name;
    issuerUrl = provider.issuer_url;
    clientId = provider.client_id;
    clientSecret = "";
    providerEnabled = provider.enabled;
    autoProvision = provider.auto_provision;
    editorOpen = true;
  }

  async function saveProvider(): Promise<void> {
    working = true;
    try {
      const path = editing
        ? `/api/v1/admin/authentication/providers/${editing.id}`
        : "/api/v1/admin/authentication/providers";
      const saved = await requestJson(path, adminOidcProviderSchema, {
        method: editing ? "PUT" : "POST",
        body: jsonBody({
          name: providerName,
          issuer_url: issuerUrl,
          client_id: clientId,
          client_secret: clientSecret || null,
          enabled: providerEnabled,
          auto_provision: autoProvision,
        }),
      });
      providers = editing
        ? providers.map((provider) =>
            provider.id === saved.id ? saved : provider,
          )
        : [...providers, saved];
      setOidcProviders(app.authorizationScope, providers);
      await app.refreshAuth();
      editorOpen = false;
      toast.success(
        editing ? "Identity provider updated." : "Identity provider added.",
      );
    } catch (caught) {
      toast.error(message(caught, "Could not save the identity provider."));
    } finally {
      working = false;
    }
  }

  async function setProviderEnabled(
    provider: AdminOidcProvider,
    enabled: boolean,
  ): Promise<void> {
    working = true;
    try {
      const saved = await requestJson(
        `/api/v1/admin/authentication/providers/${provider.id}`,
        adminOidcProviderSchema,
        {
          method: "PUT",
          body: jsonBody({
            name: provider.name,
            issuer_url: provider.issuer_url,
            client_id: provider.client_id,
            client_secret: null,
            enabled,
            auto_provision: provider.auto_provision,
          }),
        },
      );
      providers = providers.map((candidate) =>
        candidate.id === saved.id ? saved : candidate,
      );
      setOidcProviders(app.authorizationScope, providers);
      await app.refreshAuth();
      toast.success(`${provider.name} ${enabled ? "enabled" : "disabled"}.`);
    } catch (caught) {
      toast.error(
        message(
          caught,
          `Could not ${enabled ? "enable" : "disable"} ${provider.name}.`,
        ),
      );
    } finally {
      working = false;
    }
  }

  async function removeProvider(): Promise<void> {
    if (!pendingRemove) return;
    working = true;
    try {
      await requestEmpty(
        `/api/v1/admin/authentication/providers/${pendingRemove.id}`,
        { method: "DELETE" },
      );
      providers = providers.filter(
        (provider) => provider.id !== pendingRemove?.id,
      );
      setOidcProviders(app.authorizationScope, providers);
      await app.refreshAuth();
      removeOpen = false;
      pendingRemove = null;
      toast.success("Identity provider removed.");
    } catch (caught) {
      toast.error(message(caught, "Could not remove the identity provider."));
    } finally {
      working = false;
    }
  }
</script>

{#if error}
  <Alert.Root variant="destructive">
    <Alert.Title>Authentication settings unavailable</Alert.Title>
    <Alert.Description>{error}</Alert.Description>
  </Alert.Root>
{:else if loading || !configuration}
  <p
    class="flex items-center justify-center gap-2 py-16 text-sm text-muted-foreground"
  >
    <Spinner class="size-4" /> Loading authentication settings…
  </p>
{:else}
  <div class="grid gap-6">
    <Card.Root>
      <Card.Header class="border-b">
        <div class="flex items-center gap-3">
          <ShieldCheck class="size-4 text-muted-foreground" />
          <div>
            <Card.Title>Login methods</Card.Title>
            <Card.Description>
              Choose which credentials Gitadel accepts on the sign-in page.
            </Card.Description>
          </div>
        </div>
      </Card.Header>
      <Card.Content class="divide-y p-0">
        <label class="flex items-center justify-between gap-5 px-6 py-5">
          <span>
            <span class="flex items-center gap-2 text-sm font-medium">
              <LockKeyhole class="size-4 text-muted-foreground" /> Username and password
            </span>
            <span class="mt-1 block text-xs text-muted-foreground">
              Local credentials managed by each Gitadel account.
            </span>
          </span>
          <Switch
            bind:checked={configuration.password_enabled}
            disabled={working}
          />
        </label>
        <label class="flex items-center justify-between gap-5 px-6 py-5">
          <span>
            <span class="flex items-center gap-2 text-sm font-medium">
              <KeyRound class="size-4 text-muted-foreground" /> Passkeys
            </span>
            <span class="mt-1 block text-xs text-muted-foreground">
              WebAuthn login and passkey registration.
            </span>
          </span>
          <Switch
            bind:checked={configuration.passkey_enabled}
            disabled={working}
          />
        </label>
      </Card.Content>
      <Card.Footer class="justify-end border-t">
        <Button disabled={working} onclick={() => void saveMethods()}>
          {#if working}<Spinner data-icon="inline-start" />{/if}
          Save login methods
        </Button>
      </Card.Footer>
    </Card.Root>

    <section class="grid gap-4">
      <div>
        <h2 class="text-sm font-semibold">OpenID Connect providers</h2>
        <p class="mt-0.5 text-xs text-muted-foreground">
          Each enabled provider gets its own button on the sign-in page.
        </p>
      </div>
      <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        <IntegrationAddCard
          title="Add identity provider"
          description="Connect an OpenID Connect provider for external account access."
          onclick={openCreate}
          compact
        />
        {#each providers as provider (provider.id)}
          <IntegrationConnectionCard
            name={provider.name}
            provider="oidc"
            providerName="OpenID Connect"
            subtitle={provider.issuer_url}
            description={provider.auto_provision
              ? "Creates a Gitadel account after the first successful login."
              : "Only existing linked Gitadel accounts can sign in."}
            enabled={provider.enabled}
            statusLabel={provider.enabled ? "Enabled" : "Disabled"}
            statusHealthy={provider.enabled}
            detailLabel={provider.has_client_secret
              ? "Confidential client"
              : "Public client"}
            busy={working}
            compact
            onenabledchange={(enabled) =>
              void setProviderEnabled(provider, enabled)}
            onconfigure={() => openEdit(provider)}
            onremove={() => {
              pendingRemove = provider;
              removeOpen = true;
            }}
          >
            {#snippet icon()}
              <ShieldCheck class="size-6 text-primary" />
            {/snippet}
          </IntegrationConnectionCard>
        {/each}
      </div>
    </section>
  </div>
{/if}

<Dialog.Root bind:open={editorOpen}>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title
        >{editing ? "Configure provider" : "Add provider"}</Dialog.Title
      >
      <Dialog.Description>
        Gitadel validates the issuer discovery document before saving.
      </Dialog.Description>
    </Dialog.Header>
    <form
      class="grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void saveProvider();
      }}
    >
      <Field.Field>
        <Field.Label for="oidc-name">Name</Field.Label>
        <Input
          id="oidc-name"
          bind:value={providerName}
          maxlength={80}
          placeholder="Company identity"
          required
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="oidc-issuer">Issuer URL</Field.Label>
        <Input
          id="oidc-issuer"
          bind:value={issuerUrl}
          type="url"
          placeholder="https://id.example.com/realms/company"
          required
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="oidc-client-id">Client ID</Field.Label>
        <Input
          id="oidc-client-id"
          bind:value={clientId}
          autocomplete="off"
          required
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="oidc-client-secret">Client secret</Field.Label>
        <Input
          id="oidc-client-secret"
          bind:value={clientSecret}
          type="password"
          autocomplete="new-password"
          placeholder={editing?.has_client_secret
            ? "Leave blank to keep existing secret"
            : "Optional for public clients"}
        />
      </Field.Field>
      {#if editing}
        <div class="rounded-md border bg-muted/20 p-3 text-xs">
          <p class="font-medium">Callback URL</p>
          <code class="mt-1 block break-all text-muted-foreground"
            >{editing.callback_url}</code
          >
        </div>
      {/if}
      <label
        class="flex items-center justify-between gap-4 rounded-md border p-3 text-sm"
      >
        <span>
          <span class="font-medium">Enabled</span>
          <span class="mt-0.5 block text-xs text-muted-foreground"
            >Show this provider on the sign-in page.</span
          >
        </span>
        <Switch bind:checked={providerEnabled} disabled={working} />
      </label>
      <label
        class="flex items-center justify-between gap-4 rounded-md border p-3 text-sm"
      >
        <span>
          <span class="font-medium">Create accounts automatically</span>
          <span class="mt-0.5 block text-xs text-muted-foreground"
            >Create a Gitadel account on first successful login.</span
          >
        </span>
        <Switch bind:checked={autoProvision} disabled={working} />
      </label>
      <Dialog.Footer>
        <Button
          type="button"
          variant="ghost"
          disabled={working}
          onclick={() => (editorOpen = false)}>Cancel</Button
        >
        <Button type="submit" disabled={working}>
          {#if working}<Spinner data-icon="inline-start" />{/if}
          Save provider
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={removeOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Remove {pendingRemove?.name}?</AlertDialog.Title>
      <AlertDialog.Description>
        Existing Gitadel accounts remain, but identities linked through this
        provider can no longer sign in.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={working}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        disabled={working}
        onclick={() => void removeProvider()}
      >
        Remove provider
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
