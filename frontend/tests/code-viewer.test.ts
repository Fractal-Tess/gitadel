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
  blameHunkStarts,
  codePermalink,
  compactAge,
  headingSlugger,
  isLineHash,
  markMatches,
  highlightedLines,
  markdownSnippet,
  parseLineRange,
  readPreferences,
  searchMarkup,
  sourceLines,
} from "../src/lib/repository/code-viewer";
import {
  websiteHost,
  websiteProblem,
  withWebsiteScheme,
} from "../src/lib/repository/websites";

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
  test("rendered links can drop plain and carry a heading hash", () => {
    const url = new URL(
      codePermalink("https://g.test", "o", "r", "abc", "README.md", null, {
        plain: false,
        hash: "#user-content-install",
      }),
    );
    expect(url.searchParams.has("plain")).toBe(false);
    expect(url.hash).toBe("#user-content-install");
  });
  test("only line hashes switch a rendered document to its source", () => {
    expect(isLineHash("#L4")).toBe(true);
    expect(isLineHash("#L4-L9")).toBe(true);
    expect(isLineHash("#L4-9")).toBe(true);
    expect(isLineHash("#user-content-install")).toBe(false);
    expect(isLineHash("#Licence")).toBe(false);
    expect(isLineHash("")).toBe(false);
  });
});

describe("search marks inside highlighted markup", () => {
  test("keeps syntax spans and marks case-insensitively", () => {
    expect(
      markMatches('<span class="k">return</span> Value', "value"),
    ).toBe('<span class="k">return</span> <mark>Value</mark>');
  });
  test("splits a match that crosses a span boundary", () => {
    expect(markMatches('<span class="s">&quot;hi</span>&quot;', 'hi"')).toBe(
      '<span class="s">&quot;<mark>hi</mark></span><mark>&quot;</mark>',
    );
  });
  test("matches decoded entities without breaking them", () => {
    expect(markMatches("a &lt;b&gt; &amp; c", "<b>")).toBe(
      "a <mark>&lt;b&gt;</mark> &amp; c",
    );
    expect(markMatches("x &amp;&amp; y", "&")).toBe(
      "x <mark>&amp;&amp;</mark> y",
    );
  });
  test("treats the query literally and leaves unmatched markup alone", () => {
    expect(markMatches("a.*b", ".*")).toBe("a<mark>.*</mark>b");
    expect(markMatches('<span class="c">x</span>', "zzz")).toBe(
      '<span class="c">x</span>',
    );
    expect(markMatches("😀 ok", "ok")).toBe("😀 <mark>ok</mark>");
  });
});

describe("blame and document helpers", () => {
  test("starts a blame hunk where the commit changes", () => {
    expect([
      ...blameHunkStarts([
        { line: 3, commit_oid: "b" },
        { line: 1, commit_oid: "a" },
        { line: 2, commit_oid: "a" },
        { line: 4, commit_oid: "a" },
      ]),
    ]).toEqual([1, 3, 4]);
  });
  test("formats compact ages", () => {
    expect(compactAge(1000, 1030)).toBe("now");
    expect(compactAge(0, 3 * 86_400)).toBe("3d");
    expect(compactAge(0, 400 * 86_400)).toBe("1y");
  });
  test("heading anchors remain unique when a title already includes a suffix", () => {
    const slug = headingSlugger();
    expect([slug("Intro"), slug("Intro-1"), slug("Intro")]).toEqual(["intro", "intro-1", "intro-2"]);
  });
  test("slugs headings like GitHub and numbers repeats", () => {
    const slug = headingSlugger();
    expect(slug("Getting Started!")).toBe("getting-started");
    expect(slug("Getting started")).toBe("getting-started-1");
    expect(slug("API: v2.0 & más")).toBe("api-v20--más");
  });
});

describe("website links", () => {
  test("adds https to bare hosts and reports the host", () => {
    expect(withWebsiteScheme(" docs.example.com/x ")).toBe(
      "https://docs.example.com/x",
    );
    expect(withWebsiteScheme("http://a.test")).toBe("http://a.test");
    expect(withWebsiteScheme("localhost:3030/demo")).toBe("https://localhost:3030/demo");
    expect(withWebsiteScheme("example.com:8443/docs")).toBe("https://example.com:8443/docs");
    expect(websiteHost("example.com/demo")).toBe("example.com");
    expect(websiteHost("javascript:alert(1)")).toBe("");
  });
  test("explains links the server would reject", () => {
    expect(websiteProblem("", "", [])).toBe("");
    expect(websiteProblem("", "Docs", [])).toContain("Add a URL");
    expect(websiteProblem("ftp://a.test", "", [])).toContain("http");
    expect(websiteProblem("https://u:p@a.test", "", [])).toContain(
      "username",
    );
    expect(websiteProblem("a.test", "", ["https://a.test/"])).toContain(
      "already",
    );
    expect(websiteProblem("https://a.test/docs", "", [])).toBe("");
  });
});
