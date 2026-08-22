import { z } from "zod";

import {
  oauthApplicationSchema,
  organizationSchema,
  passkeySchema,
  repositoryOverviewSchema,
  requestJson,
  sshKeySchema,
  tokenSchema,
} from "$lib/api.js";

const cacheLifetime = 5 * 60_000;

type CacheEntry<T> = {
  expiresAt: number;
  promise: Promise<T>;
  value?: T;
};

function cached<T>(
  cache: Map<string, CacheEntry<T>>,
  key: string,
  loader: () => Promise<T>,
) {
  const existing = cache.get(key);
  if (existing && existing.expiresAt > Date.now()) return existing;

  const entry: CacheEntry<T> = {
    expiresAt: Date.now() + cacheLifetime,
    promise: Promise.resolve().then(loader),
  };
  entry.promise.then(
    (value) => {
      entry.value = value;
    },
    () => {
      if (cache.get(key) === entry) cache.delete(key);
    },
  );
  cache.set(key, entry);
  return entry;
}

function peek<T>(cache: Map<string, CacheEntry<T>>, key: string) {
  const entry = cache.get(key);
  return entry && entry.expiresAt > Date.now() ? (entry.value ?? null) : null;
}

function viewerKey(username: string | null | undefined) {
  return username ?? "anonymous";
}

const exploreCache = new Map<
  string,
  CacheEntry<Awaited<ReturnType<typeof fetchExplore>>>
>();

function fetchExplore(page: number, perPage: number) {
  return requestJson(
    `/api/v1/repositories/overview?${new URLSearchParams({ page: String(page), per_page: String(perPage) })}`,
    repositoryOverviewSchema,
  );
}

function exploreKey(page: number, perPage: number, username?: string | null) {
  return `${viewerKey(username)}:${page}:${perPage}`;
}

const exploreRefreshes = new Map<string, ReturnType<typeof fetchExplore>>();

export function refreshExplore(
  page: number,
  perPage: number,
  username?: string | null,
) {
  const key = exploreKey(page, perPage, username);
  const active = exploreRefreshes.get(key);
  if (active) return active;

  const refresh = fetchExplore(page, perPage).then((value) => {
    exploreCache.set(key, {
      expiresAt: Date.now() + cacheLifetime,
      promise: Promise.resolve(value),
      value,
    });
    return value;
  });
  exploreRefreshes.set(key, refresh);
  void refresh.then(
    () => exploreRefreshes.delete(key),
    () => exploreRefreshes.delete(key),
  );
  return refresh;
}

export function peekExplore(
  page: number,
  perPage: number,
  username?: string | null,
) {
  return peek(exploreCache, exploreKey(page, perPage, username));
}

export function preloadExplore(username?: string | null) {
  void refreshExplore(1, 20, username).catch(() => undefined);
}

export function invalidateExplore(username?: string | null) {
  const prefix = `${viewerKey(username)}:`;
  for (const key of exploreCache.keys()) {
    if (key.startsWith(prefix)) {
      exploreCache.delete(key);
      exploreRefreshes.delete(key);
    }
  }
}

const organizationCache = new Map<
  string,
  CacheEntry<Awaited<ReturnType<typeof fetchOrganizations>>>
>();

function fetchOrganizations() {
  return requestJson("/api/v1/organizations", z.array(organizationSchema));
}

export function loadOrganizations(username?: string | null) {
  const key = viewerKey(username);
  return cached(organizationCache, key, fetchOrganizations).promise;
}

const accountSettingsCache = new Map<
  string,
  CacheEntry<Awaited<ReturnType<typeof fetchAccountSettings>>>
>();

async function fetchAccountSettings(username: string) {
  const [passkeys, sshKeys, tokens, oauthApplications, organizations] =
    await Promise.all([
      requestJson("/api/v1/me/passkeys", z.array(passkeySchema)),
      requestJson("/api/v1/me/ssh-keys", z.array(sshKeySchema)),
      requestJson("/api/v1/me/tokens", z.array(tokenSchema)),
      requestJson(
        "/api/v1/me/oauth-applications",
        z.array(oauthApplicationSchema),
      ),
      loadOrganizations(username),
    ]);

  return { passkeys, sshKeys, tokens, oauthApplications, organizations };
}

export type AccountSettingsData = Awaited<
  ReturnType<typeof fetchAccountSettings>
>;

export function loadAccountSettings(username: string) {
  return cached(accountSettingsCache, username, () =>
    fetchAccountSettings(username),
  ).promise;
}

export function peekAccountSettings(username: string) {
  return peek(accountSettingsCache, username);
}

export function preloadAccountSettings(username: string | null | undefined) {
  if (!username) return;
  void loadAccountSettings(username).catch(() => undefined);
}

export function updateAccountSettings(
  username: string,
  value: AccountSettingsData,
) {
  accountSettingsCache.set(username, {
    expiresAt: Date.now() + cacheLifetime,
    promise: Promise.resolve(value),
    value,
  });
  organizationCache.set(username, {
    expiresAt: Date.now() + cacheLifetime,
    promise: Promise.resolve(value.organizations),
    value: value.organizations,
  });
}

export function clearAccountSettings(username: string) {
  accountSettingsCache.delete(username);
  organizationCache.delete(username);
}
