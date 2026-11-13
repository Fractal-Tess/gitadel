import assert from "node:assert/strict";
import { describe, test } from "node:test";

function expect(actual: unknown) {
  return {
    toEqual(expected: unknown) {
      assert.deepEqual(actual, expected);
    },
    toBe(expected: unknown) {
      assert.equal(actual, expected);
    },
    toBeNull() {
      assert.equal(actual, null);
    },
    toContain(expected: string) {
      assert.ok(typeof actual === "string" && actual.includes(expected));
    },
  };
}
import {
  codePermalink,
  highlightedLines,
  markdownSnippet,
  parseLineRange,
  readPreferences,
  searchMarkup,
  sourceLines,
} from "../src/lib/repository/code-viewer";

describe("source line sharing", () => {
  test("counts logical lines without creating a phantom trailing line", () => {
    expect(sourceLines("")).toEqual([]);
    expect(sourceLines("a\r\nb\r\n")).toEqual(["a", "b"]);
    expect(sourceLines("\n")).toEqual([""]);
    expect(sourceLines("a\n\n")).toEqual(["a", ""]);
  });
  test("accepts hashes and typed ranges while rejecting invalid lines", () => {
    expect(parseLineRange("#L2-L4", 8)).toEqual({ start: 2, end: 4 });
    expect(parseLineRange("3", 8)).toEqual({ start: 3, end: 3 });
    for (const value of ["0", "3-2", "9", "2-9", "1.5", "#LNaN"])
      expect(parseLineRange(value, 8)).toBeNull();
  });
  test("pins links to a commit and safely encodes paths", () => {
    const url = new URL(
      codePermalink(
        "https://gitadel.test",
        "owner",
        "repo",
        "abc123",
        "src/has #?.ts",
        { start: 2, end: 4 },
      ),
    );
    expect(url.searchParams.get("path")).toBe("src/has #?.ts");
    expect(url.searchParams.get("rev")).toBe("abc123");
    expect(url.hash).toBe("#L2-L4");
    expect(url.searchParams.get("plain")).toBe("1");
  });
  test("balances syntax spans across newlines", () => {
    expect(
      highlightedLines('<span class="hljs-comment">first\nsecond</span>\n', 2),
    ).toEqual([
      '<span class="hljs-comment">first</span>',
      '<span class="hljs-comment">second</span>',
    ]);
  });
  test("escapes untrusted code and searches literals instead of regular expressions", () => {
    expect(searchMarkup("<script>.*</script>", ".*")).toBe(
      "&lt;script&gt;<mark>.*</mark>&lt;/script&gt;",
    );
    expect(searchMarkup("a&b", "&")).toBe("a<mark>&amp;</mark>b");
  });
  test("Markdown snippets cannot terminate their own code fence", () => {
    expect(
      markdownSnippet("```\nalert(1)", "js", "https://gitadel.test"),
    ).toContain("````js\n```\nalert(1)\n````");
  });
  test("handles unavailable or corrupt saved preferences", () => {
    expect(readPreferences("oops")).toEqual({
      wrap: false,
      fontSize: 12,
      tabSize: 4,
    });
    expect(readPreferences('{"wrap":true,"fontSize":100,"tabSize":1}')).toEqual(
      { wrap: true, fontSize: 24, tabSize: 2 },
    );
  });
});
