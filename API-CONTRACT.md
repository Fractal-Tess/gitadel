# REST contract and frontend bindings

Gitadel keeps REST at `/api/v1`. The same Axum router builders register the
handlers and collect their OpenAPI description through Utoipa 6.0.0 and
utoipa-axum 0.3.0. Both versions are pinned in Cargo.toml.

`crates/api/src/openapi/typed.rs` collects JSON request bodies, every query extractor,
and JSON response bodies from the registered function's Rust types. An old
body annotation cannot silently override its actual `Json<T>` return type.
Query DTOs derive `IntoParams` and `ToSchema`; body/error DTOs derive
`ToSchema`. Path parameters, success statuses, authentication requirements,
and native HTTP representations are documented beside each handler.
Conflicting schema names stop generation.

## Generate and check

Enter the project's Nix development shell and install locked dependencies:

```sh
bun install --cwd frontend --frozen-lockfile
cargo fetch --locked
just api-generate
just api-check
```

Generation uses a local, pinned Hey API 0.99.0 installation. It never needs a
running instance, configuration file, database, credentials, or repository
data. After dependencies are fetched, Cargo runs offline and the generator
reads only `api/openapi.json`. No timestamps or machine addresses enter the
artifacts. The SDK's MIT notice is included with its generated sources.

The server can also export its own registered contract without opening data:

```sh
gitadel openapi > openapi.json
```

The `api-contract-export` Cargo feature produces a dedicated exporter that
always prints the contract and cannot start the service. Nix uses this to
generate/check the frontend before building the production server. This
compiles `gitadel-api` in exporter and service configurations. The `gitadel`
binary in `crates/server` forwards this feature to `gitadel-api`; the API
crate owns the build script and registered handlers. The exporter source
includes all workspace crates, protobuf definitions, the changelog, and the
embedded favicons, without depending on generated frontend assets.

`api/openapi.json`, `frontend/src/lib/api/generated/`, and `contracts.gen.ts`
are checked in. Review their diffs after changing handlers or DTOs. The drift
check regenerates into a temporary directory and compares every artifact.
Unresolved references and unspecified top-level JSON bodies fail before SDK
generation.

## Frontend calls

The existing transport uses Hey API's generated operation types while retaining
Zod validation, abort signals, same-origin credentials, and session expiry
handling. Callers infer their result from a validator checked against the
generated response; they cannot choose an arbitrary generic response type.
Both the keys and values of prepared JSON bodies and query objects are checked.
The generated fetch SDK is available for direct use as well.

```ts
await requestJson(repositoryApi(repository, '/history'), historySchema, {
  signal: controller.signal,
  query: apiQuery({ rev: revision, page: 2 }),
});
```

Use `jsonBody(...)` for JSON payloads and `apiQuery(...)` for query options.
Embedding a query string in a JSON request URL is rejected. Native binary
uploads keep their original body and Content-Type. Numeric path segments are
checked against generated path parameter types.

The compile-only fixture `frontend/tests/contract.typecheck.ts` checks rejected
URLs, methods, payloads, query values/keys, response fields, enum variants, and
direct SDK calls. An unexpectedly accepted example fails TypeScript checking
through an unused `@ts-expect-error`.

## Production packaging

`bun run --cwd frontend build` checks OpenAPI/SDK drift, runs real Svelte/TypeScript
checking, and then bundles assets. CI runs the same checks. Nix's frontend
derivation uses the offline exporter and runs this checked build, with Node
available for the generator and type checker.

The successful build records a fingerprint of the inputs in `api/inputs.txt`.
The Rust release build verifies it before embedding frontend assets, so an old
bundle cannot bypass type checking after source or contract changes. Run
`just frontend-build` again when that check reports stale assets. The
`build:assets` script is a bundler primitive; its output alone does not satisfy
the Rust release gate.

## Native protocols and remaining limits

Git, the OCI registry, OAuth token exchange, and the runner protocol remain
separate protocols. REST downloads, uploads, redirects, and SSE streams keep
native HTTP behavior; their parameters, media types, and statuses are documented.
The OpenAPI document itself is consumed through native HTTP rather than a
recursive generated document DTO. SSE frame payloads still use their existing
runtime validators.

WebAuthn options/credentials, provider-defined stored resource/configuration
JSON, and webhook event payloads contain opaque extension fields. Their enclosing
DTOs are typed, but changes inside those extension values are not compile-time
guarantees. String refinements, numeric ranges, permissions, and extractor error
responses also need runtime checks. Rust enums and response nullability are
checked against frontend validators.

The compatibility bridge compares nested response structures to eight levels.
Deeper structures remain runtime-validated. Path scalar types, native bodies,
and HTTP statuses still require accurate handler annotations; JSON bodies,
query DTOs, and JSON response DTOs are collected from the handler signature.

The historical, uncompiled `crates/api/src/openapi/operations/` catalog was
removed with the owner's approval. Registered handlers remain the sole source
for the live contract; do not reintroduce a parallel operation catalog.
