# API refinement

The API is a transport bridge. Domain operations must be callable without an
Axum state, request, cookie jar, HTTP error, or generated runner protobuf.

## Ownership

- `identity`: authentication and account workflows, invitations, organizations,
  credentials, passkeys, OAuth/SSO, email verification and reset policy.
- `instance`: installation settings, branding, assets and administration.
- `backups`: provider workflows, preflight proofs, backup/restore reservation,
  maintenance progress and schedule claims.
- `storage`: storage target administration, usage, migration and registry data.
- `repositories`: lifecycle and access, issues/releases, imports/mirrors,
  content/diff/blame/media, caches, webhook persistence and delivery.
- `actions`: workflow discovery/dispatch, runner tasks, logs, secrets, artifact
  operations and their authorization.
- `integrations`: provider clients and connection/deployment workflows.
- `server`: construction, listeners/TLS, startup recovery, task supervision,
  cross-domain wiring, shutdown and maintenance drain.
- `api`: routing, request extraction, authentication transport, published DTOs,
  protocol encoding, cookies/redirects/headers, response and error mapping.

Keep `db` responsible for persistence primitives and transactions supplied by
callers. `core::audit::AuditContext` carries request metadata; `db::audit::record`
persists it inside the domain operation's transaction. Instance business rules
do not belong in DB or configuration parsing.

## Reuse and dependency rules

Extend existing implementations. Share account insertion, access policy,
validation, audit persistence, matching pagination semantics and DB fixtures.
Keep operations explicit where their permissions or side effects differ.
Do not hide behavior behind a generic CRUD framework or add a crate per feature.

Domain crates must not depend on `api` or `server`. Preserve public DTO names
and route registration. Use transport-independent domain errors. HTTP state
contains handles, not SQL, workflow state machines or background job ownership.

Repository access policy belongs in identity permissions and is reused by
repository and registry queries. Authentication source and scope distinctions
must survive service calls. Keep read, write and manage decisions separate.

An explicit repository post-write port may connect committed Git changes to
Actions and deployment effects. Server supplies the concrete coordinator.
Avoid a repository/Actions dependency cycle or a generic event bus.

## Acceptance

- API production handlers do not execute ORM queries, Git operations,
  password/encryption algorithms or domain transactions.
- Services enforce their business policy when called without HTTP adapters.
- Stateful authentication, backup proofs, storage operations and caches have
  domain owners; startup and worker supervision are server-owned.
- Existing statuses, error messages, scope rules, transaction/effect ordering,
  cancellation, maintenance drain and all persisted formats are preserved.
- Registered OpenAPI and generated bindings remain checked against handlers;
  focused domain and route tests cover the extracted operations.
- Count actual deduplication separately from code moved between crates and
  from route annotations or tests. Report remaining boundaries explicitly.

The uncompiled legacy OpenAPI catalog requires the owner's deletion approval;
its removal does not block these changes.
