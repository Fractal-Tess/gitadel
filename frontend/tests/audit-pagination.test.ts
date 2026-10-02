import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  AUDIT_PAGE_SIZE,
  appendAuditPage,
  firstAuditPage,
} from "../src/lib/settings/audit-pagination.js";

function events(count: number, from = 0): { id: string }[] {
  return Array.from({ length: count }, (_, index) => ({
    id: String(from + index),
  }));
}

describe("firstAuditPage", () => {
  test("offers more only after a full page", () => {
    assert.equal(firstAuditPage(events(AUDIT_PAGE_SIZE)).hasMore, true);
    assert.equal(firstAuditPage(events(AUDIT_PAGE_SIZE - 1)).hasMore, false);
    assert.equal(firstAuditPage(events(0)).hasMore, false);
  });
});

describe("appendAuditPage", () => {
  test("appends older events and keeps offering more after a full page", () => {
    const next = appendAuditPage(events(100), events(100, 100));
    assert.equal(next.events.length, 200);
    assert.equal(next.hasMore, true);
  });

  test("ends the list on a short page", () => {
    const next = appendAuditPage(events(100), events(12, 100));
    assert.equal(next.events.length, 112);
    assert.equal(next.hasMore, false);
  });

  test("ends the list on an empty page", () => {
    const next = appendAuditPage(events(100), []);
    assert.equal(next.events.length, 100);
    assert.equal(next.hasMore, false);
  });

  test("stops without repeating when the server ignores the offset", () => {
    const loaded = events(100);
    const next = appendAuditPage(loaded, events(100));
    assert.deepEqual(next.events, loaded);
    assert.equal(next.hasMore, false);
  });

  test("skips events shifted onto the page by newer activity", () => {
    const next = appendAuditPage(events(100), events(100, 97));
    assert.equal(next.events.length, 197);
    assert.equal(new Set(next.events.map((event) => event.id)).size, 197);
    assert.equal(next.hasMore, true);
  });
});
