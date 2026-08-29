import { z } from "zod";

import {
  actionRunnersSchema,
  memberSchema,
  mirrorIdentitiesSchema,
  namespaceIntegrationsSchema,
  requestJson,
  type ActionRunner,
  type Member,
  type MirrorIdentity,
} from "$lib/api.js";

export type NamespaceIntegrations = z.infer<typeof namespaceIntegrationsSchema>;
export type NamespaceMirrorSettings = {
  identities: MirrorIdentity[];
};

type CacheEntry = {
  expiresAt: number;
  promise: Promise<unknown>;
};

const CACHE_LIFETIME_MS = 15_000;
const REFRESH_COOLDOWN_MS = 1_000;
const MAX_CACHE_ENTRIES = 32;
const cache = new Map<string, CacheEntry>();

function remember<T>(key: string, load: () => Promise<T>): Promise<T> {
  const now = Date.now();
  const current = cache.get(key);
  if (current && current.expiresAt > now) {
    cache.delete(key);
    cache.set(key, current);
    return current.promise as Promise<T>;
  }
  if (current) cache.delete(key);
  for (const [cachedKey, entry] of cache) {
    if (entry.expiresAt <= now) cache.delete(cachedKey);
  }
  if (cache.size >= MAX_CACHE_ENTRIES) {
    const leastRecentlyUsed = cache.keys().next().value;
    if (leastRecentlyUsed) cache.delete(leastRecentlyUsed);
  }
  const promise: Promise<T> = load().catch((error) => {
    if (cache.get(key)?.promise === promise) cache.delete(key);
    throw error;
  });
  cache.set(key, {
    expiresAt: Date.now() + CACHE_LIFETIME_MS,
    promise,
  });
  return promise;
}

function refresh<T>(key: string, load: () => Promise<T>): Promise<T> {
  const current = cache.get(key);
  if (
    current &&
    current.expiresAt > Date.now() + CACHE_LIFETIME_MS - REFRESH_COOLDOWN_MS
  ) {
    return current.promise as Promise<T>;
  }
  cache.delete(key);
  return remember(key, load);
}

function take<T>(key: string, load: () => Promise<T>): Promise<T> {
  return remember(key, load);
}

function requestMembers(slug: string): Promise<Member[]> {
  return requestJson(
    `/api/v1/organizations/${encodeURIComponent(slug)}/members`,
    z.array(memberSchema),
  );
}

function requestRunners(slug: string): Promise<ActionRunner[]> {
  return requestJson(
    `/api/v1/namespaces/${encodeURIComponent(slug)}/actions/runners`,
    actionRunnersSchema,
  ).then((response) => response.runners);
}

function requestIntegrations(slug: string): Promise<NamespaceIntegrations> {
  return requestJson(
    `/api/v1/namespaces/${encodeURIComponent(slug)}/integrations`,
    namespaceIntegrationsSchema,
  );
}

async function requestMirrorSettings(
  slug: string,
): Promise<NamespaceMirrorSettings> {
  const identities = await requestJson(
    `/api/v1/namespaces/${encodeURIComponent(slug)}/mirror-identities`,
    mirrorIdentitiesSchema,
  );
  return { identities };
}

export function preloadNamespaceTabs(
  slug: string,
  options: { members: boolean; management: boolean },
): void {
  const requests: Promise<unknown>[] = [];
  if (options.members) {
    requests.push(remember(`members:${slug}`, () => requestMembers(slug)));
  }
  if (options.management) {
    requests.push(
      remember(`runners:${slug}`, () => requestRunners(slug)),
      remember(`integrations:${slug}`, () => requestIntegrations(slug)),
      remember(`mirror:${slug}`, () => requestMirrorSettings(slug)),
    );
  }
  void Promise.allSettled(requests);
}

export type NamespaceDataTab =
  "members" | "runners" | "integrations" | "mirror-credentials";

export function refreshNamespaceTab(slug: string, tab: NamespaceDataTab): void {
  const request =
    tab === "members"
      ? refresh(`members:${slug}`, () => requestMembers(slug))
      : tab === "runners"
        ? refresh(`runners:${slug}`, () => requestRunners(slug))
        : tab === "integrations"
          ? refresh(`integrations:${slug}`, () => requestIntegrations(slug))
          : refresh(`mirror:${slug}`, () => requestMirrorSettings(slug));
  void request.catch(() => undefined);
}

export function takeNamespaceMembers(slug: string): Promise<Member[]> {
  return take(`members:${slug}`, () => requestMembers(slug));
}

export function takeNamespaceRunners(slug: string): Promise<ActionRunner[]> {
  return take(`runners:${slug}`, () => requestRunners(slug));
}

export function takeNamespaceIntegrations(
  slug: string,
): Promise<NamespaceIntegrations> {
  return take(`integrations:${slug}`, () => requestIntegrations(slug));
}

export function takeNamespaceMirrorSettings(
  slug: string,
): Promise<NamespaceMirrorSettings> {
  return take(`mirror:${slug}`, () => requestMirrorSettings(slug));
}
