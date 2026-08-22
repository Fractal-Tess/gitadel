# Dokploy Gitea-provider compatibility contract for Gitadel

Research date: 2026-08-22  
Dokploy snapshot: `canary` / v0.30.2, commit [`772b76821771c53b072c2fbb95cf8876e1a65ae4`](https://github.com/Dokploy/dokploy/tree/772b76821771c53b072c2fbb95cf8876e1a65ae4) (2026-08-21)  
Gitea reference: current first-party documentation and v1.27.2 source, commit [`1dac1bb2f8593d4319125fa6bca9283000a2ddc2`](https://github.com/go-gitea/gitea/tree/1dac1bb2f8593d4319125fa6bca9283000a2ddc2)  
Gitadel snapshot: local working tree on 2026-08-22; the OAuth/Gitea compatibility work described below was uncommitted and changing during the research.

## Conclusion

Dokploy does not require a general Gitea clone. Its current integration has a narrow contract:

1. confidential OAuth authorization-code login at two Gitea paths;
2. two read-only JSON API endpoints for repository and branch discovery;
3. authenticated smart-HTTP cloning at `/{owner}/{repo}.git`; and
4. for automatic deployments only, a manually configured outgoing JSON push webhook.

Gitadel's current working tree is close to the **core MVP** (connect, discover, clone, and manually deploy): it already contains the OAuth application/authorization/token foundation, `Authorization: token` support, Gitea-shaped repository and branch routes, and OAuth-token Git HTTP authentication. It is **not yet an automatic-deploy MVP** because there is no repository webhook model, configuration surface, push-payload construction, or delivery worker. It is also not a full Gitea-compatible OAuth/API implementation.

The compatibility boundary should therefore be:

| Tier | Required behavior |
|---|---|
| **Core MVP** | Register a confidential OAuth app; authorize and exchange a code; list all accessible repositories and branches with Dokploy's pagination; clone the selected private branch over HTTP using the OAuth token. This is enough to configure a provider and perform on-demand deployments. |
| **Automatic-deploy MVP** | Core MVP plus manually configurable, repository-scoped outgoing `push` webhooks containing the fields Dokploy reads. |
| **Optional/full Gitea compatibility** | Refresh-token rotation and Gitea token lifetimes, granular scope enforcement, user/OIDC endpoints, Gitea hook CRUD/test APIs, complete DTOs and headers, signatures, delivery history/retries, OpenAPI/version surfaces, and broader webhook events. |

## 1. Exact OAuth contract

### Endpoints and request shapes

The active Dokploy UI constructs this browser URL directly:

```text
GET {publicGiteaUrl}/login/oauth/authorize
  ?client_id={clientId}
  &redirect_uri={dokployBaseUrl}/api/providers/gitea/callback
  &response_type=code
  &scope=read:repository%20read:user%20read:organization
  &state={dokployGiteaProviderId}
```

The source of truth is [`getGiteaOAuthUrl`](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/utils/gitea-utils.ts#L13-L30), which is called by the add-provider flow ([lines 96-130](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/components/dashboard/settings/git/gitea/add-gitea-provider.tsx#L96-L130)). Gitea documents the same authorization and token paths and the granular scopes introduced in Gitea 1.23 ([OAuth2 provider documentation](https://docs.gitea.com/development/oauth2-provider/)); the v1.27.2 router binds `/authorize`, `/access_token`, `/userinfo`, `/keys`, and `/introspect` at [`routers/web/web.go:603-617`](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/routers/web/web.go#L603-L617).

After the browser returns to Dokploy, Dokploy exchanges the code server-to-server:

```http
POST {giteaInternalUrl || publicGiteaUrl}/login/oauth/access_token
Content-Type: application/x-www-form-urlencoded
Accept: application/json

client_id=...
client_secret=...
code=...
grant_type=authorization_code
redirect_uri={the exact registered Dokploy callback}
```

Dokploy requires a successful JSON response with `access_token`. It stores `refresh_token` and computes an expiry only when `expires_in` is present ([callback exchange and persistence](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/pages/api/providers/gitea/callback.ts#L16-L85)). Consequently, a non-expiring access token with no `refresh_token` or `expires_in` works with current Dokploy: its refresh helper returns the stored access token when no refresh token exists ([`refreshGiteaToken`, lines 27-50](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/packages/server/src/utils/providers/gitea.ts#L27-L50)).

A Gitea-faithful token response additionally returns `token_type`, `expires_in`, and `refresh_token`, and the same endpoint accepts `grant_type=refresh_token`; Dokploy sends `refresh_token`, `client_id`, and `client_secret` in form data and retains the previous refresh token if the response omits a replacement ([Dokploy lines 52-90](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/packages/server/src/utils/providers/gitea.ts#L52-L90); [Gitea grant dispatch and validation](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/routers/web/auth/oauth2_provider.go#L512-L638)). Refresh tokens are therefore optional for the narrow current-Dokploy MVP, but required for full Gitea parity or expiring Gitadel access tokens.

### Callback, redirect, and state behavior

- The OAuth application must register the exact Dokploy callback shown in its form: `{dokployBaseUrl}/api/providers/gitea/callback` ([form defaults](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/components/dashboard/settings/git/gitea/add-gitea-provider.tsx#L64-L79)). Gitea itself rejects unregistered redirect URIs and non-`code` response types ([v1.27.2 source](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/routers/web/auth/oauth2_provider.go#L239-L278)), and binds the token exchange back to the original redirect URI ([lines 641-708](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/routers/web/auth/oauth2_provider.go#L641-L708)). Gitadel should do the same.
- Gitadel must treat `state` as opaque and return it byte-for-byte as a query parameter alongside `code`; on denial it should return the same state with an OAuth error. Current Dokploy sets state to its database-side provider ID. Its callback also accepts an older JSON-shaped state containing `giteaId`, but the active helper sends the raw ID ([state parser and callback checks](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/pages/api/providers/gitea/callback.ts#L5-L58)).
- Dokploy does not bind this state to a browser session or add its own nonce/MAC. Gitadel cannot repair that without breaking the callback; it should validate its own logged-in authorization/consent session, exact client and redirect URI, short-lived single-use code, and then preserve Dokploy's state unchanged.
- Dokploy tells the operator to create the app at `{giteaUrl}/user/settings/applications` ([add-provider instructions](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/components/dashboard/settings/git/gitea/add-gitea-provider.tsx#L170-L201)). Supporting that path as a redirect to Gitadel's Applications settings is not protocol-critical, but removes a setup-time 404.
- An unused Dokploy API route constructs a different authorization scope, `read:user repo` ([`authorize.ts:13-36`](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/pages/api/providers/gitea/authorize.ts#L13-L36)). No current UI reference to that route was found. Supporting legacy aliases `repo` and `user` is a low-cost compatibility precaution; the required active scopes are the three granular read scopes above.

### User endpoints

Dokploy does **not** call `GET /api/v1/user` or `/login/oauth/userinfo` in its Gitea provider flow. Its only authenticated-user route is `/api/v1/user/repos`. Implementing the Gitea current-user endpoint or OAuth/OIDC userinfo is optional for Dokploy, although both belong in fuller Gitea compatibility ([Gitea user operation](https://docs.gitea.com/api/operations/user-get-current/); [OAuth endpoint list](https://docs.gitea.com/development/oauth2-provider/)).

## 2. Repository and branch discovery

Dokploy issues these two requests using the server-side internal URL when configured, otherwise the public URL:

```http
GET {base}/api/v1/user/repos?page=1&limit=50
GET {base}/api/v1/repos/{owner}/{repo}/branches?page=1&limit=50
Accept: application/json
Authorization: token {access_token}
```

The scheme word is the lowercase literal `token`, not `Bearer` ([repository loop](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/packages/server/src/utils/providers/gitea.ts#L267-L327); [branch loop](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/packages/server/src/utils/providers/gitea.ts#L329-L394)). Gitea officially accepts `Authorization: token ...` and defines 1-based `page` plus `limit` pagination ([API usage](https://docs.gitea.com/development/api-usage/)).

Minimum repository item:

```json
{
  "id": "stable scalar",
  "name": "repo",
  "full_name": "owner/repo",
  "owner": { "login": "owner" }
}
```

Dokploy maps only `id`, `name`, `full_name`, and `owner.login`; its UI renames `full_name` to `url` and `owner.login` to `owner.username` ([mapping](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/packages/server/src/utils/providers/gitea.ts#L317-L326)). The official Gitea repository DTO has a numeric `id`, plus `ssh_url` and `clone_url` among many fields ([v1.27.2 `Repository`](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/modules/structs/repo.go#L58-L90)). Those extra URL fields and a numeric ID are full-compatibility concerns, not current Dokploy requirements.

`/user/repos` must include every repository the authenticated user can read—personal, organization-owned, and collaborator/team-visible, including private repositories—not merely repositories they own. This matches Gitea's authenticated repository-list semantics and operation ([current operation](https://docs.gitea.com/api/operations/user-current-list-repos/); [v1.27.2 handler](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/routers/api/v1/user/repo.go#L88-L138)).

Minimum branch item:

```json
{ "name": "main", "commit": { "id": "full-commit-object-id" } }
```

Dokploy uses both fields when normalizing branches. Gitea's route is 1-based and returns a raw JSON array ([current operation](https://docs.gitea.com/api/operations/repo-list-branches/); [v1.27.2 handler](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/routers/api/v1/repo/branch.go#L283-L386)).

Pagination is behaviorally important:

- honor `page` and `limit=50` and return raw arrays;
- return fewer than 50 items on the last non-full page;
- if the total is an exact multiple of 50, return `[]` on the next page;
- never ignore `page` while repeatedly returning 50 items, because Dokploy would loop forever; and
- `Link` and `X-Total-Count` are optional for Dokploy because it ignores them, although Gitea emits them and they belong in full compatibility.

## 3. Clone behavior and URL fields

Dokploy ignores `clone_url` and `ssh_url` from repository discovery. It synthesizes this URL itself:

```text
{giteaInternalUrl || publicGiteaUrl}/{owner}/{repository}.git
```

with URL userinfo `oauth2:{access_token}`, then runs a shallow branch-specific clone:

```text
git clone --branch {branch} --depth 1 [--recurse-submodules] {url} {destination}
```

See [`buildGiteaCloneUrl`](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/packages/server/src/utils/providers/gitea.ts#L100-L110) and the clone command ([lines 137-182](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/packages/server/src/utils/providers/gitea.ts#L137-L182)). Gitadel must therefore:

- serve smart-HTTP fetch at `/{owner}/{repo}.git/info/refs?service=git-upload-pack` and `/{owner}/{repo}.git/git-upload-pack`;
- accept HTTP Basic credentials where the username is `oauth2` and the password is the OAuth access token;
- authorize that token for read access to the selected repository and any submodule repositories; and
- avoid logging the credential-bearing clone URL or Authorization header.

Dokploy trims trailing slashes before API requests but not before authorization URL or clone URL construction. The provider's public and internal Gitea URLs should therefore be stored without a trailing slash. A path-prefixed base URL is string-concatenated and can work only if Gitadel/reverse proxy consistently serves OAuth, `/api/v1`, and Git smart HTTP below that prefix.

## 4. Webhooks and automatic deployment

### Dokploy does not use Gitea's hook API

No Gitea hook create/list/edit call exists in the inspected Dokploy provider. Instead, Dokploy displays a bearer-like per-application URL:

```text
https://dokploy.example/api/deploy/{refreshToken}
https://dokploy.example/api/deploy/compose/{refreshToken}
```

and tells the operator to copy it into the Git provider ([URL construction and UI](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/components/dashboard/application/deployments/show-deployments.tsx#L105-L109), [lines 229-263](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/components/dashboard/application/deployments/show-deployments.tsx#L229-L263)). Therefore the automatic-deploy MVP only needs a Gitadel-native way to save and emit a repository push webhook. Gitea-compatible hook CRUD is optional.

For comparison, full Gitea exposes `GET/POST /api/v1/repos/{owner}/{repo}/hooks`, CRUD/test by hook ID, and requires repository-admin authorization ([v1.27.2 routes](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/routers/api/v1/api.go#L1286-L1295)). Its create body includes `type`, `config` (`url` and `content_type`), `events`, `branch_filter`, `authorization_header`, and `active` ([Gitea DTO](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/modules/structs/hook.go#L49-L73); [operation](https://docs.gitea.com/api/operations/repo-create-hook/)). Dokploy's read-only OAuth scopes would not be sufficient for automatic hook creation even if Dokploy attempted it.

### Minimum event and payload Dokploy consumes

Configure an active generic webhook using **POST**, `application/json`, and the **push** event only. The smallest reliable Gitadel request is:

```http
POST /api/deploy/{refreshToken}
Content-Type: application/json
X-Gitea-Event: push

{
  "ref": "refs/heads/main",
  "after": "<new-tip-oid>",
  "commits": [
    {
      "id": "<commit-oid>",
      "message": "commit subject/body",
      "added": ["new-file"],
      "modified": ["changed-file"],
      "removed": ["deleted-file"]
    }
  ]
}
```

Dokploy's application and Compose handlers:

- strip `refs/heads/` from `body.ref` and require an exact match with the configured Gitea branch;
- use `body.after` as the deployment hash when only `X-Gitea-Event` identifies the provider;
- use the first commit message as the title; and
- flatten every commit's `added`, `modified`, and `removed` arrays for watch-path matching.

The application checks are at [`[refreshToken].ts:237-260`](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/pages/api/deploy/%5BrefreshToken%5D.ts#L237-L260), the Compose checks at [`compose/[refreshToken].ts:160-183`](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/pages/api/deploy/compose/%5BrefreshToken%5D.ts#L160-L183), and the shared extraction precedence at [`[refreshToken].ts:442-550`](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/pages/api/deploy/%5BrefreshToken%5D.ts#L442-L550). If no watch paths are configured, the commit file arrays may be empty or absent; otherwise they are required for a matching deployment ([`shouldDeploy`](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/packages/server/src/utils/watch-paths/should-deploy.ts#L3-L11)).

For Gitea-shaped payload parity, also send `before`, `compare_url`, `total_commits`, `head_commit`, `repository`, `pusher`, and `sender`; those are the official push fields ([v1.27.2 `PushPayload`](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/modules/structs/hook.go#L304-L330)). Each commit's official shape includes `id`, `message`, URL/identity/timestamp data, and the three file arrays ([`PayloadCommit`](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/modules/structs/hook.go#L109-L132)).

### Header precedence and security quirks

Full Gitea sends both `X-Gitea-Event: push` and GitHub-compatibility `X-GitHub-Event: push`, plus delivery/event-type and signature headers ([first-party webhook documentation](https://docs.gitea.com/usage/repository/webhooks/); [v1.27.2 header builder](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/services/webhook/deliver.go#L97-L145)). Dokploy checks `X-GitHub-Event` before `X-Gitea-Event`. If Gitadel sends both, `head_commit.id` and `head_commit.message` must be present or Dokploy loses the hash/title; sending only `X-Gitea-Event` uses `after` and `commits[0].message` instead.

Other current quirks:

- Dokploy special-cases only `X-GitHub-Event: ping`, not `X-Gitea-Event: ping` ([handler entry](https://github.com/Dokploy/dokploy/blob/772b76821771c53b072c2fbb95cf8876e1a65ae4/apps/dokploy/pages/api/deploy/%5BrefreshToken%5D.ts#L35-L66)). A Gitadel “test hook” should either send the GitHub-compatible ping header or, more safely for MVP, test with a normal push-shaped payload.
- Dokploy does not check that `X-Gitea-Event` equals `push`; it merely checks presence and then reads push fields. Gitadel should expose only the push event to this target to avoid accidental deployments.
- Dokploy does not verify `X-Gitea-Signature`, method, sender, repository identity, or an additional webhook secret. The unguessable `refreshToken` in the URL is the effective bearer credential. Gitadel must not log or expose it. Gitea HMAC signatures and secret configuration are valuable full-compatibility features, but do not add verification to current Dokploy.
- Delivery should occur only after a receive-pack succeeds. It should be asynchronous/outbox-backed so a slow or failed Dokploy request cannot turn an accepted Git push into a failure. Retry policy and delivery history are strongly recommended production features, although not part of Dokploy's input contract.

## 5. Version and header dependencies

Dokploy does not call `/api/v1/version`, inspect `X-Gitea-Version`, request a vendor media type, or negotiate a Gitea API revision. It sends only `Accept: application/json` and `Authorization: token ...` on discovery GETs. It also ignores Gitea `Link` and `X-Total-Count` pagination headers and stops based solely on array length.

For the core MVP, Gitadel can expose only the fixed `/api/v1` paths above. `GET /api/v1/version`, `/swagger.v1.json`, complete OpenAPI schemas, `X-Gitea-Version`, `Link`, and `X-Total-Count` are optional/full-compatibility surfaces. Current Gitea documents the API as OpenAPI and serves the instance specification at `/swagger.v1.json` ([API overview](https://docs.gitea.com/api/); [API usage](https://docs.gitea.com/development/api-usage/)).

## 6. Gitadel gap assessment

This section describes the local working tree observed on 2026-08-22. Several files were untracked or modified concurrently, so these are source-path/line citations rather than claims about committed `HEAD`.

### Present in the in-progress working tree

- OAuth application CRUD and a one-time client-secret response: `src/identity/oauth.rs:35-166`; UI at `frontend/src/lib/components/settings/oauth-application-settings.svelte:33-185`.
- Authorization endpoint, exact app/redirect lookup, login resumption, consent, supported Dokploy scopes, opaque state echo, and a ten-minute code: `src/identity/oauth.rs:168-315, 436-519`; `frontend/src/routes/login/+page.svelte:24-39`.
- Authorization-code token exchange and hashed persistent access tokens: `src/identity/oauth.rs:317-434`; tables/entities at `src/migration/m20260822_000007_create_oauth_provider.rs:9-169` and `src/entity.rs:133-199`.
- Both `Bearer` and lowercase `token` authorization schemes, with OAuth-token lookup and disabled-user checks: `src/identity/mod.rs:241-255, 286-348`.
- Gitea repository and branch aliases under `/api/v1`, 1-based pagination clamped to 50, accessible-repository filtering, the needed DTO fields, and raw arrays: `src/repository/mod.rs:293-345`; `src/repository/gitea.rs:14-165`.
- HTTP clone URL generation and OAuth-token Basic-password authentication for private repositories: `src/repository/mod.rs:111-120`; `src/repository/git_http.rs:159-201`.

The current `cargo test --workspace` run passed all 9 unit tests, including scope normalization and pagination. This is compile/unit evidence only; no live Dokploy OAuth/discovery/clone/webhook end-to-end test was present or run.

### Remaining core-MVP work or validation

1. **Finish and commit the in-progress compatibility code.** At research time the main OAuth module, Gitea adapter, migration, and related UI were uncommitted. A release cannot rely on a transient working tree.
2. **Run a real Dokploy end-to-end test.** Create the app using Dokploy's exact callback, complete consent in a fresh signed-out browser, discover private/personal/organization/collaborator repositories across more than 50 results, list more than 50 branches, and clone a selected private branch using the public and internal URL variants.
3. **Decide the token lifetime contract explicitly.** The current response returns `access_token`, `token_type`, and `scope`, with no expiry or refresh token (`src/identity/oauth.rs:326-331, 428-433`). That is compatible with current Dokploy but intentionally below Gitea parity. If access tokens become expiring, implement refresh grants first; otherwise Dokploy will eventually retain and retry a dead token.
4. **Preserve single-use code semantics under contention.** The current read-delete-insert transaction is directionally correct (`src/identity/oauth.rs:377-426`), but acceptance testing should attempt simultaneous exchanges and prove exactly one access token is committed.
5. **Clarify scope enforcement.** The code records the textual Dokploy scope but grants the same internal read bit to every accepted OAuth token (`src/identity/oauth.rs:409-415, 492-508`). This works for Dokploy's two repository APIs, but is not granular Gitea scope enforcement. Any future user/organization routes must enforce the corresponding textual scope.
6. **Add the conventional settings redirect.** `/user/settings/applications` should redirect an authenticated user to Gitadel's Applications settings tab. The existing `/settings` page defaults to Security and the selected tab is not URL-addressable (`frontend/src/routes/settings/+page.svelte:51-99`). This is setup usability, not a wire-protocol blocker.
7. **Treat DTO deviations as deliberate.** Gitadel currently serializes repository UUIDs as strings (`src/repository/gitea.rs:36-48, 137-164`), while Gitea specifies numeric IDs. Current Dokploy does not depend on numeric arithmetic, but a broader compatibility claim should supply a stable numeric compatibility ID or document the divergence.

### Remaining automatic-deploy MVP work

No `webhook`, `X-Gitea-*`, hook delivery, or push-payload implementation was found in `src/` or `frontend/src/`. Successful SSH receive-pack currently triggers only repository timestamp/audit recording (`src/repository/ssh.rs:301-310, 350-417`; `src/repository/resources.rs:291-313`). Gitadel must add, without coupling delivery success to Git push success:

1. repository-scoped webhook storage and an owner/admin configuration UI or native API;
2. target URL validation and secret-safe display/logging;
3. active/push-event/optional branch-filter configuration;
4. capture of each successful push's updated ref, before/after OIDs, commits, and added/modified/removed paths;
5. JSON payload construction with at least the minimum fields above;
6. `X-Gitea-Event: push` delivery after the ref update is durable; and
7. preferably an outbox/worker, bounded retries, timeouts, SSRF controls, and delivery audit/history.

## 7. Acceptance checklist

Core MVP is demonstrated only when all of these pass against the pinned Dokploy build or a documented newer commit:

- [ ] Dokploy's Applications link reaches Gitadel's OAuth-app UI, or the operator has a clearly documented Gitadel path.
- [ ] A confidential app accepts exactly the displayed Dokploy callback and returns client ID/secret once.
- [ ] Signed-out authorization resumes after login; allow and deny both echo the exact opaque state.
- [ ] Wrong client secret, wrong redirect URI, expired code, and code replay fail closed.
- [ ] Token JSON is accepted by Dokploy; if expiry is returned, refresh succeeds before expiry.
- [ ] `Authorization: token ACCESS` lists precisely all readable repositories and no unreadable private repository.
- [ ] Repository pagination works for 0, 1, 49, 50, 51, 100, and 101 results without duplicates or an infinite loop.
- [ ] Branch pagination has the same boundary tests and returns `commit.id`.
- [ ] `https://oauth2:ACCESS@host/owner/repo.git` shallow-clones the selected private branch; revoked tokens and disabled users fail.
- [ ] Public and internal provider URLs work, with documented trailing-slash and path-prefix behavior.

Automatic-deploy MVP additionally requires:

- [ ] A successful push to the configured branch produces one JSON `push` delivery; a failed receive-pack produces none.
- [ ] A different branch does not deploy.
- [ ] Watch-path deployments receive accurate `added`, `modified`, and `removed` arrays.
- [ ] With both Gitea and GitHub compatibility headers enabled, `head_commit.id/message` are present and Dokploy records the expected hash/title.
- [ ] The Dokploy URL token is redacted from logs, and delivery failures do not fail the Git push.

## Optional/full compatibility backlog

- Gitea-style grants, access-token expiry, refresh tokens/rotation, revocation UI, PKCE/public clients, client Basic authentication at the token endpoint, and multiple registered redirect URIs.
- `/login/oauth/userinfo`, OIDC discovery/keys/introspection, and `GET /api/v1/user`.
- Correct granular `read`/`write` scope enforcement for repository, user, and organization route groups. Gitea's current scope names are defined in [`models/auth/access_token_scope.go`](https://github.com/go-gitea/gitea/blob/1dac1bb2f8593d4319125fa6bca9283000a2ddc2/models/auth/access_token_scope.go#L54-L84).
- Complete Gitea repository, owner, branch, and error DTOs; numeric IDs; `Link` and `X-Total-Count` headers.
- Gitea repository-hook list/create/get/edit/delete/test endpoints, hook DTOs, generic hook settings, event selection, branch glob filters, signatures, delivery UUIDs/history, retries, and user/organization/system hooks.
- `/api/v1/version`, `/swagger.v1.json`, documented server/version headers, and compatibility tests across supported Gitea/Dokploy versions.

