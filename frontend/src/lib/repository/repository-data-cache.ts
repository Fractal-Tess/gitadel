import { z } from "zod";

import {
  actionRunsSchema,
  historySchema,
  issueLabelSchema,
  issueSchema,
  issueUserSchema,
  releaseSchema,
  requestJson,
  webhookSchema,
  type ActionRuns,
  type History,
  type Issue,
  type IssueLabel,
  type IssueUser,
  type Release,
  type Webhook,
} from "$lib/api.js";

const cacheLifetime = 5 * 60_000;
const cacheLimit = 128;

type CacheEntry<T> = {
  expiresAt: number;
  promise: Promise<T>;
};

const cache = new Map<string, CacheEntry<unknown>>();

function repositoryKey(namespace: string, name: string): string {
  return `${namespace}\0${name}`;
}

function datasetKey(namespace: string, name: string, dataset: string): string {
  return `${repositoryKey(namespace, name)}\0${dataset}`;
}

function repositoryApi(namespace: string, name: string, path: string): string {
  return `/api/v1/repositories/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}${path}`;
}

function pruneCache(): void {
  const now = Date.now();
  for (const [key, entry] of cache) {
    if (entry.expiresAt <= now) cache.delete(key);
  }
  while (cache.size >= cacheLimit) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}

function loadCached<T>(
  key: string,
  loader: () => Promise<T>,
  refresh = false,
): Promise<T> {
  const existing = cache.get(key) as CacheEntry<T> | undefined;
  if (!refresh && existing && existing.expiresAt > Date.now()) {
    return existing.promise;
  }

  pruneCache();
  const entry: CacheEntry<T> = {
    expiresAt: Date.now() + cacheLifetime,
    promise: Promise.resolve().then(loader),
  };
  entry.promise.catch(() => {
    if (cache.get(key) === entry) cache.delete(key);
  });
  cache.set(key, entry);
  return entry.promise;
}

export function loadRepositoryHistory(
  namespace: string,
  name: string,
  revision: string,
  page: number,
  refresh = false,
): Promise<History> {
  const query = new URLSearchParams({
    rev: revision,
    page: String(page),
    per_page: "30",
  });
  return loadCached(
    datasetKey(namespace, name, `history:${revision}:${page}`),
    () =>
      requestJson(
        `${repositoryApi(namespace, name, "/history")}?${query}`,
        historySchema,
      ),
    refresh,
  );
}

export function loadRepositoryActionRuns(
  namespace: string,
  name: string,
  page: number,
  commit = "",
  refresh = false,
): Promise<ActionRuns> {
  const query = new URLSearchParams({
    page: String(page),
    per_page: "25",
  });
  if (commit) query.set("commit", commit);
  return loadCached(
    datasetKey(namespace, name, `actions:${page}:${commit}`),
    () =>
      requestJson(
        `${repositoryApi(namespace, name, "/actions/runs")}?${query}`,
        actionRunsSchema,
      ),
    refresh,
  );
}

export function loadRepositoryReleases(
  namespace: string,
  name: string,
  refresh = false,
): Promise<Release[]> {
  return loadCached(
    datasetKey(namespace, name, "releases"),
    () =>
      requestJson(
        repositoryApi(namespace, name, "/releases"),
        z.array(releaseSchema),
      ),
    refresh,
  );
}

export function loadRepositoryIssues(
  namespace: string,
  name: string,
  refresh = false,
): Promise<Issue[]> {
  return loadCached(
    datasetKey(namespace, name, "issues"),
    () =>
      requestJson(
        repositoryApi(namespace, name, "/issues"),
        z.array(issueSchema),
      ),
    refresh,
  );
}

export function loadRepositoryIssueLabels(
  namespace: string,
  name: string,
  refresh = false,
): Promise<IssueLabel[]> {
  return loadCached(
    datasetKey(namespace, name, "issue-labels"),
    () =>
      requestJson(
        repositoryApi(namespace, name, "/issue-labels"),
        z.array(issueLabelSchema),
      ),
    refresh,
  );
}

export function loadRepositoryAssignableUsers(
  namespace: string,
  name: string,
  refresh = false,
): Promise<IssueUser[]> {
  return loadCached(
    datasetKey(namespace, name, "assignable-users"),
    () =>
      requestJson(
        repositoryApi(namespace, name, "/assignable-users"),
        z.array(issueUserSchema),
      ),
    refresh,
  );
}

export function loadRepositoryWebhooks(
  namespace: string,
  name: string,
  refresh = false,
): Promise<Webhook[]> {
  const url = `/api/v1/repos/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}/hooks`;
  return loadCached(
    datasetKey(namespace, name, "webhooks"),
    () => requestJson(url, z.array(webhookSchema)),
    refresh,
  );
}

export function preloadRepositoryData(
  namespace: string,
  name: string,
  revision: string,
  canManage: boolean,
): Promise<void> {
  const requests: Promise<unknown>[] = [
    loadRepositoryHistory(namespace, name, revision, 1),
    loadRepositoryActionRuns(namespace, name, 1),
    loadRepositoryReleases(namespace, name),
    loadRepositoryIssues(namespace, name),
    loadRepositoryIssueLabels(namespace, name),
    loadRepositoryAssignableUsers(namespace, name),
  ];
  if (canManage) requests.push(loadRepositoryWebhooks(namespace, name));
  return Promise.allSettled(requests).then(() => undefined);
}

export function clearRepositoryDataCache(
  namespace: string,
  name: string,
  datasets?: readonly string[],
): void {
  const repositoryPrefix = `${repositoryKey(namespace, name)}\0`;
  const prefixes = datasets?.map(
    (dataset) => `${repositoryPrefix}${dataset}`,
  ) ?? [repositoryPrefix];
  for (const key of cache.keys()) {
    if (prefixes.some((prefix) => key.startsWith(prefix))) cache.delete(key);
  }
}
