import { z } from "zod";

import { organizationSchema, type Organization } from "$lib/api/organizations.js";
import { repositoryOverviewSchema, repositorySchema } from "$lib/api/repositories.js";
import { requestJson } from "$lib/api/transport.js";
import {
  authorizationCacheScopeKey,
  type AuthorizationCacheScope,
} from "$lib/cache-scope.js";

const cacheLifetime = 5 * 60_000;
const cacheLimit = 128;

type CacheEntry<T> = {
  expiresAt: number;
  promise: Promise<T>;
  value?: T;
};

function pruneCache<T>(cache: Map<string, CacheEntry<T>>): void {
  const now = Date.now();
  for (const [key, entry] of cache) {
    if (entry.expiresAt <= now) cache.delete(key);
  }
  while (cache.size >= cacheLimit) {
    const leastRecentlyUsed = cache.keys().next().value;
    if (leastRecentlyUsed === undefined) break;
    cache.delete(leastRecentlyUsed);
  }
}

function cached<T>(
  cache: Map<string, CacheEntry<T>>,
  key: string,
  loader: () => Promise<T>,
): CacheEntry<T> {
  const existing = cache.get(key);
  if (existing && existing.expiresAt > Date.now()) {
    cache.delete(key);
    cache.set(key, existing);
    return existing;
  }
  pruneCache(cache);
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

function peek<T>(cache: Map<string, CacheEntry<T>>, key: string): T | null {
  const entry = cache.get(key);
  return entry && entry.expiresAt > Date.now() ? (entry.value ?? null) : null;
}

function scopedKey(scope: AuthorizationCacheScope, key: string): string {
  return `${authorizationCacheScopeKey(scope)}\0${key}`;
}

const exploreCache = new Map<
  string,
  CacheEntry<Awaited<ReturnType<typeof fetchExplore>>>
>();

function fetchExplore(
  page: number,
  perPage: number,
  namespace?: string | null,
) {
  const query = new URLSearchParams({
    page: String(page),
    per_page: String(perPage),
  });
  if (namespace) query.set("namespace", namespace);
  return requestJson(
    `/api/v1/repositories/overview?${query}`,
    repositoryOverviewSchema,
  );
}

function exploreKey(
  page: number,
  perPage: number,
  scope: AuthorizationCacheScope,
  namespace?: string | null,
) {
  return scopedKey(scope, `${namespace ?? "*"}:${page}:${perPage}`);
}

const exploreRefreshes = new Map<string, ReturnType<typeof fetchExplore>>();

export function refreshExplore(
  page: number,
  perPage: number,
  scope: AuthorizationCacheScope,
  namespace?: string | null,
) {
  const key = exploreKey(page, perPage, scope, namespace);
  const active = exploreRefreshes.get(key);
  if (active) return active;

  const refresh = fetchExplore(page, perPage, namespace).then((value) => {
    exploreCache.delete(key);
    pruneCache(exploreCache);
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
  scope: AuthorizationCacheScope,
  namespace?: string | null,
) {
  return peek(exploreCache, exploreKey(page, perPage, scope, namespace));
}

export function preloadExplore(
  scope: AuthorizationCacheScope,
  namespace?: string | null,
) {
  const key = exploreKey(1, 20, scope, namespace);
  void cached(exploreCache, key, () => fetchExplore(1, 20, namespace)).promise.catch(
    () => undefined,
  );
}

export function invalidateExplore(scope: AuthorizationCacheScope) {
  const prefix = `${authorizationCacheScopeKey(scope)}\0`;
  for (const key of exploreCache.keys()) {
    if (key.startsWith(prefix)) exploreCache.delete(key);
  }
  for (const key of exploreRefreshes.keys()) {
    if (key.startsWith(prefix)) exploreRefreshes.delete(key);
  }
  for (const key of repositoryIndexCache.keys()) {
    if (key.startsWith(prefix)) repositoryIndexCache.delete(key);
  }
}

const repositoryIndexCache = new Map<
  string,
  CacheEntry<Awaited<ReturnType<typeof fetchRepositoryIndex>>>
>();

function fetchRepositoryIndex() {
  return requestJson("/api/v1/repositories", z.array(repositorySchema));
}

export function loadRepositoryIndex(scope: AuthorizationCacheScope) {
  return cached(repositoryIndexCache, scopedKey(scope, "index"), fetchRepositoryIndex).promise;
}

export function peekRepositoryIndex(scope: AuthorizationCacheScope) {
  return peek(repositoryIndexCache, scopedKey(scope, "index"));
}

export function preloadRepositoryIndex(scope: AuthorizationCacheScope) {
  void loadRepositoryIndex(scope).catch(() => undefined);
}

const organizationCache = new Map<
  string,
  CacheEntry<Awaited<ReturnType<typeof fetchOrganizations>>>
>();

function fetchOrganizations() {
  return requestJson("/api/v1/organizations", z.array(organizationSchema));
}

export function loadOrganizations(scope: AuthorizationCacheScope) {
  return cached(organizationCache, scopedKey(scope, "organizations"), fetchOrganizations).promise;
}

export function refreshOrganizations(scope: AuthorizationCacheScope) {
  organizationCache.delete(scopedKey(scope, "organizations"));
  return loadOrganizations(scope);
}


export function updateOrganizations(
  scope: AuthorizationCacheScope,
  organizations: Organization[],
) {
  const value = [...organizations].sort((left, right) =>
    left.slug.localeCompare(right.slug),
  );
  organizationCache.set(scopedKey(scope, "organizations"), {
    expiresAt: Date.now() + cacheLifetime,
    promise: Promise.resolve(value),
    value,
  });
}


export function clearNavigationCaches(): void {
  exploreCache.clear();
  exploreRefreshes.clear();
  repositoryIndexCache.clear();
  organizationCache.clear();
}
