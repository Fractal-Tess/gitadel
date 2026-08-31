import { z } from "zod";

import {
  oauthApplicationSchema,
  passkeySchema,
  sshKeySchema,
  tokenSchema,
  type ApiToken,
  type OauthApplication,
  type PasskeySummary,
  type SshKey,
} from "$lib/api/account.js";
import {
  adminOidcProvidersSchema,
  authenticationConfigurationSchema,
  type AdminOidcProvider,
  type AuthenticationConfiguration,
} from "$lib/api/sso.js";
import { auditEventSchema, type AuditEvent } from "$lib/api/instance.js";
import {
  lfsStorageStatusSchema,
  storageTargetsSchema,
  type LfsStorageStatus,
  type StorageTarget,
} from "$lib/api/storage.js";
import { requestJson } from "$lib/api/transport.js";
import {
  authorizationCacheScopeKey,
  type AuthorizationCacheScope,
} from "$lib/cache-scope.js";
import { preloadBackupSettings } from "$lib/settings/backup-settings-cache.js";

const CACHE_LIFETIME_MS = 5 * 60_000;
const CACHE_LIMIT = 64;

type CacheEntry<T> = {
  expiresAt: number;
  promise: Promise<T>;
  value?: T;
};

class SettingsCache<T> {
  readonly #entries = new Map<string, CacheEntry<T>>();

  load(key: string, loader: () => Promise<T>): Promise<T> {
    const existing = this.#entries.get(key);
    if (existing && existing.expiresAt > Date.now()) {
      this.#entries.delete(key);
      this.#entries.set(key, existing);
      return existing.promise;
    }
    if (existing) this.#entries.delete(key);
    this.#prune();

    const entry: CacheEntry<T> = {
      expiresAt: Date.now() + CACHE_LIFETIME_MS,
      promise: Promise.resolve().then(loader),
    };
    entry.promise.then(
      (value) => {
        if (this.#entries.get(key) === entry) entry.value = value;
      },
      () => {
        if (this.#entries.get(key) === entry) this.#entries.delete(key);
      },
    );
    this.#entries.set(key, entry);
    return entry.promise;
  }

  peek(key: string): T | null {
    const entry = this.#entries.get(key);
    if (!entry || entry.expiresAt <= Date.now()) {
      if (entry) this.#entries.delete(key);
      return null;
    }
    return entry.value ?? null;
  }

  set(key: string, value: T): void {
    this.#entries.delete(key);
    this.#prune();
    this.#entries.set(key, {
      expiresAt: Date.now() + CACHE_LIFETIME_MS,
      promise: Promise.resolve(value),
      value,
    });
  }

  invalidate(key: string): void {
    this.#entries.delete(key);
  }

  clear(): void {
    this.#entries.clear();
  }

  #prune(): void {
    const now = Date.now();
    for (const [key, entry] of this.#entries) {
      if (entry.expiresAt <= now) this.#entries.delete(key);
    }
    while (this.#entries.size >= CACHE_LIMIT) {
      const oldest = this.#entries.keys().next().value;
      if (oldest === undefined) break;
      this.#entries.delete(oldest);
    }
  }
}

export type AccountSettingsView =
  | "profile"
  | "authentication"
  | "ssh-keys"
  | "api-tokens"
  | "oauth-applications";

export type AdminSettingsView =
  "appearance" | "access" | "storage" | "lfs" | "backups" | "activity";

const passkeys = new SettingsCache<PasskeySummary[]>();
const sshKeys = new SettingsCache<SshKey[]>();
const apiTokens = new SettingsCache<ApiToken[]>();
const oauthApplications = new SettingsCache<OauthApplication[]>();
const storageTargets = new SettingsCache<StorageTarget[]>();
const lfsStatus = new SettingsCache<LfsStorageStatus>();
const adminActivity = new SettingsCache<AuditEvent[]>();
const authenticationConfiguration =
  new SettingsCache<AuthenticationConfiguration>();
const oidcProviders = new SettingsCache<AdminOidcProvider[]>();

function key(scope: AuthorizationCacheScope): string {
  return authorizationCacheScopeKey(scope);
}

export function loadPasskeys(
  scope: AuthorizationCacheScope,
): Promise<PasskeySummary[]> {
  return passkeys.load(key(scope), () =>
    requestJson("/api/v1/me/passkeys", z.array(passkeySchema)),
  );
}

export function peekPasskeys(
  scope: AuthorizationCacheScope,
): PasskeySummary[] | null {
  return passkeys.peek(key(scope));
}

export function setPasskeys(
  scope: AuthorizationCacheScope,
  value: PasskeySummary[],
): void {
  passkeys.set(key(scope), value);
}

export function loadSshKeys(scope: AuthorizationCacheScope): Promise<SshKey[]> {
  return sshKeys.load(key(scope), () =>
    requestJson("/api/v1/me/ssh-keys", z.array(sshKeySchema)),
  );
}

export function peekSshKeys(scope: AuthorizationCacheScope): SshKey[] | null {
  return sshKeys.peek(key(scope));
}

export function setSshKeys(
  scope: AuthorizationCacheScope,
  value: SshKey[],
): void {
  sshKeys.set(key(scope), value);
}

export function loadApiTokens(
  scope: AuthorizationCacheScope,
): Promise<ApiToken[]> {
  return apiTokens.load(key(scope), () =>
    requestJson("/api/v1/me/tokens", z.array(tokenSchema)),
  );
}

export function peekApiTokens(
  scope: AuthorizationCacheScope,
): ApiToken[] | null {
  return apiTokens.peek(key(scope));
}

export function setApiTokens(
  scope: AuthorizationCacheScope,
  value: ApiToken[],
): void {
  apiTokens.set(key(scope), value);
}

export function loadOauthApplications(
  scope: AuthorizationCacheScope,
): Promise<OauthApplication[]> {
  return oauthApplications.load(key(scope), () =>
    requestJson(
      "/api/v1/me/oauth-applications",
      z.array(oauthApplicationSchema),
    ),
  );
}

export function peekOauthApplications(
  scope: AuthorizationCacheScope,
): OauthApplication[] | null {
  return oauthApplications.peek(key(scope));
}

export function setOauthApplications(
  scope: AuthorizationCacheScope,
  value: OauthApplication[],
): void {
  oauthApplications.set(key(scope), value);
}

export function preloadAccountSettingsView(
  scope: AuthorizationCacheScope,
  view: AccountSettingsView,
): void {
  const request =
    view === "authentication"
      ? loadPasskeys(scope)
      : view === "ssh-keys"
        ? loadSshKeys(scope)
        : view === "api-tokens"
          ? loadApiTokens(scope)
          : view === "oauth-applications"
            ? loadOauthApplications(scope)
            : null;
  void request?.catch(() => undefined);
}

export function loadStorageTargets(
  scope: AuthorizationCacheScope,
): Promise<StorageTarget[]> {
  return storageTargets.load(key(scope), () =>
    requestJson("/api/v1/admin/storage/targets", storageTargetsSchema),
  );
}

export function peekStorageTargets(
  scope: AuthorizationCacheScope,
): StorageTarget[] | null {
  return storageTargets.peek(key(scope));
}

export function setStorageTargets(
  scope: AuthorizationCacheScope,
  value: StorageTarget[],
): void {
  storageTargets.set(key(scope), value);
}
export function refreshStorageTargets(
  scope: AuthorizationCacheScope,
): Promise<StorageTarget[]> {
  storageTargets.invalidate(key(scope));
  return loadStorageTargets(scope);
}

export function loadLfsStatus(
  scope: AuthorizationCacheScope,
): Promise<LfsStorageStatus> {
  return lfsStatus.load(key(scope), () =>
    requestJson("/api/v1/admin/storage/lfs/status", lfsStorageStatusSchema),
  );
}

export function peekLfsStatus(
  scope: AuthorizationCacheScope,
): LfsStorageStatus | null {
  return lfsStatus.peek(key(scope));
}

export function setLfsStatus(
  scope: AuthorizationCacheScope,
  value: LfsStorageStatus,
): void {
  lfsStatus.set(key(scope), value);
}
export function refreshLfsStatus(
  scope: AuthorizationCacheScope,
): Promise<LfsStorageStatus> {
  lfsStatus.invalidate(key(scope));
  return loadLfsStatus(scope);
}

export function loadAdminActivity(
  scope: AuthorizationCacheScope,
): Promise<AuditEvent[]> {
  return adminActivity.load(key(scope), () =>
    requestJson("/api/v1/audit?limit=100", z.array(auditEventSchema)),
  );
}

export function peekAdminActivity(
  scope: AuthorizationCacheScope,
): AuditEvent[] | null {
  return adminActivity.peek(key(scope));
}

export function refreshAdminActivity(
  scope: AuthorizationCacheScope,
): Promise<AuditEvent[]> {
  adminActivity.invalidate(key(scope));
  return loadAdminActivity(scope);
}

export function invalidateAdminActivity(scope: AuthorizationCacheScope): void {
  adminActivity.invalidate(key(scope));
}

export function loadAuthenticationConfiguration(
  scope: AuthorizationCacheScope,
): Promise<AuthenticationConfiguration> {
  return authenticationConfiguration.load(key(scope), () =>
    requestJson(
      "/api/v1/admin/authentication",
      authenticationConfigurationSchema,
    ),
  );
}

export function setAuthenticationConfiguration(
  scope: AuthorizationCacheScope,
  value: AuthenticationConfiguration,
): void {
  authenticationConfiguration.set(key(scope), value);
}

export function loadOidcProviders(
  scope: AuthorizationCacheScope,
): Promise<AdminOidcProvider[]> {
  return oidcProviders.load(key(scope), () =>
    requestJson(
      "/api/v1/admin/authentication/providers",
      adminOidcProvidersSchema,
    ),
  );
}

export function setOidcProviders(
  scope: AuthorizationCacheScope,
  value: AdminOidcProvider[],
): void {
  oidcProviders.set(key(scope), value);
}

export function preloadAdminSettingsView(
  scope: AuthorizationCacheScope,
  view: AdminSettingsView,
): void {
  const requests =
    view === "access"
      ? [loadAuthenticationConfiguration(scope), loadOidcProviders(scope)]
      : view === "storage"
        ? [loadStorageTargets(scope)]
        : view === "lfs"
          ? [loadStorageTargets(scope), loadLfsStatus(scope)]
          : view === "backups"
            ? (preloadBackupSettings(scope), [])
            : view === "activity"
              ? [loadAdminActivity(scope)]
              : [];
  void Promise.allSettled(requests);
}

export function clearSettingsDataCache(): void {
  passkeys.clear();
  sshKeys.clear();
  apiTokens.clear();
  oauthApplications.clear();
  storageTargets.clear();
  lfsStatus.clear();
  adminActivity.clear();
  authenticationConfiguration.clear();
  oidcProviders.clear();
}
