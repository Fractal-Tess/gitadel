import assert from "node:assert/strict";
import { describe, test } from "node:test";

import { isServerRoute, safeReturnTo } from "../src/lib/return-to.js";

describe("safeReturnTo", () => {
  test("keeps paths on this origin", () => {
    for (const path of [
      "/",
      "/settings",
      "/-/account/profile",
      "/acme/widgets?view=issues&issue=4",
      "/acme/widgets?view=registry",
      "/acme/members",
      "/login/oauth/authorize?client_id=abc&redirect_uri=https%3A%2F%2Fapp.example%2Fcb",
      "/acme/widgets?next=https://example.com/#readme",
    ]) {
      assert.equal(safeReturnTo(path), path);
    }
  });

  test("falls back to the front page when nothing usable is given", () => {
    for (const value of [null, undefined, "", "settings", "?view=issues"]) {
      assert.equal(safeReturnTo(value), "/");
    }
  });

  test("rejects targets that leave this origin", () => {
    for (const value of [
      "https://evil.example/",
      "http://evil.example",
      "//evil.example",
      "//evil.example/path",
      "///evil.example",
      "/\\evil.example",
      "/\\/evil.example",
      "\\\\evil.example",
      "/path\\..\\//evil.example",
      "/\t/evil.example",
      "/\n/evil.example",
      "/\r/evil.example",
      "\t//evil.example",
      " //evil.example",
      "javascript:alert(1)",
      "data:text/html,<script>alert(1)</script>",
    ]) {
      assert.equal(safeReturnTo(value), "/", JSON.stringify(value));
    }
  });

  test("does not return to the sign-in page", () => {
    assert.equal(safeReturnTo("/login"), "/");
    assert.equal(safeReturnTo("/login?returnTo=%2Fsettings"), "/");
  });
});

describe("isServerRoute", () => {
  test("only matches the OAuth authorize endpoint", () => {
    assert.equal(isServerRoute("/login/oauth/authorize?client_id=a"), true);
    assert.equal(isServerRoute("/login/oauth/authorize"), false);
    assert.equal(isServerRoute("/acme/widgets?view=issues"), false);
  });
});
