import assert from "node:assert/strict";
import { test } from "node:test";
import { summarizeActivityDay } from "../src/lib/profile-activity.js";

test("activity preview ranks repositories and authors without changing the response", () => {
  const commit = (author_name: string, timestamp: number) => ({
    oid: "abc",
    short_oid: "abc",
    title: "Change",
    author_name,
    timestamp,
  });
  const repositories = [
    { namespace: "team", name: "small", commits: [commit(" Tess ", 300)] },
    {
      namespace: "team",
      name: "large",
      commits: [commit("Tess", 100), commit("", 200)],
    },
  ];
  const summary = summarizeActivityDay({
    date: "2026-11-13",
    total_commits: 3,
    repositories,
  });
  assert.deepEqual(
    summary.repositories.map((repo) => repo.name),
    ["large", "small"],
  );
  assert.equal(repositories[0].name, "small");
  assert.deepEqual(summary.authors, [
    { name: "Tess", count: 2 },
    { name: "Unknown author", count: 1 },
  ]);
  assert.equal(summary.first, 100);
  assert.equal(summary.last, 300);
});

test("empty activity has no fabricated author or time range", () => {
  assert.deepEqual(
    summarizeActivityDay({
      date: "2026-11-13",
      total_commits: 0,
      repositories: [],
    }),
    {
      repositories: [],
      authors: [],
      first: null,
      last: null,
    },
  );
});
