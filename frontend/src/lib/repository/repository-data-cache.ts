import { z } from "zod";

import { actionRunsSchema, type ActionRuns } from "$lib/api/actions.js";
import { historySchema, type History } from "$lib/api/repositories.js";
import { issueLabelSchema, issueSchema, issueUserSchema, type Issue, type IssueLabel, type IssueUser } from "$lib/api/issues.js";
import { releaseSchema, type Release } from "$lib/api/releases.js";
import { requestJson } from "$lib/api/transport.js";
import { webhookSchema, type Webhook } from "$lib/api/webhooks.js";
import { authorizationCacheScopeKey, type AuthorizationCacheScope } from "$lib/cache-scope.js";

const cacheLifetime = 5 * 60_000;
const cacheLimit = 128;
type CacheEntry<T> = { expiresAt: number; promise: Promise<T> };
const cache = new Map<string, CacheEntry<unknown>>();

function repositoryKey(namespace: string, name: string): string { return `${namespace}\0${name}`; }
function datasetKey(namespace: string, name: string, dataset: string): string { return `${repositoryKey(namespace, name)}\0${dataset}`; }
function scopedDatasetKey(scope: AuthorizationCacheScope, namespace: string, name: string, dataset: string): string { return `${authorizationCacheScopeKey(scope)}\0${datasetKey(namespace, name, dataset)}`; }
function repositoryApi(namespace: string, name: string, path: string): string { return `/api/v1/repositories/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}${path}`; }
function pruneCache(): void {
  const now = Date.now();
  for (const [key, entry] of cache) if (entry.expiresAt <= now) cache.delete(key);
  while (cache.size >= cacheLimit) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}
function loadCached<T>(key: string, loader: () => Promise<T>, refresh = false): Promise<T> {
  const existing = cache.get(key) as CacheEntry<T> | undefined;
  if (!refresh && existing && existing.expiresAt > Date.now()) return existing.promise;
  pruneCache();
  const entry: CacheEntry<T> = { expiresAt: Date.now() + cacheLifetime, promise: Promise.resolve().then(loader) };
  entry.promise.catch(() => { if (cache.get(key) === entry) cache.delete(key); });
  cache.set(key, entry);
  return entry.promise;
}

export function loadRepositoryHistory(namespace: string, name: string, revision: string, scope: AuthorizationCacheScope, page: number, refresh = false): Promise<History> {
  const query = new URLSearchParams({ rev: revision, page: String(page), per_page: "30" });
  return loadCached(scopedDatasetKey(scope, namespace, name, `history:${revision}:${page}`), () => requestJson(`${repositoryApi(namespace, name, "/history")}?${query}`, historySchema), refresh);
}
export function loadRepositoryActionRuns(namespace: string, name: string, _scope: AuthorizationCacheScope, page: number, commit = "", _refresh = false): Promise<ActionRuns> {
  const query = new URLSearchParams({ page: String(page), per_page: "25" });
  if (commit) query.set("commit", commit);
  return requestJson(`${repositoryApi(namespace, name, "/actions/runs")}?${query}`, actionRunsSchema);
}
export function loadRepositoryReleases(namespace: string, name: string, _scope: AuthorizationCacheScope, _refresh = false): Promise<Release[]> { return requestJson(repositoryApi(namespace, name, "/releases"), z.array(releaseSchema)); }
export function loadRepositoryIssues(namespace: string, name: string, _scope: AuthorizationCacheScope, _refresh = false): Promise<Issue[]> { return requestJson(repositoryApi(namespace, name, "/issues"), z.array(issueSchema)); }
export function loadRepositoryIssueLabels(namespace: string, name: string, _scope: AuthorizationCacheScope, _refresh = false): Promise<IssueLabel[]> { return requestJson(repositoryApi(namespace, name, "/issue-labels"), z.array(issueLabelSchema)); }
export function loadRepositoryAssignableUsers(namespace: string, name: string, _scope: AuthorizationCacheScope, _refresh = false): Promise<IssueUser[]> { return requestJson(repositoryApi(namespace, name, "/assignable-users"), z.array(issueUserSchema)); }
export function loadRepositoryWebhooks(namespace: string, name: string, _scope: AuthorizationCacheScope, _refresh = false): Promise<Webhook[]> {
  return requestJson(`/api/v1/repos/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}/hooks`, z.array(webhookSchema));
}
export function preloadRepositoryData(namespace: string, name: string, revision: string, scope: AuthorizationCacheScope, canManage: boolean): Promise<void> {
  const requests: Promise<unknown>[] = [
    loadRepositoryHistory(namespace, name, revision, scope, 1),
    loadRepositoryActionRuns(namespace, name, scope, 1),
    loadRepositoryReleases(namespace, name, scope),
    loadRepositoryIssues(namespace, name, scope),
    loadRepositoryIssueLabels(namespace, name, scope),
    loadRepositoryAssignableUsers(namespace, name, scope),
  ];
  if (canManage) requests.push(loadRepositoryWebhooks(namespace, name, scope));
  return Promise.allSettled(requests).then(() => undefined);
}
export function clearRepositoryDataCache(namespace: string, name: string, scope: AuthorizationCacheScope, _datasets?: readonly string[]): void {
  const prefix = `${authorizationCacheScopeKey(scope)}\0${repositoryKey(namespace, name)}\0`;
  for (const key of cache.keys()) if (key.startsWith(prefix)) cache.delete(key);
}
export function clearRepositoryCaches(): void { cache.clear(); }
