import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, test } from "node:test";

import {
  clearRepositoryDataCache,
  loadRepositoryIssues,
  preloadRepositoryData,
} from "../src/lib/repository/repository-data-cache.js";

const originalFetch = globalThis.fetch;

beforeEach(() => {
  clearRepositoryDataCache("acme", "widgets");
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe("repository data cache", () => {
  test("deduplicates matching frontend data requests", async () => {
    const paths: string[] = [];
    globalThis.fetch = ((input: string | URL | Request) => {
      paths.push(String(input));
      return Promise.resolve(Response.json([]));
    }) as typeof fetch;

    await Promise.all([
      loadRepositoryIssues("acme", "widgets"),
      loadRepositoryIssues("acme", "widgets"),
    ]);

    assert.equal(paths.length, 1);
    assert.equal(paths[0], "/api/v1/repositories/acme/widgets/issues");
  });

  test("invalidating a repository forces a new request", async () => {
    let requestCount = 0;
    globalThis.fetch = (() => {
      requestCount += 1;
      return Promise.resolve(Response.json([]));
    }) as typeof fetch;

    await loadRepositoryIssues("acme", "widgets");
    clearRepositoryDataCache("acme", "widgets", ["issues"]);
    await loadRepositoryIssues("acme", "widgets");

    assert.equal(requestCount, 2);
  });

  test("preloads every default tab dataset without surfacing failures", async () => {
    const paths: string[] = [];
    globalThis.fetch = ((input: string | URL | Request) => {
      paths.push(String(input));
      return Promise.resolve(Response.json(null, { status: 500 }));
    }) as typeof fetch;

    await preloadRepositoryData("acme", "widgets", "main", true);

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
