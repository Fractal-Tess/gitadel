import { goto } from "$app/navigation";
import { resolve } from "$app/paths";
import { toast } from "svelte-sonner";

import type {
  ApiToken,
  OauthApplication,
  PasskeySummary,
  SshKey,
} from "$lib/api/account.js";
import { requestEmpty } from "$lib/api/transport.js";
import type { AuthorizationCacheScope } from "$lib/cache-scope.js";
import {
  invalidateAdminActivity,
  loadApiTokens,
  loadOauthApplications,
  loadPasskeys,
  loadSshKeys,
  peekApiTokens,
  peekOauthApplications,
  peekPasskeys,
  peekSshKeys,
  setApiTokens,
  setOauthApplications,
  setPasskeys,
  setSshKeys,
  type AccountSettingsView,
} from "$lib/settings/settings-data-cache.js";
import type { AppState } from "$lib/state/app-state.svelte.js";
import { ActionsSettingsState } from "$lib/settings/account/actions-settings-state.svelte.js";
import { CredentialsSettingsState } from "$lib/settings/account/credentials-settings-state.svelte.js";
import { OAuthSettingsState } from "$lib/settings/account/oauth-settings-state.svelte.js";
import { OrganizationSettingsState } from "$lib/settings/account/organization-settings-state.svelte.js";
import {
  PasswordSettingsState,
  ProfileSettingsState,
} from "$lib/settings/account/profile-settings-state.svelte.js";
type AccountDataset =
  PasskeySummary[] | SshKey[] | ApiToken[] | OauthApplication[];

export class AccountSettingsState {
  readonly profile: ProfileSettingsState;
  readonly password: PasswordSettingsState;
  readonly credentials: CredentialsSettingsState;
  readonly oauth: OAuthSettingsState;
  readonly organization: OrganizationSettingsState;
  readonly actions: ActionsSettingsState;
  loading = $state(false);
  error = $state<string | null>(null);

  scope: AuthorizationCacheScope;
  #loadSequence = 0;
  readonly #loaded = new Set<AccountSettingsView>();

  constructor(private readonly app: AppState) {
    this.scope = app.authorizationScope;
    const current = (scope: AuthorizationCacheScope) =>
      this.app.authorizationScope === scope;
    this.profile = new ProfileSettingsState(app, this.scope, current, () =>
      this.rotateScope(),
    );
    this.password = new PasswordSettingsState(this.scope, current);
    this.credentials = new CredentialsSettingsState(
      {
        passkeys: peekPasskeys(this.scope) ?? [],
        sshKeys: peekSshKeys(this.scope) ?? [],
        tokens: peekApiTokens(this.scope) ?? [],
      },
      this.scope,
      current,
      (change) => {
        if (change.dataset === "passkeys") {
          setPasskeys(this.scope, change.value);
        } else if (change.dataset === "ssh-keys") {
          setSshKeys(this.scope, change.value);
        } else {
          setApiTokens(this.scope, change.value);
        }
        invalidateAdminActivity(this.scope);
      },
    );
    this.oauth = new OAuthSettingsState(
      peekOauthApplications(this.scope) ?? [],
      this.scope,
      current,
      (applications) => {
        setOauthApplications(this.scope, applications);
        invalidateAdminActivity(this.scope);
      },
    );
    this.organization = new OrganizationSettingsState(
      app,
      app.organizations,
      this.scope,
      current,
      () => undefined,
    );
    this.actions = new ActionsSettingsState(this.scope, current);
    if (peekPasskeys(this.scope)) this.#loaded.add("authentication");
    if (peekSshKeys(this.scope)) this.#loaded.add("ssh-keys");
    if (peekApiTokens(this.scope)) this.#loaded.add("api-tokens");
    if (peekOauthApplications(this.scope))
      this.#loaded.add("oauth-applications");
  }

  syncScope(): void {
    if (this.app.authorizationScope !== this.scope) this.rotateScope();
  }

  async initialize(view: AccountSettingsView): Promise<void> {
    const username = this.app.authStatus?.user?.username ?? "";
    this.profile.username = username;
    if (!username || view === "profile") {
      this.loading = false;
      this.error = null;
      return;
    }

    const cached = this.#peek(view);
    if (cached) {
      this.#apply(view, cached);
      this.#loaded.add(view);
      this.loading = false;
      this.error = null;
      return;
    }
    if (this.#loaded.has(view)) {
      this.loading = false;
      return;
    }

    const sequence = ++this.#loadSequence;
    const scope = this.scope;
    this.loading = true;
    this.error = null;
    try {
      const value = await this.#load(view, scope);
      if (
        sequence !== this.#loadSequence ||
        this.app.authorizationScope !== scope
      )
        return;
      this.#apply(view, value);
      this.#loaded.add(view);
    } catch (caught) {
      if (
        sequence === this.#loadSequence &&
        this.app.authorizationScope === scope
      ) {
        this.error =
          caught instanceof Error ? caught.message : "The request failed.";
      }
    } finally {
      if (
        sequence === this.#loadSequence &&
        this.app.authorizationScope === scope
      )
        this.loading = false;
    }
  }

  async logout(): Promise<void> {
    try {
      await requestEmpty("/api/v1/auth/logout", { method: "POST" });
      this.app.advanceAuthorizationScope();
      await this.app.refreshAuth();
      await goto(resolve("/login"));
    } catch (caught) {
      this.error =
        caught instanceof Error ? caught.message : "The request failed.";
      toast.error(this.error);
    }
  }

  rotateScope(): void {
    this.scope = this.app.authorizationScope;
    this.#loadSequence += 1;
    this.#loaded.clear();
    this.loading = false;
    this.error = null;
    this.profile.setScope(this.scope);
    this.password.setScope(this.scope);
    this.credentials.setScope(this.scope);
    this.credentials.passkeys = [];
    this.credentials.sshKeys = [];
    this.credentials.tokens = [];
    this.credentials.createdToken = null;
    this.oauth.setScope(this.scope);
    this.oauth.oauthApplications = [];
    this.oauth.createdOauthClientId = null;
    this.oauth.createdOauthClientSecret = null;
    this.organization.setScope(this.scope);
    this.organization.organizations = this.app.organizations;
    this.actions.setScope(this.scope);
  }

  #peek(view: Exclude<AccountSettingsView, "profile">): AccountDataset | null {
    return view === "authentication"
      ? peekPasskeys(this.scope)
      : view === "ssh-keys"
        ? peekSshKeys(this.scope)
        : view === "api-tokens"
          ? peekApiTokens(this.scope)
          : peekOauthApplications(this.scope);
  }

  #load(
    view: Exclude<AccountSettingsView, "profile">,
    scope: AuthorizationCacheScope,
  ): Promise<AccountDataset> {
    return view === "authentication"
      ? loadPasskeys(scope)
      : view === "ssh-keys"
        ? loadSshKeys(scope)
        : view === "api-tokens"
          ? loadApiTokens(scope)
          : loadOauthApplications(scope);
  }

  #apply(
    view: Exclude<AccountSettingsView, "profile">,
    value: AccountDataset,
  ): void {
    if (view === "authentication") {
      this.credentials.passkeys = value as PasskeySummary[];
    } else if (view === "ssh-keys") {
      this.credentials.sshKeys = value as SshKey[];
    } else if (view === "api-tokens") {
      this.credentials.tokens = value as ApiToken[];
    } else {
      this.oauth.oauthApplications = value as OauthApplication[];
    }
  }
}
