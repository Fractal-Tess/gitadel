import { goto } from "$app/navigation";
import { resolve } from "$app/paths";
import { toast } from "svelte-sonner";

import { requestEmpty } from "$lib/api/transport.js";
import type { AuthorizationCacheScope } from "$lib/cache-scope.js";
import {
  clearAccountSettings,
  loadAccountSettings,
  peekAccountSettings,
  updateAccountSettings,
  type AccountSettingsData,
} from "$lib/navigation-cache.js";
import type { AppState } from "$lib/state/app-state.svelte.js";
import { ActionsSettingsState } from "$lib/settings/account/actions-settings-state.svelte.js";
import { CredentialsSettingsState } from "$lib/settings/account/credentials-settings-state.svelte.js";
import { OAuthSettingsState } from "$lib/settings/account/oauth-settings-state.svelte.js";
import { OrganizationSettingsState } from "$lib/settings/account/organization-settings-state.svelte.js";
import {
  PasswordSettingsState,
  ProfileSettingsState,
} from "$lib/settings/account/profile-settings-state.svelte.js";

export type AccountSettingsView =
  "account" | "authentication" | "ssh-keys" | "api-tokens" | "applications";

export class AccountSettingsState {
  readonly profile: ProfileSettingsState;
  readonly password: PasswordSettingsState;
  readonly credentials: CredentialsSettingsState;
  readonly oauth: OAuthSettingsState;
  readonly organization: OrganizationSettingsState;
  readonly actions: ActionsSettingsState;
  loading = $state(true);
  error = $state<string | null>(null);

  #cacheUsername: string;
  #fullSettingsLoaded = false;
  scope: AuthorizationCacheScope;

  constructor(private readonly app: AppState) {
    this.scope = app.authorizationScope;
    const current = (scope: AuthorizationCacheScope) =>
      this.app.authorizationScope === scope;
    this.#cacheUsername = app.authStatus?.user?.username ?? "";
    this.profile = new ProfileSettingsState(app, this.scope, current, () =>
      this.rotateScope(),
    );
    this.password = new PasswordSettingsState(this.scope, current);
    this.credentials = new CredentialsSettingsState(
      { passkeys: [], sshKeys: [], tokens: [] },
      this.scope,
      current,
      (value) => this.#cacheSettings(value),
    );
    this.oauth = new OAuthSettingsState(
      [],
      this.scope,
      current,
      (oauthApplications) => this.#cacheSettings({ oauthApplications }),
    );
    this.organization = new OrganizationSettingsState(
      app,
      [],
      this.scope,
      current,
      (organizations) => this.#cacheSettings({ organizations }),
    );
    this.actions = new ActionsSettingsState(this.scope, current);
    const cached = this.#cacheUsername ? peekAccountSettings(this.scope) : null;
    if (cached) {
      this.#applySettings(cached);
      this.#fullSettingsLoaded = true;
      this.loading = false;
    }
  }

  private rotateScope(): void {
    const scope = this.app.authorizationScope;
    this.scope = scope;
    this.#fullSettingsLoaded = false;
    this.profile.setScope(scope);
    this.password.setScope(scope);
    this.credentials.setScope(scope);
    this.oauth.setScope(scope);
    this.organization.setScope(scope);
    this.actions.setScope(scope);
  }
  syncScope(): void {
    if (this.app.authorizationScope !== this.scope) this.rotateScope();
  }

  async initialize(_view: AccountSettingsView): Promise<void> {
    const username = this.app.authStatus?.user?.username ?? "";
    if (!username) {
      this.loading = false;
      return;
    }
    if (this.#fullSettingsLoaded) {
      this.loading = false;
      return;
    }
    const scope = this.scope;
    this.loading = !peekAccountSettings(scope);
    this.profile.username = username;
    await this.run(async () => {
      const settings = await loadAccountSettings(scope);
      if (this.app.authorizationScope !== scope) return;
      this.#applySettings(settings);
      this.#fullSettingsLoaded = true;
    });
    this.loading = false;
  }

  async logout(): Promise<void> {
    await this.run(async () => {
      await requestEmpty("/api/v1/auth/logout", { method: "POST" });
      this.app.advanceAuthorizationScope();
      await this.app.refreshAuth();
      await goto(resolve("/login"));
    });
  }

  #applySettings(settings: AccountSettingsData): void {
    this.credentials.passkeys = settings.passkeys;
    this.credentials.sshKeys = settings.sshKeys;
    this.credentials.tokens = settings.tokens;
    this.oauth.oauthApplications = settings.oauthApplications;
    this.organization.organizations = settings.organizations;
  }

  #cacheSettings(changes: Partial<AccountSettingsData>): void {
    const username = this.app.authStatus?.user?.username;
    if (!username) {
      if (this.#cacheUsername) clearAccountSettings(this.scope);
      return;
    }
    if (this.#cacheUsername && this.#cacheUsername !== username)
      clearAccountSettings(this.scope);
    this.#cacheUsername = username;
    const current = peekAccountSettings(this.scope);
    if (!current) return;
    updateAccountSettings(this.scope, {
      passkeys: changes.passkeys ?? current.passkeys,
      sshKeys: changes.sshKeys ?? current.sshKeys,
      tokens: changes.tokens ?? current.tokens,
      oauthApplications: changes.oauthApplications ?? current.oauthApplications,
      organizations: changes.organizations ?? current.organizations,
    });
  }

  private async run(task: () => Promise<void>): Promise<void> {
    this.error = null;
    try {
      await task();
    } catch (caught) {
      this.error =
        caught instanceof Error ? caught.message : "The request failed.";
      toast.error(this.error);
    }
  }
}
