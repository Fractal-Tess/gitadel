import { z } from "zod";

import {
  ApiFailure,
  authStatusSchema,
  blobSchema,
  languageStatSchema,
  organizationSchema,
  refsSchema,
  repositorySchema,
  requestJson,
  topicsSchema,
  treeSchema,
  type Organization,
} from "$lib/api.js";

const preloadLifetime = 30_000;

function repositoryKey(namespace: string, name: string) {
  return `${namespace}\0${name}`;
}

function repositoryApi(namespace: string, name: string, path = "") {
  return `/api/v1/repositories/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}${path}`;
}

async function fetchBootstrap(namespace: string, name: string) {
  const api = repositoryApi(namespace, name);
  const [repository, refs, authStatus, organizations, topics] =
    await Promise.all([
      requestJson(api, repositorySchema),
      requestJson(`${api}/refs`, refsSchema),
      requestJson("/api/v1/auth/status", authStatusSchema),
      requestJson("/api/v1/organizations", z.array(organizationSchema)).catch(
        () => [] as Organization[],
      ),
      requestJson(`${api}/topics`, topicsSchema).catch(() => ({ topics: [] })),
    ]);

  return { repository, refs, authStatus, organizations, topics };
}

async function fetchOverview(
  namespace: string,
  name: string,
  revision: string,
) {
  const api = repositoryApi(namespace, name);
  const query = new URLSearchParams({ rev: revision });

  try {
    const [tree, stats] = await Promise.all([
      requestJson(`${api}/tree?${query}`, treeSchema),
      requestJson(`${api}/stats?${query}`, z.array(languageStatSchema)),
    ]);
    const readmeEntry = tree.entries.find(
      (entry) =>
        entry.kind === "blob" && /^readme(?:\.[^.]+)?$/i.test(entry.name),
    );
    const readme = readmeEntry
      ? await requestJson(
          `${api}/blob?${new URLSearchParams({ rev: revision, path: readmeEntry.path })}`,
          blobSchema,
        )
      : null;

    return { tree, stats, readme, emptyRepository: false };
  } catch (caught) {
    if (caught instanceof ApiFailure && caught.status === 404) {
      return { tree: null, stats: [], readme: null, emptyRepository: true };
    }
    throw caught;
  }
}

type PreloadEntry = {
  expiresAt: number;
  bootstrap: ReturnType<typeof fetchBootstrap>;
  overviews: Map<string, ReturnType<typeof fetchOverview>>;
};

const preloads = new Map<string, PreloadEntry>();

function getEntry(namespace: string, name: string) {
  const key = repositoryKey(namespace, name);
  const cached = preloads.get(key);
  if (cached && cached.expiresAt > Date.now()) return cached;

  const entry: PreloadEntry = {
    expiresAt: Date.now() + preloadLifetime,
    bootstrap: fetchBootstrap(namespace, name),
    overviews: new Map(),
  };
  entry.bootstrap.catch(() => {
    if (preloads.get(key) === entry) preloads.delete(key);
  });
  preloads.set(key, entry);
  return entry;
}

export function loadRepositoryBootstrap(namespace: string, name: string) {
  return getEntry(namespace, name).bootstrap;
}

export function takePreloadedRepositoryOverview(
  namespace: string,
  name: string,
  revision: string,
) {
  const entry = preloads.get(repositoryKey(namespace, name));
  if (!entry || entry.expiresAt <= Date.now()) return null;
  return entry.overviews.get(revision) ?? null;
}

export function clearRepositoryPreload(namespace: string, name: string) {
  preloads.delete(repositoryKey(namespace, name));
}

export function preloadRepository(
  namespace: string,
  name: string,
  knownRevision?: string,
) {
  const entry = getEntry(namespace, name);

  function preloadOverview(revision: string) {
    if (entry.overviews.has(revision)) return;
    const overview = fetchOverview(namespace, name, revision);
    overview.catch(() => entry.overviews.delete(revision));
    entry.overviews.set(revision, overview);
  }

  if (knownRevision) preloadOverview(knownRevision);
  void entry.bootstrap
    .then(({ repository }) => preloadOverview(repository.default_branch))
    .catch(() => undefined);
}
