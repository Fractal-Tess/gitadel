import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  LIST_PAGE_SIZE,
  appendPage,
  isFullPage,
  loadPages,
} from "../src/lib/repository/pagination.js";

function items(count: number, from = 0): { id: string }[] {
  return Array.from({ length: count }, (_, index) => ({
    id: String(from + index),
  }));
}

/** A server that honours `page`, holding `total` items. */
function paginating(total: number) {
  const requested: number[] = [];
  const all = items(total);
  return {
    requested,
    load: async (page: number) => {
      requested.push(page);
      return all.slice((page - 1) * LIST_PAGE_SIZE, page * LIST_PAGE_SIZE);
    },
  };
}

/** A server that ignores `page` and always answers with everything. */
function ignoring(total: number) {
  const requested: number[] = [];
  return {
    requested,
    load: async (page: number) => {
      requested.push(page);
      return items(total);
    },
  };
}

describe("isFullPage", () => {
  test("only an exactly full page promises another", () => {
    assert.equal(isFullPage(items(LIST_PAGE_SIZE)), true);
    assert.equal(isFullPage(items(LIST_PAGE_SIZE - 1)), false);
    assert.equal(isFullPage(items(0)), false);
    assert.equal(isFullPage(items(LIST_PAGE_SIZE + 1)), false);
  });
});

describe("loadPages", () => {
  test("loads one page and reports that more follow", async () => {
    const server = paginating(250);
    const loaded = await loadPages(server.load);
    assert.equal(loaded.items.length, 100);
    assert.equal(loaded.pages, 1);
    assert.equal(loaded.hasMore, true);
    assert.deepEqual(server.requested, [1]);
  });

  test("reloads as many pages as were on screen", async () => {
    const server = paginating(250);
    const loaded = await loadPages(server.load, 3);
    assert.equal(loaded.items.length, 250);
    assert.equal(loaded.pages, 3);
    assert.equal(loaded.hasMore, false);
    assert.deepEqual(server.requested, [1, 2, 3]);
  });

  test("stops early when the list became shorter", async () => {
    const server = paginating(40);
    const loaded = await loadPages(server.load, 3);
    assert.equal(loaded.items.length, 40);
    assert.equal(loaded.pages, 1);
    assert.equal(loaded.hasMore, false);
    assert.deepEqual(server.requested, [1]);
  });

  test("an empty list is complete", async () => {
    const loaded = await loadPages(paginating(0).load);
    assert.deepEqual(loaded, { items: [], pages: 0, hasMore: false });
  });

  test("treats an unpaginated answer longer than a page as complete", async () => {
    const server = ignoring(250);
    const loaded = await loadPages(server.load, 2);
    assert.equal(loaded.items.length, 250);
    assert.equal(loaded.hasMore, false);
    assert.deepEqual(server.requested, [1]);
  });
});

describe("appendPage", () => {
  test("appends the next page", async () => {
    const server = paginating(150);
    const first = await loadPages(server.load);
    const second = appendPage(first, await server.load(2));
    assert.equal(second.items.length, 150);
    assert.equal(second.pages, 2);
    assert.equal(second.hasMore, false);
  });

  test("skips items that are already loaded", () => {
    const first = { items: items(100), pages: 1, hasMore: true };
    // An item created meanwhile shifts the last loaded one onto page two.
    const second = appendPage(first, items(100, 99));
    assert.equal(second.items.length, 199);
    assert.equal(new Set(second.items.map((item) => item.id)).size, 199);
    assert.equal(second.hasMore, true);
  });

  test("ends the list when an unpaginated server repeats page one", async () => {
    const server = ignoring(LIST_PAGE_SIZE);
    const first = await loadPages(server.load);
    assert.equal(first.hasMore, true);
    const second = appendPage(first, await server.load(2));
    assert.equal(second.items.length, LIST_PAGE_SIZE);
    assert.equal(second.pages, 1);
    assert.equal(second.hasMore, false);
  });
});
