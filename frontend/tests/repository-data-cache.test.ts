import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, test } from "node:test";

import {
  clearNamespaceCaches,
  takeNamespaceMembers,
} from "../src/lib/namespace-preload.js";
import {
  clearNavigationCaches,
  loadRepositoryIndex,
} from "../src/lib/navigation-cache.js";
import type { AuthorizationCacheScope } from "../src/lib/cache-scope.js";
import {
  clearBackupSettingsCache,
  loadBackupSettings,
} from "../src/lib/settings/backup-settings-cache.js";
import {
  clearRepositoryCaches,
  clearRepositoryDataCache,
  loadRepositoryHistory,
  preloadRepositoryData,
} from "../src/lib/repository/repository-data-cache.js";

const originalFetch = globalThis.fetch;
const scope = { viewer: "viewer", epoch: 1 } as const;

const historyResponse = {
  commits: [],
  page: 1,
  per_page: 30,
  has_next: false,
} as const;
const emptyBackupResponse = { providers: [], connections: [] } as const;

type CacheCase = {
  name: string;
  load(scope: AuthorizationCacheScope): Promise<unknown>;
  response: unknown;
};

const cacheCases: readonly CacheCase[] = [
  {
    name: "repository history cache",
    load: (authorizationScope) =>
      loadRepositoryHistory(
        "scope-acme",
        "scope-widgets",
        "main",
        authorizationScope,
        1,
      ),
    response: historyResponse,
  },
  {
    name: "navigation cache",
    load: loadRepositoryIndex,
    response: [],
  },
  {
    name: "namespace cache",
    load: (authorizationScope) =>
      takeNamespaceMembers("scope-acme", authorizationScope),
    response: [],
  },
  {
    name: "backup settings cache",
    load: loadBackupSettings,
    response: emptyBackupResponse,
  },
];

type PendingRequest = {
  path: string;
  resolve(response: Response): void;
};

function deferFetches(): PendingRequest[] {
  const requests: PendingRequest[] = [];
  globalThis.fetch = ((input: string | URL | Request) =>
    new Promise<Response>((resolve) => {
      requests.push({ path: String(input), resolve });
    })) as typeof fetch;
  return requests;
}

beforeEach(() => {
  clearNavigationCaches();
  clearNamespaceCaches();
  clearRepositoryCaches();
  clearBackupSettingsCache();
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe("authorization cache scopes", () => {
  for (const cacheCase of cacheCases) {
    test(`${cacheCase.name} does not reuse settled data for another viewer`, async () => {
      let requestCount = 0;
      globalThis.fetch = (() => {
        requestCount += 1;
        return Promise.resolve(Response.json(cacheCase.response));
      }) as typeof fetch;

      const first = await cacheCase.load({ viewer: "viewer-a", epoch: 1 });
      const second = await cacheCase.load({ viewer: "viewer-b", epoch: 1 });

      assert.equal(requestCount, 2);
      assert.notStrictEqual(first, second);
    });

    test(`${cacheCase.name} does not reuse in-flight data after an epoch advance`, async () => {
      const requests = deferFetches();
      const first = cacheCase.load({ viewer: "viewer", epoch: 1 });
      const second = cacheCase.load({ viewer: "viewer", epoch: 2 });

      await Promise.resolve();

      assert.equal(requests.length, 2);
      for (const request of requests) {
        request.resolve(Response.json(cacheCase.response));
      }
      await Promise.all([first, second]);
    });
  }

  test("a stale repository response cannot replace data from the advanced scope", async () => {
    const requests = deferFetches();
    const staleScope = { viewer: "viewer", epoch: 1 } as const;
    const currentScope = { viewer: "viewer", epoch: 2 } as const;
    const stale = loadRepositoryHistory(
      "scope-acme",
      "scope-widgets",
      "main",
      staleScope,
      1,
    );
    const current = loadRepositoryHistory(
      "scope-acme",
      "scope-widgets",
      "main",
      currentScope,
      1,
    );

    await Promise.resolve();
    assert.equal(requests.length, 2);

    const currentResponse = { ...historyResponse, page: 2 };
    requests[1].resolve(Response.json(currentResponse));
    await current;
    requests[0].resolve(Response.json(historyResponse));
    await stale;

    const adopted = await loadRepositoryHistory(
      "scope-acme",
      "scope-widgets",
      "main",
      currentScope,
      1,
    );
    assert.deepEqual(
      { adopted, requestCount: requests.length },
      { adopted: currentResponse, requestCount: 2 },
    );
  });
});

describe("repository data cache", () => {
  test("deduplicates revision-keyed history within one scope", async () => {
    const paths: string[] = [];
    globalThis.fetch = ((input: string | URL | Request) => {
      paths.push(String(input));
      return Promise.resolve(Response.json(historyResponse));
    }) as typeof fetch;

    await Promise.all([
      loadRepositoryHistory("acme", "widgets", "main", scope, 1),
      loadRepositoryHistory("acme", "widgets", "main", scope, 1),
    ]);
    await loadRepositoryHistory("acme", "widgets", "main", scope, 1);

    assert.deepEqual(paths, [
      "/api/v1/repositories/acme/widgets/history?rev=main&page=1&per_page=30",
    ]);
  });

  test("invalidating history forces a new request", async () => {
    let requestCount = 0;
    globalThis.fetch = (() => {
      requestCount += 1;
      return Promise.resolve(Response.json(historyResponse));
    }) as typeof fetch;

    await loadRepositoryHistory("acme", "widgets", "main", scope, 1);
    clearRepositoryDataCache("acme", "widgets", scope, ["history"]);
    await loadRepositoryHistory("acme", "widgets", "main", scope, 1);

    assert.equal(requestCount, 2);
  });
  test("preloads every default tab dataset without surfacing failures", async () => {
    const paths: string[] = [];
    globalThis.fetch = ((input: string | URL | Request) => {
      paths.push(String(input));
      return Promise.resolve(Response.json(null, { status: 500 }));
    }) as typeof fetch;

    await preloadRepositoryData("acme", "widgets", "main", scope, true);

    assert.deepEqual(
      paths.sort(),
      [
        "/api/v1/repos/acme/widgets/hooks",
        "/api/v1/repositories/acme/widgets/actions/runs?page=1&per_page=25",
        "/api/v1/repositories/acme/widgets/assignable-users",
        "/api/v1/repositories/acme/widgets/history?rev=main&page=1&per_page=30",
        "/api/v1/repositories/acme/widgets/issue-labels",
        "/api/v1/repositories/acme/widgets/issues",
        "/api/v1/repositories/acme/widgets/releases",
      ].sort(),
    );
  });
});

describe("backup settings cache", () => {
  test("preloads providers and the selected provider snapshots once", async () => {
    const paths: string[] = [];
    globalThis.fetch = ((input: string | URL | Request) => {
      const path = String(input);
      paths.push(path);
      if (path === "/api/v1/admin/backup/providers") {
        return Promise.resolve(
          Response.json({
            providers: [],
            connections: [
              {
                id: "123e4567-e89b-42d3-a456-426614174000",
                name: "Local",
                provider: "filesystem",
                managed_by_config: false,
                path: "/backups",
                endpoint: null,
                bucket: null,
                access_key_hint: null,
                region: null,
                prefix: null,
                schedule: null,
                next_backup_at: null,
              },
            ],
          }),
        );
      }
      return Promise.resolve(Response.json([]));
    }) as typeof fetch;

    await Promise.all([loadBackupSettings(scope), loadBackupSettings(scope)]);

    assert.deepEqual(paths, [
      "/api/v1/admin/backup/providers",
      "/api/v1/admin/backup/providers/123e4567-e89b-42d3-a456-426614174000/backups",
    ]);
  });
});

describe("namespace preload cache", () => {
  test("retains a recently used entry when the cache reaches its bound", async () => {
    const paths: string[] = [];
    globalThis.fetch = ((input: string | URL | Request) => {
      paths.push(String(input));
      return Promise.resolve(Response.json([]));
    }) as typeof fetch;

    await takeNamespaceMembers("lru-hot", scope);
    for (let index = 0; index < 31; index += 1) {
      await takeNamespaceMembers(`lru-cold-${index}`, scope);
    }
    await takeNamespaceMembers("lru-hot", scope);
    await takeNamespaceMembers("lru-overflow", scope);
    await takeNamespaceMembers("lru-hot", scope);

    assert.equal(
      paths.filter((path) => path.endsWith("/lru-hot/members")).length,
      1,
    );
  });
});
