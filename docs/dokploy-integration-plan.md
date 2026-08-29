# Plan: Docuploy (Dokploy) project-level integration

Status: implemented. Research grounded in Gitadel's current code (`src/integrations.rs`,
`src/identity/integrations.rs`, `src/repository/integrations.rs`,
`src/repository/dokploy.rs`) and Dokploy's published API surface
(`openapi.json`, cloned at `~/dev/vendor/dokploy`).

## What exists today

- **Namespace level**: a credential (URL + API key) per namespace per provider
  (`namespace_integrations`). No connectivity test.
- **Repository level**: an opt-out toggle only — every repository inherits the
  owner's integrations and deploys on push when a Dokploy resource happens to
  match (sourceType gitea, autoDeploy, owner/repo/branch). Nothing about the
  deployment itself is configured from Gitadel.
- **Push time**: `dokploy::deploy` walks `project.all`, matches resources,
  calls their refresh-token deploy endpoints.

## What we're adding

1. **Namespace level**: a **Test** button that verifies the stored URL + key
   against the live Dokploy instance.
2. **Repository level** (Settings → Integrations): explicit per-repo enabling
   plus a **Configure** page where build type, paths, environment variables,
   and domains are set for this repository.
3. **Push time**: before triggering a deploy, Gitadel syncs the saved
   configuration (env vars, domains, build type) to the linked Dokploy
   resource, so "configure once here, push" just works.

## Design decisions

### D1 — Generic provider abstraction

`src/integrations.rs` becomes the single place that knows what an integration
is. HTTP handlers and the UI enumerate providers; nothing names Dokploy
directly except its dispatcher module.

```rust
// The event every dispatcher reacts to. Providers never see git internals.
pub struct PushEvent<'a> {
    pub namespace: &'a str,
    pub repository: &'a str,
    pub branch: &'a str,
    pub payload: &'a Value, // provider-agnostic webhook-style body
}

pub struct Credential<'a> { pub url: &'a str, pub api_key: &'a str }

pub trait Dispatcher {
    /// Verify credentials without storing anything.
    fn test(&self, credential: &Credential) -> impl Future<Output = Result<TestReport, String>>;
    /// React to a push on an enabled, configured repository.
    fn on_push(&self, ctx: PushContext) -> impl Future<Output = Result<(), String>>;
}

pub struct PushContext<'a> {
    pub state: &'a RepositoryState,
    pub repository: &'a repository::Model,
    pub event: PushEvent<'a>,
    /// Per-repo row for this provider: enabled flag + opaque JSON config.
    pub config: Option<&'a str>,
    pub credential: Credential<'a>,
}
```

The registry gains `dispatcher(slug)` alongside the existing `provider(slug)`.
`src/repository/integrations.rs::trigger` stops matching slugs with a `match`
and calls `dispatcher(provider.slug).on_push(..)` instead. A future platform
with different needs (e.g. watching tag pushes, needing no repo config)
implements the same trait; storage and routes already fit it because:

- credentials stay `(url, api_key)` pairs owned by namespaces;
- per-repo state stays one row per `(repository_id, provider)` with an opaque
  JSON `config` whose schema only the dispatcher knows.

### D2 — Per-repo rows replace the opt-out model

New table `repository_integrations`:

```
repository_id uuid      PK
provider      text      PK
enabled       bool
resource      json?     -- provider-specific link: {kind, id} for dokploy
config        json?     -- {buildType, paths, env[], domains[], branch...}
updated_at    timestamptz
```

Semantics: **absence of a row means disabled**. Enabling is manual per
repository, as requested. The old `repository_integration_opt_outs` table is
migrated and dropped: opted-out repos become `enabled = false` rows, all other
existing repositories become `enabled = true` rows so nobody's current
deployments silently stop; newly created repositories start unconfigured and
off. The repository integrations endpoint keeps working for managers who only
want the on/off toggle; configuring is optional but required for anything to
actually deploy.

### D3 — Link first, create second

A Gitadel repository maps to exactly one Dokploy resource. On the configure
page the user either:

- **links** an existing application or compose (Gitadel lists candidates via
  `project.all` + detail lookups it already performs for matching), or
- **creates** one: `application.create` → `application.saveGiteaProvider`
  (`giteaId` resolved via `gitea.giteaProviders`, owner, repo, branch,
  build path), or `compose.create` → `compose.update` (sourceType gitea,
  owner, repository, branch, autoDeploy).

The chosen `{kind: "application"|"compose", id}` lands in `resource`. Only
linked + enabled repositories participate in push deployments; the generic
matching loop remains as a safety net for unconfigured-but-inherited repos
during the migration window, then goes away for configured ones.

### D4 — Sync then deploy

On push to an enabled + linked repository, the dispatcher:

1. serializes the canonical env map to `.env` text →
   `application.saveEnvironment` / `compose.saveEnvironment`;
2. applies build type → `application.saveBuildType`
   (`dockerfile`: dockerfile path + context path + build stage;
   `static`: publish directory + SPA flag; compose skips this);
3. diffs desired domains against `domain.byApplicationId` → minimal set of
   `domain.create` / `domain.update` / `domain.delete` calls;
4. triggers the deploy exactly as today (refresh-token endpoints).

Every step is best-effort and logged; a push never fails because Dokploy is
unreachable. A manual **Sync & Deploy** button on the configure page runs the
same pipeline on demand. Env values are stored plaintext in `gitadel.db`,
same trust boundary as today's API key — documented, not encrypted.

## API additions

All provider-parameterized by slug; unknown slugs 404 through the registry.

| Route | Who | Purpose |
|---|---|---|
| `POST /api/v1/namespaces/:ns/integrations/:provider/test` | namespace owner | Live credential check |
| `GET/PUT /api/v1/repos/:ns/:name/integrations/:provider/config` | repo manage | Read/write enabled + config (+ resource) |
| `GET /api/v1/repos/:ns/:name/integrations/:provider/resources` | repo manage | Linkable resources on the remote |
| `POST /api/v1/repos/:ns/:name/integrations/:provider/sync` | repo manage | Apply config now, optionally redeploy |

The existing list/toggle endpoints keep their shapes (they now read the new
table).

## Dokploy endpoints used (verified in openapi.json)

- Test: `GET /user.get` (401/403 ⇒ bad key), optionally `gitea.giteaProviders`
  to warn when no Gitea provider is connected yet.
- Discovery: `project.all`, `application.one`, `compose.one`.
- Create/link: `application.create`, `application.saveGiteaProvider`,
  `compose.create`, `compose.update`; `gitea.giteaProviders` for `giteaId`.
- Build type: `application.saveBuildType` — enum includes `dockerfile`,
  `static` (also nixpacks/railpack/heroku later if wanted).
  Dockerfile fields: `dockerfile` (path), `dockerContextPath`,
  `dockerBuildStage`. Static fields: `publishDirectory`, `isStaticSpa`.
- Env: `application.saveEnvironment` (env blob, buildArgs, buildSecrets),
  `compose.saveEnvironment`.
- Domains: `domain.byApplicationId`, `domain.create`, `domain.update`,
  `domain.delete`.
- Deploy: existing refresh-token flow, unchanged.

## Frontend

1. **Account/org settings → Integrations** (`integrations-settings.svelte`):
   each connected provider gets a **Test connection** button showing success
   ("Connected as <user>") or the error inline.
2. **Repo Settings → Integrations**
   (`repository-integration-settings.svelte`): status per provider becomes
   Not configured / Enabled / Disabled, with an Enable toggle and a
   **Configure** button (disabled while the namespace credential is missing).
3. **Configure view**: a new repository view (rail entry, same pattern as
   Settings/Webhooks — no new SvelteKit route needed), sections:
   - Resource: link existing (searchable list) or create new (kind, name,
     branch, build path);
   - Build: type selector (Dockerfile / Static / Compose-managed) + paths;
   - Environment variables: key/value editor serialized to `.env` order-stable;
   - Domains: host, path, port, HTTPS, certificate type;
   - Save writes the local row; **Sync & Deploy** pushes it to Dokploy.
4. Svelte 5 runes throughout, per the svelte skills when implementing.

## Implementation steps

1. Migration `m20260824_000019_create_repository_integrations.rs` (create +
   backfill + drop opt-outs) and entity changes in `src/entity.rs`.
2. `src/integrations.rs`: `PushEvent`, `Credential`, `Dispatcher`,
   registry lookup; rewire `repository::integrations::trigger` to dispatchers.
3. Expand `src/repository/dokploy.rs`: test, resource listing, link/create,
   config apply (env/build/domains diff), keeping the deploy code path.
4. Namespace test endpoint; repo config/resources/sync endpoints.
5. Frontend: test button → integrations tab rework → configure view.
6. Update `docs/dokploy.md`; run format/lint/check.

Roughly: 2 new backend modules' worth of edits, 1 migration, ~5 frontend files.
