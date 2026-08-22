# Minimal Git forge feature landscape

**Research date:** 2026-08-22

**Scope:** reliable self-hosted Git hosting, archiving, and distribution without pull requests, issues, social features, or browser editing

## Executive recommendation

Gitadel should remain a **repository server with a good read-only web view**, not become a collaboration suite. The highest-priority transport gap is authorized Smart HTTP push. The implementation audit found that the live working tree already authenticates private clone/fetch through scoped bearer or Basic-password tokens; that behavior should be documented and covered by regression tests rather than rebuilt. In the same milestone, add `receive-pack` and prove Git protocol v2 and shallow-clone behavior over both HTTP and SSH.

The next baseline is lifecycle and recoverability: explicit default-branch/`HEAD` management; atomic rename and transfer; reversible read-only archive; delayed delete and restore; one-shot import; a portable per-repository export; and an automated clean-host restore drill. Existing offline backup support is not enough unless restored refs, Git objects, LFS objects, metadata, and permissions are verified.

For automation, add read-only-by-default deploy keys and a documented, paginated subset of the Gitea API. Add signed, retryable webhooks next, not arbitrary server-side hooks. For operations, add bounded resource use, authentication and Git HTTP rate limits, scheduled integrity checks, health/readiness endpoints, and a small Prometheus surface.

For distribution, add immutable-revision raw-file and source-archive URLs plus paginated repository/branch/tag listings. Full code search, feeds, recurring mirrors, partial clone, and signature verification are useful second-stage features. Releases are not yet justified: tags, raw files, LFS, and source archives cover the minimal source-distribution case without introducing another mutable artifact model.

## Starting point and decision rule

The supplied baseline already includes:

- public/private repositories and namespaces;
- SSH push and clone, plus Smart HTTP fetch (including token-authenticated private fetch in the audited working tree);
- Git LFS and locks;
- branch, tag, history, diff, Markdown, highlighting, and language-stat views;
- passwords, passkeys, sessions, API tokens, invites, SSH keys, and OAuth;
- organizations, collaborators, and an audit log;
- offline backups; and
- Gitea-compatible APIs needed by Dokploy.

Those are treated as assumptions, not re-recommended work. A feature belongs in Gitadel only if it materially improves at least one of:

1. **transport completeness** — a normal Git client can read and write through expected secure transports;
2. **repository continuity** — repositories can be moved, frozen, recovered, and exported without losing identity or refs;
3. **operational safety** — corruption, abuse, exhaustion, and failed dependencies can be detected and contained;
4. **tool interoperability** — machines can use least-privilege credentials and stable, documented interfaces; or
5. **source discovery/distribution** — people and scripts can find and retrieve source without joining a collaboration workflow.

## Evidence from primary sources

This section records product behavior; recommendations follow separately. It does **not** imply that every surveyed product has every listed capability.

### Native Git defines the transport and recovery floor

- Git's `git-http-backend` supports Smart HTTP fetch and push. It enables `upload-pack` by default and enables `receive-pack` by default for a web-server-authenticated user. It also documents the important split between anonymous read and authenticated write. It warns that the legacy `http.getanyfile` service can expose unreachable objects still present in a repository. ([Git `git-http-backend`](https://git-scm.com/docs/git-http-backend))
- Protocol v2 is requested over HTTP with `Git-Protocol: version=2`; over SSH/file transports, `GIT_PROTOCOL` must be passed with `version=2`. A server or proxy that drops these values silently loses v2 negotiation. ([Git protocol v2](https://git-scm.com/docs/protocol-v2))
- Git clone supports shallow history with `--depth`, `--shallow-since`, and `--shallow-exclude`. Partial clone is distinct: `--filter` requests a subset of reachable objects, and omitted objects may later be fetched from a promisor remote. ([Git `clone`](https://git-scm.com/docs/git-clone), [Git partial clone design](https://git-scm.com/docs/partial-clone))
- A clone's initial branch comes from the cloned repository's active branch/`HEAD`; `--branch` overrides the branch pointed to by the remote `HEAD`. A stale or invalid server `HEAD` therefore directly affects normal clone behavior. ([Git `clone`](https://git-scm.com/docs/git-clone))
- Git bundles are intended for offline transfer and can contain all refs for a full Git-data backup; `git bundle verify` checks format and prerequisites. Git explicitly cautions that a bundle does **not** preserve non-ref state such as hooks and per-repository configuration. ([Git `bundle`](https://git-scm.com/docs/git-bundle))
- `git fsck` verifies object connectivity and validity and reports missing or corrupt objects. It is an integrity check, not a repair source; corrupt objects must come from another copy or backup. ([Git `fsck`](https://git-scm.com/docs/git-fsck))

### Forgejo and Gitea expose useful operational patterns

- Gitea implements periodic pull and push mirrors with manual synchronization. Its documentation says pull mirrors copy commit, tag, and branch history, and warns that push mirroring force-pushes and can overwrite remote changes. ([Gitea repository mirrors](https://docs.gitea.com/usage/repository/repo-mirror/))
- Gitea's backup documentation says the instance must be stopped for a consistent backup because database and repository changes can otherwise race. Its dump contains configuration/customization, the data directory including LFS, repositories, and a database dump; restore is manual and may require regenerated Git hooks and a doctor check. ([Gitea backup and restore](https://docs.gitea.com/administration/backup-and-restore/))
- Gitea publishes an OpenAPI document and Swagger UI. Its API uses `page`/`limit`, returns RFC-style `Link` pagination and `x-total-count`, and supports scoped access tokens. ([Gitea API usage](https://docs.gitea.com/development/api-usage/))
- Gitea supports TOTP and WebAuthn MFA, but its docs note that Git-over-HTTP CLI operations cannot perform interactive MFA; when MFA is enabled, a token is used in place of a password. ([Gitea MFA](https://docs.gitea.com/usage/user-setting/multi-factor-authentication/))
- Gitea webhooks have branch filters, push/custom event selection, delivery inspection and replay; instance settings can constrain destination hosts, timeouts, and cleanup. ([Gitea webhooks](https://docs.gitea.com/usage/repository/webhooks))
- Forgejo exposes a scheduled repository health check backed by `git fsck`, a token-protected Prometheus endpoint, a default-branch setting, and source-archive downloads. The same configuration reference permits blocking query-string API tokens and constraining migration destinations. ([Forgejo configuration](https://forgejo.org/docs/latest/admin/config-cheat-sheet/))
- Forgejo disables user-created custom Git hooks by default and warns that enabling them permits arbitrary code execution as the Forgejo operating-system user, including access to configuration and the database. ([Forgejo configuration, security section](https://forgejo.org/docs/latest/admin/config-cheat-sheet/#security-security))
- Forgejo's CLI has repository dump and restore commands, with optional restore validation, alongside instance doctor and dump commands. ([Forgejo CLI](https://forgejo.org/docs/latest/admin/command-line/))

### GitLab demonstrates lifecycle, machine access, and controls

- GitLab's Projects API has explicit archive/unarchive, delete, restore-from-pending-deletion, namespace transfer, and default-branch fields. These are repository lifecycle operations independent of merge requests or issues. ([GitLab Projects API](https://docs.gitlab.com/api/projects/))
- GitLab supports pull and push repository mirrors and describes them as ways to retain an old home, publish a copy, or copy commits, tags, and branches from an external canonical repository. ([GitLab repository mirroring](https://docs.gitlab.com/user/project/repository/mirror/))
- GitLab file exports are portable in offline environments and include repositories, but GitLab explicitly says project export files should not be treated as backups because not every item is exported. Its documented import compatibility is also version-bounded. ([GitLab project file export/import](https://docs.gitlab.com/user/project/settings/import_export/))
- GitLab deploy keys are intended for non-human repository access and can be read-only or explicitly read-write, revocable, and expiring. Its documentation also highlights ownership/lifecycle pitfalls when keys remain tied to a user. ([GitLab deploy keys](https://docs.gitlab.com/user/project/deploy_keys/))
- GitLab's configurable throttles distinguish authenticated and unauthenticated web, API, Git HTTP, and Git LFS traffic; rate-limited HTTP responses use `429`, `RateLimit-*`, and `Retry-After` headers. ([GitLab user and IP rate limits](https://docs.gitlab.com/administration/settings/user_and_ip_rate_limits/))
- GitLab has separate health, readiness, liveness, and dependency checks. Its comprehensive check covers database and Redis, while readiness can additionally validate dependencies such as Gitaly. ([GitLab health checks](https://docs.gitlab.com/administration/monitoring/health_check/))
- GitLab verifies SSH, GPG, and X.509 signatures on commits and tags and distinguishes signed content from signatures associated with a known user key. ([GitLab signed commits](https://docs.gitlab.com/user/project/repository/signed_commits/))

### Minimal servers show what can stay small

- cgit offers source snapshots in tar and zip variants, clone URL presentation, configurable repository-index pagination, name/recency sorting, README/about pages, and optional history graphs without adding collaboration objects. ([cgit `cgitrc(5)` source](https://git.zx2c4.com/cgit/tree/cgitrc.5.txt))
- gitweb supports repository browsing, commit-message search, and RSS/Atom commit feeds; it keeps blame optional and disables it by default for performance reasons. ([Git `gitweb`](https://git-scm.com/docs/gitweb), [Git `gitweb.conf`](https://git-scm.com/docs/gitweb.conf))
- Soft Serve is especially relevant because its small surface still includes SSH and HTTP Git transport, token-authenticated HTTP access to private repositories, raw file output, public/private repositories, import and pull mirrors, rename/delete, and repository webhooks. ([Soft Serve README](https://github.com/charmbracelet/soft-serve/blob/main/README.md))
- Gitolite sits on standard Git/OpenSSH (or an authenticating HTTP server), authorizes before invoking `git-receive-pack`, and can apply access rules at branch/tag ref level. It illustrates that protected refs are an authorization concern, not necessarily a code-review feature. ([Gitolite overview](https://gitolite.com/gitolite/overview.html))
- Radicle stores bare Git repositories, replicates them between seeders, gives repositories stable IDs independent of location, and signs refs so a clone can authenticate repository state. This is evidence for location-independent identity and independently verifiable copies, not a recommendation that Gitadel adopt peer-to-peer networking. ([Radicle protocol guide](https://radicle.xyz/guides/protocol))

## Prioritized matrix

| Priority                   | Gap or parity target                                                                                                | Minimal acceptance boundary                                                                                                                                                                                   | Why it belongs here                                                                                                                                                                                                                                                                                                                                             |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Must / P0**              | Smart HTTP write parity                                                                                             | Preserve and regression-test private `ls-remote`/clone/fetch; add authorized push using scoped write tokens; no query-string tokens; consistent private-repo non-disclosure                                   | Closes the remaining transport gap and gives CI/deploy tools an HTTPS write path without weakening MFA. Git itself defines authenticated Smart HTTP push behavior. ([Git HTTP backend](https://git-scm.com/docs/git-http-backend), [Gitea MFA](https://docs.gitea.com/usage/user-setting/multi-factor-authentication/))                                         |
| **Must / P0**              | Protocol v2 and shallow parity                                                                                      | Preserve `Git-Protocol` over HTTP and `GIT_PROTOCOL` over SSH; test v0 fallback, v2, depth/since/exclude, deepen and unshallow on public/private repos                                                        | This is wire-protocol correctness, not a new product subsystem. ([Git protocol v2](https://git-scm.com/docs/protocol-v2), [Git clone](https://git-scm.com/docs/git-clone))                                                                                                                                                                                      |
| **Must / P0**              | Lifecycle state machine                                                                                             | Set/repair default branch and `HEAD`; atomic rename and namespace transfer; reversible read-only archive; delayed delete, restore, and explicit purge; stable internal repository ID and old-path redirects   | Prevents normal administration from breaking clones or turning one mistake into data loss. GitLab exposes the same lifecycle as repository APIs, while Soft Serve proves rename/delete fit a small server. ([GitLab Projects API](https://docs.gitlab.com/api/projects/), [Soft Serve README](https://github.com/charmbracelet/soft-serve/blob/main/README.md)) |
| **Must / P0**              | Restore proof, not merely backup creation                                                                           | Versioned manifest; database/config/secrets/repositories/LFS/locks coverage; checksums; clean-host restore; ref and `HEAD` comparison; `git fsck --strict`; LFS SHA-256 verification; scheduled restore drill | A backup is operationally useful only when its consistency and restoration are demonstrated. ([Gitea backup and restore](https://docs.gitea.com/administration/backup-and-restore/), [Git fsck](https://git-scm.com/docs/git-fsck))                                                                                                                             |
| **Must / P0**              | Portable repository ingress/egress                                                                                  | One-shot import by HTTPS/SSH; mirror-clone-compatible ref advertisement; admin CLI export containing a full Git bundle, separate metadata, `HEAD`, and referenced LFS objects; verification command           | Makes archives recoverable outside Gitadel. A bundle covers Git refs/objects but must be paired with non-Git state. ([Git bundle](https://git-scm.com/docs/git-bundle), [Forgejo repository dump/restore](https://forgejo.org/docs/latest/admin/command-line/#dump-repo))                                                                                       |
| **Must / P0**              | Least-privilege machine access                                                                                      | Repository-scoped deploy keys; read-only default; explicit write; expiry/revocation; last-used timestamp; audit events; no dependence on an employee account                                                  | Machine clone is core hosting interoperability. GitLab's documented ownership pitfalls argue for a first-class service credential rather than pretending every machine is a user. ([GitLab deploy keys](https://docs.gitlab.com/user/project/deploy_keys/))                                                                                                     |
| **Must / P0**              | Minimal protected refs                                                                                              | Protect the default branch and selected tag/branch patterns from deletion and force-push; explicit owner/admin override; no approvals, reviewers, or CODEOWNERS                                               | This protects archive integrity without implementing a review workflow. Gitolite applies authorization directly to refs. ([Gitolite overview](https://gitolite.com/gitolite/overview.html))                                                                                                                                                                     |
| **Must / P0**              | Operational guardrails                                                                                              | Rate-limit sign-in/token/Git HTTP/LFS paths; connection and request limits; repository/LFS quotas; scheduled `fsck` and maintenance; liveness/readiness/dependency health; small protected metrics endpoint   | These controls prevent resource exhaustion and make corruption or dependency failure visible. ([GitLab rate limits](https://docs.gitlab.com/administration/settings/user_and_ip_rate_limits/), [Forgejo health/metrics configuration](https://forgejo.org/docs/latest/admin/config-cheat-sheet/))                                                               |
| **Must / P1**              | Raw and source distribution                                                                                         | Raw file by ref and immutable OID; tar.gz and zip source archives by OID/tag; authorization parity; size/time bounds; `ETag`/conditional requests; clear clone URLs                                           | This is the browser/download counterpart to clone, and cgit demonstrates it without social scope. ([cgit configuration](https://git.zx2c4.com/cgit/tree/cgitrc.5.txt))                                                                                                                                                                                          |
| **Must / P1**              | Documented API subset                                                                                               | Inventory current Dokploy endpoints; publish OpenAPI; fill only repository lifecycle, branch/tag listing, archive/raw, deploy-key and health gaps; stable errors; `page`/`limit`, `Link`, and total count     | Preserves Gitea interoperability without chasing the full Gitea feature set. ([Gitea API usage](https://docs.gitea.com/development/api-usage/))                                                                                                                                                                                                                 |
| **Next**                   | Signed, retryable webhooks                                                                                          | Push and branch/tag create/delete plus repository lifecycle events; per-hook secret/signature; bounded retries; delivery log/redelivery; destination SSRF policy and timeouts                                 | Enables deployment automation while keeping execution outside Gitadel. Gitea and Soft Serve both treat webhooks as a repository integration. ([Gitea webhooks](https://docs.gitea.com/usage/repository/webhooks), [Soft Serve README](https://github.com/charmbracelet/soft-serve/blob/main/README.md))                                                         |
| **Next**                   | Recurring pull/push mirrors                                                                                         | Pull mirror first; status, last success/error, manual sync, backoff; encrypted credentials; destination allowlist; make force-push semantics explicit; push mirror later                                      | Valuable for migration and off-site copies, but scheduling, credentials, SSRF, divergence, and destructive push behavior make it larger than one-shot import. ([Gitea mirrors](https://docs.gitea.com/usage/repository/repo-mirror/), [GitLab mirrors](https://docs.gitlab.com/user/project/repository/mirror/))                                                |
| **Next**                   | Partial clone                                                                                                       | Support and test `--filter=blob:none` and bounded filters only after measuring large-repository demand; monitor on-demand object fetch failures                                                               | Unlike shallow clone, partial clone deliberately creates clients with missing objects and continued dependence on a promisor remote. ([Git partial clone](https://git-scm.com/docs/partial-clone))                                                                                                                                                              |
| **Next**                   | Signature verification display                                                                                      | Verify SSH and GPG signatures on commits and tags; show valid/invalid/unknown-key distinctly; do not require signatures globally at first                                                                     | Improves archive provenance without creating approvals or a PKI workflow. GitLab's distinction between cryptographic validity and known-user identity is important. ([GitLab signed commits](https://docs.gitlab.com/user/project/repository/signed_commits/))                                                                                                  |
| **Next**                   | Focused discovery                                                                                                   | Paginate/sort public and authorized repository lists; search repository name/description first, then bounded commit-message/path search; optional Atom feed per repository                                    | Discovery is useful for an archive, but a global code index and notification system are not prerequisites. cgit and gitweb show smaller alternatives. ([cgit configuration](https://git.zx2c4.com/cgit/tree/cgitrc.5.txt), [Git gitweb](https://git-scm.com/docs/gitweb))                                                                                       |
| **Next, only if demanded** | Additional MFA method                                                                                               | Enforce existing passkeys for administrators/local accounts and provide recovery first; add TOTP only for operators whose clients or policies require it                                                      | Gitadel already has passkeys. The real gap is enforceability and recovery, while Git CLI HTTP must continue to use tokens rather than interactive MFA. ([Gitea MFA](https://docs.gitea.com/usage/user-setting/multi-factor-authentication/))                                                                                                                    |
| **Avoid**                  | Dumb HTTP and unauthenticated `git://`                                                                              | Do not add them; keep HTTPS Smart HTTP and SSH                                                                                                                                                                | They add transport/security surface without a current interoperability need; legacy dumb HTTP can expose unreachable objects. ([Git HTTP backend](https://git-scm.com/docs/git-http-backend))                                                                                                                                                                   |
| **Avoid**                  | User-supplied server hook scripts                                                                                   | Internal hooks for Gitadel invariants are fine; expose webhooks, not shell execution                                                                                                                          | Arbitrary hooks collapse repository ownership into host code execution. ([Forgejo security configuration](https://forgejo.org/docs/latest/admin/config-cheat-sheet/#security-security))                                                                                                                                                                         |
| **Avoid**                  | PRs, issues, reviews, comments, stars, follows, forks, snippets, wikis, browser editing, CI, and package registries | Keep absent                                                                                                                                                                                                   | None is required to store, recover, browse, clone, or distribute a Git repository.                                                                                                                                                                                                                                                                              |
| **Avoid for now**          | Release objects and binary-asset workflow                                                                           | Use tags, immutable source archives, raw files, and LFS; reconsider only with concrete demand for signed binary bundles or release notes                                                                      | A release database introduces another lifecycle, permission, storage, API, and retention surface; source distribution does not require it.                                                                                                                                                                                                                      |

## Recommended behavior in detail

### 1. Transport parity

Retain the dedicated Git HTTP authentication path already present for `upload-pack`, and extend it to `receive-pack`. Accept a repository-scoped token through the HTTP `Authorization` mechanism that standard Git credential helpers understand; do not accept tokens in query strings. Keep browser cookies and CSRF semantics out of Git RPC authorization. A read token may invoke `upload-pack`; a write token may invoke `receive-pack` after collaborator/ref checks. Password auth can remain optional for accounts without enforced strong authentication, but token auth must be the reliable CLI path.

The acceptance matrix should cover:

- public and private repositories;
- valid read token, valid write token, expired/revoked token, and unrelated-repository token;
- `ls-remote`, clone, fetch, pull, push, force-push rejection, branch/tag creation and deletion;
- SSH and HTTP protocol v2 negotiation plus v0 fallback;
- shallow clone, deepen and unshallow;
- LFS batch/object transfer and lock authorization; and
- indistinguishable unauthorized/not-found behavior for private repositories.

Do not infer protocol v2 support merely because Git subprocesses are used. Assert the advertised protocol in integration tests so proxy/header or SSH-environment regressions are visible.

### 2. Repository lifecycle

Use a small state model:

`active -> archived -> active` and `active|archived -> pending_deletion -> restored|purged`.

An archived repository remains cloneable and browsable but rejects writes, key changes, mirrors, and other mutating API operations. Pending deletion disappears from normal discovery and transport, retains an administrator-visible deletion deadline, and can be restored before purge. Purge is a separate, audited operation.

Store an immutable internal repository ID. Namespace/path is a locator, not identity. Rename and transfer should update routing and permissions atomically, preserve visibility and archive state, and leave bounded redirects for old HTTP and SSH paths. This borrows the continuity property—not the peer-to-peer machinery—of Radicle's location-independent repository ID. ([Radicle protocol guide](https://radicle.xyz/guides/protocol))

Default branch changes must update symbolic `HEAD`. Ref protection must prevent deletion of the current default branch until a replacement is selected. Empty repositories need an explicit unborn default branch so the first push and clone URLs behave predictably.

### 3. Portability and disaster recovery

Keep two separate promises:

1. **Instance recovery** restores Gitadel itself: database, configuration/secrets required to decrypt stored credentials, repositories, LFS objects and locks, users/orgs/permissions, audit data, and relevant token state.
2. **Repository portability** produces a format usable without Gitadel: full Git refs/objects, `HEAD`, LFS objects with OID manifest, and a simple versioned JSON metadata file.

Do not label a Git bundle a complete backup. The Git documentation explicitly excludes hooks and configuration, and LFS objects are outside the Git object database. ([Git bundle](https://git-scm.com/docs/git-bundle)) Likewise, do not label an application export a backup unless its consistency and completeness contract is explicit; GitLab makes that distinction for its own project exports. ([GitLab project export/import](https://docs.gitlab.com/user/project/settings/import_export/))

A restore test should create branches, annotated and lightweight tags, a changed default branch, unreachable-object expectations, LFS content and locks, private permissions, deploy credentials, and an archived and pending-deletion repository. Restore onto a blank host, compare every advertised ref/OID and `HEAD`, run `git fsck --strict`, verify every referenced LFS object's SHA-256 and size, and perform fresh SSH/HTTP clones. Publish the last successful drill time and backup format/application version in admin diagnostics.

### 4. Security and operations

Add policy around existing authentication rather than collecting more login methods:

- enforce passkeys/strong authentication for administrators and optionally all local accounts;
- issue one-time recovery material and audit recovery use;
- keep Git HTTP on scoped, expiring tokens because Git CLI cannot complete WebAuthn/TOTP challenges; and
- rate-limit failed sign-in, token creation/use, SSH handshakes, authenticated and unauthenticated Git HTTP, archive/raw downloads, and LFS separately.

Rate limits should return `429` and `Retry-After` on HTTP paths and should key on both trusted client IP and authenticated principal where available. Proxy trust configuration must be explicit so spoofed forwarding headers cannot defeat limits.

Bound storage and work: per-repository and per-namespace Git/LFS quotas, maximum LFS object and batch sizes, push pack limits, archive generation timeout, concurrent Git subprocess limits, and import/mirror URL restrictions. Imports and webhooks are server-side network clients, so block loopback, link-local, private-network, metadata-service, and redirect-to-disallowed destinations by default; permit operator allowlists for intentional internal integrations. Forgejo exposes equivalent migration destination controls and webhook host controls. ([Forgejo configuration](https://forgejo.org/docs/latest/admin/config-cheat-sheet/), [Gitea webhooks](https://docs.gitea.com/usage/repository/webhooks))

Expose only a small operations contract:

- `/health/live`: process event loop is responsive;
- `/health/ready`: database, repository storage, and required key material are usable;
- protected metrics: request/latency/error counts by transport, active Git subprocesses, queue depth, storage use, last backup/restore drill, last `fsck`, mirror/webhook failures, and authentication throttles; and
- admin diagnostics: Git version, schema version, storage paths, migration state, and degraded repositories without secret values.

Run repository maintenance and `fsck` on schedules with per-repository locking, timeouts, jitter, and failure isolation. Never turn an integrity alert into automatic destructive repair.

### 5. Interoperability and automation

Treat Gitea compatibility as a **named conformance profile**, not a promise to implement all of Gitea. Record every endpoint/status/field Dokploy depends on, add contract tests, and publish unsupported fields or operations. Expand only for the lifecycle and distribution operations in the matrix.

For collection endpoints, use deterministic sorting, bounded `limit`, `page`, `Link`, and a total count, matching Gitea's documented pagination where compatibility matters. ([Gitea API usage](https://docs.gitea.com/development/api-usage/)) Prefer immutable repository IDs in API payloads while continuing to accept namespace/path locators.

Deploy keys should be independent service credentials where possible, with read-only as the safe default. Webhooks should be the automation escape hatch: canonical JSON, event and delivery IDs, timestamp, per-hook signing secret, retries with backoff, delivery inspection/redelivery, and no execution inside the Gitadel host. Keep the event set to push, branch/tag create/delete, rename/transfer/archive/delete/restore, and collaborator/deploy-key changes.

### 6. Browsing and distribution

Keep discovery source-oriented:

- repository index/search by name, namespace, and description;
- authorized results only for private repositories;
- deterministic pagination for repositories, branches, tags, and commits;
- direct raw-file URLs and source archives at a commit OID;
- convenient tag/default-branch aliases that redirect or resolve to the immutable OID form; and
- clone URL presentation for each enabled transport.

Archive generation should share authorization with clone, reject path/ref ambiguity, cap output and CPU time, and use conditional caching keyed by repository ID plus commit OID and format. A mutable tag URL must not be advertised as immutable merely because its current target is cached.

Commit-message search or a per-repository Atom feed can be added without introducing social state. Full code search should wait for measured need because indexing adds storage, update queues, authorization filtering, backup state, and operational tuning. Blame should likewise be bounded/on-demand; gitweb's own default-off choice is evidence that even a read-only feature can carry meaningful cost. ([Git gitweb](https://git-scm.com/docs/gitweb))

## Explicit non-goals and scope guard

The following tests should reject proposed scope growth:

- If a feature needs assignments, conversations, approvals, review state, or notifications between people, it is collaboration and stays out.
- If a feature executes repository-owned code on the server, it stays out; use signed webhooks to an external system.
- If a feature stores a second copy of binaries with its own lifecycle, permissions, and retention, it needs a separate distribution use case; tags/source archives/LFS remain the default.
- If compatibility work is not exercised by Dokploy or another named client contract, it does not justify broad Gitea API parity.
- If a search feature requires a cluster-scale index before repository-name and bounded per-repository search are inadequate, defer it until usage data exists.

## Assumptions and limits

- The competitive survey began from the supplied brief. A follow-up implementation audit corrected one important asymmetry: private token-authenticated Smart HTTP fetch already exists, although the README describes HTTP fetch as public-only. Each “must” item should still begin with a parity test to avoid rebuilding support that is undocumented.
- Documentation was reviewed as of 2026-08-22. Forgejo's `latest` reference and the Soft Serve `main` branch can change; pin behavior to versions when creating compatibility tests.
- Product editions and configuration differ. The evidence above asserts only what each cited source documents; absence from a source is not evidence that a product lacks a feature.
- Git bundle and mirror behavior concerns Git refs/objects. LFS, forge metadata, secrets, permissions, and audit state require separate handling.
- Radicle's cryptographic identity and replication model is architecturally different from a centralized forge. Only its stable-identity and independently verifiable-copy properties are relevant here.
- Recommendations are product judgments derived from the evidence, not claims that surveyed products converge on one universal baseline.

## Suggested implementation order

1. Regression-test and document private Smart HTTP fetch, then add scoped-token Smart HTTP push.
2. Protocol v2 and shallow-clone parity tests across SSH/HTTP/public/private/LFS.
3. Default branch/`HEAD`, archive, delayed delete/restore, atomic rename/transfer, and protected refs.
4. Automated restore validation and portable bundle+LFS export/import.
5. Deploy keys, quotas/rate limits, integrity maintenance, health/readiness, and metrics.
6. Raw files, source archives, deterministic pagination, OpenAPI, and the explicit Gitea conformance profile.
7. Signed webhooks and one-way mirrors.
8. Only after observed demand: partial clone, signature verification, richer search/feeds, TOTP, or push mirrors.
