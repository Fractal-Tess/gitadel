import { z } from "zod";

import { ApiFailure, requestJson } from "$lib/api/transport.js";
import { authStatusSchema } from "$lib/api/auth.js";
import {
  blobSchema,
  languageStatSchema,
  refsSchema,
  repositorySchema,
  topicsSchema,
  treeSchema,
} from "$lib/api/repositories.js";
import { organizationSchema, type Organization } from "$lib/api/organizations.js";
import {
  authorizationCacheScopeKey,
  type AuthorizationCacheScope,
} from "$lib/cache-scope.js";

const preloadLifetime = 30_000;
const preloadIntentDelay = 120;
const preloadLimit = 64;

function repositoryKey(namespace: string, name: string): string {
  return `${namespace}\0${name}`;
}
function scopedRepositoryKey(scope: AuthorizationCacheScope, namespace: string, name: string): string {
  return `${authorizationCacheScopeKey(scope)}\0${repositoryKey(namespace, name)}`;
}
function repositoryApi(namespace: string, name: string, path = ""): string {
  return `/api/v1/repositories/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}${path}`;
}

async function fetchBootstrap(namespace: string, name: string) {
  const api = repositoryApi(namespace, name);
  const [repository, refs, authStatus, organizations, topics] = await Promise.all([
    requestJson(api, repositorySchema),
    requestJson(`${api}/refs`, refsSchema),
    requestJson("/api/v1/auth/status", authStatusSchema),
    requestJson("/api/v1/organizations", z.array(organizationSchema)).catch(() => [] as Organization[]),
    requestJson(`${api}/topics`, topicsSchema).catch(() => ({ topics: [] })),
  ]);
  return { repository, refs, authStatus, organizations, topics };
}

async function fetchOverview(namespace: string, name: string, revision: string) {
  const api = repositoryApi(namespace, name);
  const query = new URLSearchParams({ rev: revision });
  try {
    const [tree, stats] = await Promise.all([
      requestJson(`${api}/tree?${query}`, treeSchema),
      requestJson(`${api}/stats?${query}`, z.array(languageStatSchema)),
    ]);
    const readmeEntry = tree.entries.find((entry) => entry.kind === "blob" && /^readme(?:\.[^.]+)?$/i.test(entry.name));
    const readme = readmeEntry
      ? await requestJson(`${api}/blob?${new URLSearchParams({ rev: revision, path: readmeEntry.path })}`, blobSchema)
      : null;
    return { tree, stats, readme, emptyRepository: false };
  } catch (caught) {
    if (caught instanceof ApiFailure && caught.status === 404) return { tree: null, stats: [], readme: null, emptyRepository: true };
    throw caught;
  }
}

type PreloadEntry = {
  expiresAt: number;
  bootstrap: ReturnType<typeof fetchBootstrap>;
  overviews: Map<string, ReturnType<typeof fetchOverview>>;
};
const preloads = new Map<string, PreloadEntry>();
const preloadTimers = new Map<string, number>();

function prunePreloads(): void {
  const now = Date.now();
  for (const [key, entry] of preloads) if (entry.expiresAt <= now) preloads.delete(key);
  while (preloads.size >= preloadLimit) {
    const oldest = preloads.keys().next().value;
    if (oldest === undefined) break;
    preloads.delete(oldest);
  }
}
function getEntry(scope: AuthorizationCacheScope, namespace: string, name: string): PreloadEntry {
  const key = scopedRepositoryKey(scope, namespace, name);
  const cached = preloads.get(key);
  if (cached && cached.expiresAt > Date.now()) return cached;
  prunePreloads();
  const entry: PreloadEntry = { expiresAt: Date.now() + preloadLifetime, bootstrap: fetchBootstrap(namespace, name), overviews: new Map() };
  entry.bootstrap.catch(() => { if (preloads.get(key) === entry) preloads.delete(key); });
  preloads.set(key, entry);
  return entry;
}

export function loadRepositoryBootstrap(namespace: string, name: string, scope: AuthorizationCacheScope) {
  return getEntry(scope, namespace, name).bootstrap;
}
export function takePreloadedRepositoryOverview(namespace: string, name: string, scope: AuthorizationCacheScope, revision: string) {
  const entry = preloads.get(scopedRepositoryKey(scope, namespace, name));
  if (!entry || entry.expiresAt <= Date.now()) return null;
  return entry.overviews.get(revision) ?? null;
}
export function clearRepositoryPreload(namespace: string, name: string, scope: AuthorizationCacheScope): void {
  preloads.delete(scopedRepositoryKey(scope, namespace, name));
}
export function scheduleRepositoryPreload(namespace: string, name: string, scope: AuthorizationCacheScope, knownRevision?: string): void {
  const key = scopedRepositoryKey(scope, namespace, name);
  if ((preloads.get(key)?.expiresAt ?? 0) > Date.now()) return;
  window.clearTimeout(preloadTimers.get(key));
  preloadTimers.set(key, window.setTimeout(() => {
    preloadTimers.delete(key);
    preloadRepository(namespace, name, scope, knownRevision);
  }, preloadIntentDelay));
}
export function cancelRepositoryPreload(namespace: string, name: string, scope: AuthorizationCacheScope): void {
  const key = scopedRepositoryKey(scope, namespace, name);
  window.clearTimeout(preloadTimers.get(key));
  preloadTimers.delete(key);
}
export function preloadRepository(namespace: string, name: string, scope: AuthorizationCacheScope, knownRevision?: string): void {
  const entry = getEntry(scope, namespace, name);
  void import("$lib/repository/material-file-icons.js").then(({ preloadMaterialIconTheme }) => preloadMaterialIconTheme()).catch(() => undefined);
  function preloadOverview(revision: string): void {
    if (entry.overviews.has(revision)) return;
    const overview = fetchOverview(namespace, name, revision);
    overview.catch(() => entry.overviews.delete(revision));
    entry.overviews.set(revision, overview);
  }
  if (knownRevision) preloadOverview(knownRevision);
  void entry.bootstrap.then(({ repository }) => preloadOverview(repository.default_branch)).catch(() => undefined);
}
export function clearRepositoryPreloads(): void {
  preloads.clear();
  for (const timer of preloadTimers.values()) window.clearTimeout(timer);
  preloadTimers.clear();
}
