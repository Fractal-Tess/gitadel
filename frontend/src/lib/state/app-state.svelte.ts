import { getContext, setContext } from "svelte";
import { setMode } from "mode-watcher";

import {
  authResponseSchema,
  authStatusSchema,
  type AuthStatus,
  type ThemePreference,
} from "$lib/api/auth.js";
import {
  instanceSettingsSchema,
  type InstanceSettings,
} from "$lib/api/instance.js";
import { jsonBody, requestJson } from "$lib/api/transport.js";
import type { AuthorizationCacheScope } from "$lib/cache-scope.js";
import type { Organization } from "$lib/api/organizations.js";
import {
  clearNavigationCaches,
  loadOrganizations as loadOrganizationMemberships,
  refreshOrganizations as refreshOrganizationMemberships,
  updateOrganizations,
} from "$lib/navigation-cache.js";
import { clearNamespaceCaches } from "$lib/namespace-preload.js";
import { clearRepositoryCaches } from "$lib/repository/repository-data-cache.js";
import { clearRepositoryPreloads } from "$lib/repository/repository-preload.js";
import { clearBackupSettingsCache } from "$lib/settings/backup-settings-cache.js";
import { clearSettingsDataCache } from "$lib/settings/settings-data-cache.js";

const APP_STATE = Symbol("gitadel-app-state");
function organizationPermissionsKey(organizations: Organization[]): string {
  return organizations
    .map((organization) => `${organization.id}:${organization.role}`)
    .sort()
    .join("|");
}

export class AppState {
  authStatus = $state.raw<AuthStatus | null>(null);
  authorizationScope = $state.raw<AuthorizationCacheScope>({
    viewer: null,
    epoch: 0,
  });
  instance = $state.raw<InstanceSettings | null>(null);
  organizations = $state.raw<Organization[]>([]);
  loading = $state(true);
  error = $state<string | null>(null);

  #authFingerprint = "";
  #initializedAt = 0;
  #organizationsFor: string | null = null;
  #initializing: Promise<AuthStatus> | null = null;

  async initialize(): Promise<AuthStatus> {
    if (
      this.authStatus &&
      this.instance &&
      Date.now() - this.#initializedAt < 30_000
    ) {
      return this.authStatus;
    }
    if (this.#initializing) return this.#initializing;

    this.#initializing = this.#loadInitialState();
    try {
      return await this.#initializing;
    } finally {
      this.#initializing = null;
    }
  }

  async #loadInitialState(): Promise<AuthStatus> {
    this.loading = true;
    this.error = null;
    try {
      const [status, instance] = await Promise.all([
        requestJson("/api/v1/auth/status", authStatusSchema),
        requestJson("/api/v1/instance", instanceSettingsSchema),
      ]);
      this.#setAuthStatus(status);
      this.#applyTheme(status);
      this.instance = instance;
      this.#loadOrganizationsFor(status);
      this.#initializedAt = Date.now();
      return status;
    } catch (caught) {
      this.error =
        caught instanceof Error ? caught.message : "Could not load Gitadel.";
      throw caught;
    } finally {
      this.loading = false;
    }
  }

  #applyTheme(status: AuthStatus): void {
    setMode(status.user?.theme_preference ?? "system");
  }

  async updateThemePreference(preference: ThemePreference): Promise<void> {
    const status = this.authStatus;
    const user = status?.user;
    if (!status?.authenticated || !user) return;

    const previous = user.theme_preference;
    setMode(preference);
    try {
      const response = await requestJson(
        "/api/v1/me/theme-preference",
        authResponseSchema,
        {
          method: "PUT",
          body: jsonBody({ theme_preference: preference }),
        },
      );
      this.#setAuthStatus({ ...status, user: response.user });
    } catch (error) {
      setMode(previous);
      throw error;
    }
  }

  async refreshInstance(): Promise<InstanceSettings> {
    const instance = await requestJson(
      "/api/v1/instance",
      instanceSettingsSchema,
    );
    this.instance = instance;
    return instance;
  }

  async refreshOrganizations(): Promise<Organization[]> {
    const username = this.authStatus?.user?.username;
    if (!username) {
      this.#organizationsFor = null;
      this.organizations = [];
      return [];
    }
    const scope = this.authorizationScope;
    const organizations = await refreshOrganizationMemberships(scope);
    if (
      this.authStatus?.user?.username === username &&
      this.authorizationScope === scope
    ) {
      if (
        organizationPermissionsKey(this.organizations) !==
        organizationPermissionsKey(organizations)
      ) {
        this.advanceAuthorizationScope();
        this.#organizationsFor = username;
        this.organizations = organizations;
        updateOrganizations(this.authorizationScope, organizations);
        return organizations;
      }
      this.#organizationsFor = username;
      this.organizations = organizations;
      updateOrganizations(scope, organizations);
      return organizations;
    }
    return [];
  }

  addOrganization(organization: Organization): void {
    const username = this.authStatus?.user?.username;
    if (!username) return;
    this.organizations = [
      ...this.organizations.filter((item) => item.id !== organization.id),
      organization,
    ].sort((left, right) => left.slug.localeCompare(right.slug));
    updateOrganizations(this.authorizationScope, this.organizations);
  }

  #loadOrganizationsFor(status: AuthStatus): void {
    const username = status.user?.username ?? null;
    if (this.#organizationsFor !== username) {
      this.#organizationsFor = username;
      this.organizations = [];
    }
    if (username) {
      void this.refreshOrganizations().catch(() => undefined);
    }
  }

  async refreshAuth(): Promise<AuthStatus> {
    this.loading = true;
    this.error = null;
    try {
      const status = await requestJson("/api/v1/auth/status", authStatusSchema);
      this.#setAuthStatus(status);
      this.#applyTheme(status);
      this.#loadOrganizationsFor(status);
      this.#initializedAt = Date.now();
      return status;
    } catch (caught) {
      this.error =
        caught instanceof Error ? caught.message : "Could not load Gitadel.";
      throw caught;
    } finally {
      this.loading = false;
    }
  }
  advanceAuthorizationScope(): void {
    this.authorizationScope = {
      viewer: this.authStatus?.user?.id ?? null,
      epoch: this.authorizationScope.epoch + 1,
    };
    clearNavigationCaches();
    clearNamespaceCaches();
    clearRepositoryCaches();
    clearRepositoryPreloads();
    clearBackupSettingsCache();
    clearSettingsDataCache();
  }

  #setAuthStatus(status: AuthStatus): void {
    const user = status.user;
    const fingerprint = [
      status.setup_required,
      status.authenticated,
      user?.id ?? "",
      user?.username ?? "",
      user?.is_admin ?? false,
      user?.default_repository_visibility ?? "",
    ].join("|");
    if (fingerprint !== this.#authFingerprint) {
      this.#authFingerprint = fingerprint;
      this.authorizationScope = {
        viewer: user?.id ?? null,
        epoch: this.authorizationScope.epoch + 1,
      };
      clearNavigationCaches();
      clearNamespaceCaches();
      clearRepositoryCaches();
      clearRepositoryPreloads();
      clearBackupSettingsCache();
      clearSettingsDataCache();
    }
    this.authStatus = status;
  }
}

export function provideAppState(): AppState {
  const state = new AppState();
  setContext(APP_STATE, state);
  return state;
}

export function useAppState(): AppState {
  return getContext<AppState>(APP_STATE);
}
