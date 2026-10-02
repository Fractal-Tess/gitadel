import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  parseSearchQuery,
  score,
} from "../src/lib/components/app/command-palette-search.js";

function repository(path: string, description = "") {
  return {
    normalizedName: path.split("/").at(-1) ?? path,
    normalizedPath: path,
    normalizedDescription: description,
  };
}

describe("parseSearchQuery", () => {
  test("lowercases and splits terms on whitespace", () => {
    assert.deepEqual(parseSearchQuery("  Acme   Widgets "), {
      needle: "acme widgets",
      terms: ["acme", "widgets"],
      topics: [],
    });
  });

  test("separates topic filters from free text", () => {
    assert.deepEqual(parseSearchQuery("topic:Rust cli topic:tui"), {
      needle: "cli",
      terms: ["cli"],
      topics: ["rust", "tui"],
    });
  });

  test("keeps a bare topic prefix as an ordinary term", () => {
    assert.deepEqual(parseSearchQuery("topic:"), {
      needle: "topic:",
      terms: ["topic:"],
      topics: [],
    });
  });

  test("an empty query has no terms", () => {
    assert.deepEqual(parseSearchQuery("   "), {
      needle: "",
      terms: [],
      topics: [],
    });
  });
});

describe("score", () => {
  const widgets = repository("acme/widgets", "tools for building things");

  test("matches everything when nothing was typed", () => {
    assert.equal(score(widgets, []), 1);
  });

  test("ranks a name prefix above a substring above scattered letters", () => {
    const prefix = score(widgets, ["wid"]);
    const substring = score(widgets, ["dget"]);
    const scattered = score(widgets, ["wgt"]);
    assert.ok(prefix > substring, "prefix beats substring");
    assert.ok(substring > scattered, "substring beats subsequence");
    assert.ok(scattered > 0, "subsequence still matches");
  });

  test("ranks a name match above a description match", () => {
    assert.ok(score(widgets, ["widgets"]) > score(widgets, ["tools"]));
  });

  test("requires every term to match somewhere", () => {
    assert.equal(score(widgets, ["widgets", "zzz"]), 0);
    assert.ok(score(widgets, ["acme", "widgets"]) > 0);
  });

  test("adds to a match on the path", () => {
    assert.equal(score(widgets, ["acme/widgets"]), 80 + 20);
    assert.equal(score(widgets, ["widgets"]), 100 + 20);
  });

  test("does not match letters that appear out of order", () => {
    assert.equal(score(repository("abc"), ["cb"]), 0);
  });
});
