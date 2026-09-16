import assert from "node:assert/strict";
import { describe, test } from "node:test";

import { submoduleLinks } from "../src/lib/repository/submodule-links.js";

const SHA1 = "0123456789abcdef0123456789abcdef01234567";
const SHA256 = "0123456789abcdef".repeat(4);
const parent = "https://gitadel.example/owner/parent.git";

describe("submoduleLinks", () => {
  test("links a GitHub HTTPS clone and its pinned commit", () => {
    assert.deepEqual(
      submoduleLinks("https://github.com/owner/chorus.git", parent, SHA1),
      {
        repositoryUrl: "https://github.com/owner/chorus",
        commitUrl: `https://github.com/owner/chorus/commit/${SHA1}`,
        label: "owner/chorus",
        host: "github.com",
      },
    );
  });

  test("resolves a sibling relative URL using repository-directory semantics", () => {
    assert.deepEqual(submoduleLinks("../sibling.git", parent, SHA1), {
      repositoryUrl: "https://gitadel.example/owner/sibling",
      commitUrl: `https://gitadel.example/owner/sibling?view=commit&oid=${SHA1}`,
      label: "owner/sibling",
      host: "gitadel.example",
    });
  });

  test("maps known provider SSH and scp clone URLs", () => {
    assert.equal(
      submoduleLinks("git@github.com:owner/ssh-repo.git", parent, SHA256)
        .commitUrl,
      `https://github.com/owner/ssh-repo/commit/${SHA256}`,
    );
    assert.equal(
      submoduleLinks("ssh://git@gitlab.com/group/repo.git", parent, SHA1)
        .commitUrl,
      `https://gitlab.com/group/repo/-/commit/${SHA1}`,
    );
    assert.equal(
      submoduleLinks("git@bitbucket.org:team/repo.git", parent, SHA1).commitUrl,
      `https://bitbucket.org/team/repo/commits/${SHA1}`,
    );
  });

  test("uses Gitadel's commit query only for a matching same-origin remote", () => {
    assert.equal(
      submoduleLinks(
        "ssh://git@gitadel.example:2222/owner/child.git",
        parent,
        SHA1,
      ).commitUrl,
      `https://gitadel.example/owner/child?view=commit&oid=${SHA1}`,
    );
    assert.equal(
      submoduleLinks(
        "ssh://git@other.example:2222/owner/child.git",
        parent,
        SHA1,
      ).repositoryUrl,
      null,
    );
  });

  test("provides arbitrary HTTP repositories without guessing commit routes", () => {
    assert.deepEqual(
      submoduleLinks("https://code.example/team/repo.git", parent, SHA1),
      {
        repositoryUrl: "https://code.example/team/repo",
        commitUrl: null,
        label: "team/repo",
        host: "code.example",
      },
    );
  });

  test("rejects unsafe protocols, credentials, traversal, and missing inputs", () => {
    for (const raw of [
      "javascript:alert(1)",
      "file:///tmp/repo.git",
      "data:text/plain,repo",
      "//evil.example/repo.git",
      "https://user:password@example.com/repo.git",
      "https://code.example/team/%2e%2e/secret.git",
    ]) {
      assert.deepEqual(submoduleLinks(raw, parent, SHA1), {
        repositoryUrl: null,
        commitUrl: null,
        label: null,
        host: null,
      });
    }
    assert.deepEqual(submoduleLinks(null, parent, SHA1), {
      repositoryUrl: null,
      commitUrl: null,
      label: null,
      host: null,
    });
    assert.equal(
      submoduleLinks("https://github.com/owner/repo.git", parent, "main")
        .commitUrl,
      null,
    );
  });
});
