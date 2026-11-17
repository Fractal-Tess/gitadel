use std::{
    borrow::Cow,
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use anyhow::Result;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::stream;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::{sync::watch, time::Duration};
use tower::ServiceExt;
use tower_http::{
    compression::{CompressionLayer, DefaultPredicate, Predicate},
    trace::TraceLayer,
};

use crate::{
    actions::ActionsState,
    archive::MaintenanceProgress,
    config::{IpNetwork, Settings},
    identity::{self, IdentityState},
    repository::{self, GitHttpState, RepositoryState},
};

#[cfg(not(debug_assertions))]
include!(concat!(env!("OUT_DIR"), "/frontend_assets.rs"));

async fn frontend_asset(name: &str) -> Option<Cow<'static, [u8]>> {
    if name.is_empty()
        || std::path::Path::new(name)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }

    #[cfg(debug_assertions)]
    {
        let root =
            tokio::fs::canonicalize(concat!(env!("CARGO_MANIFEST_DIR"), "/../../frontend/build"))
                .await
                .ok()?;
        let path = tokio::fs::canonicalize(root.join(name)).await.ok()?;
        if !path.starts_with(&root) {
            return None;
        }
        tokio::fs::read(path).await.ok().map(Cow::Owned)
    }
    #[cfg(not(debug_assertions))]
    {
        FRONTEND_ASSETS
            .binary_search_by(|(path, _)| path.cmp(&name))
            .ok()
            .map(|index| Cow::Borrowed(FRONTEND_ASSETS[index].1))
    }
}

/// Where images may load from. Beyond the instance itself, only shields.io:
/// READMEs lean on its badges, and one trusted host leaks far less about who
/// is reading than allowing any `https:` image would.
const IMAGE_SOURCES: &str = "'self' data: blob: https://img.shields.io";

/// Policy for API and asset responses; the frontend document adds script
/// hashes to its own, built from the same image sources.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https://img.shields.io; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'";

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    database: &'static str,
}

async fn dispatch_registry(
    State(registry): State<Router>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    // `v2` remains a valid Git namespace; registry requests must not steal its UI or Git transport.
    let git_path = path
        .strip_prefix("/v2/")
        .and_then(|path| path.split_once('/'))
        .is_some_and(|(_, resource)| {
            matches!(
                resource,
                "info/refs"
                    | "git-upload-pack"
                    | "git-receive-pack"
                    | "info/lfs/objects/batch"
                    | "info/lfs/locks"
                    | "info/lfs/locks/verify"
            ) || resource
                .strip_prefix("info/lfs/objects/")
                .is_some_and(|oid| {
                    oid.len() == 64 && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                || resource
                    .strip_prefix("info/lfs/locks/")
                    .and_then(|path| path.strip_suffix("/unlock"))
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        });
    let navigation = matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    ) && request
        .headers()
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|value| value.trim() == "text/html")
        });
    if path.starts_with("/v2/") && !git_path && !navigation {
        match registry.oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        }
    } else {
        next.run(request).await
    }
}
/// The `server.trusted_proxies` networks, shared by every request.
#[derive(Clone)]
struct TrustedProxies(Arc<[IpNetwork]>);

/// The address rate limits and audit events attribute a request to.
///
/// A peer inside a trusted network is a reverse proxy, so the client is the
/// rightmost `X-Forwarded-For` entry that is not itself a trusted proxy: the
/// proxy appends the address it saw, so anything the client wrote sits to
/// the left and is never reached. Any other peer is the client itself and
/// its header is ignored. An entry that is not an address stops the walk,
/// falling back to the peer rather than guessing.
fn client_address(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
    trusted: &[IpNetwork],
) -> Option<IpAddr> {
    let peer = peer?;
    let is_trusted = |address: IpAddr| trusted.iter().any(|network| network.contains(address));
    if !is_trusted(peer) {
        return Some(peer);
    }
    let forwarded = headers
        .get_all(header::HeaderName::from_static("x-forwarded-for"))
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect::<Vec<_>>();
    for entry in forwarded.into_iter().rev() {
        let Some(address) = parse_forwarded_address(entry) else {
            return Some(peer);
        };
        if !is_trusted(address) {
            return Some(address);
        }
    }
    Some(peer)
}

/// One `X-Forwarded-For` entry: an address, optionally with a port, with
/// IPv6 optionally in brackets.
fn parse_forwarded_address(entry: &str) -> Option<IpAddr> {
    if let Ok(address) = entry.parse::<IpAddr>() {
        return Some(address);
    }
    if let Ok(address) = entry.parse::<SocketAddr>() {
        return Some(address.ip());
    }
    entry
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|inner| inner.parse().ok())
}

async fn request_context(
    State(TrustedProxies(trusted)): State<TrustedProxies>,
    request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| peer.ip());
    let address = client_address(peer, request.headers(), &trusted);
    let mut response = identity::with_remote_address(address, next.run(request)).await;
    let headers = response.headers_mut();
    headers
        .entry(header::CONTENT_SECURITY_POLICY)
        .or_insert(HeaderValue::from_static(CONTENT_SECURITY_POLICY));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    response
}

/// Content types that gain nothing from transport compression: Git packs and
/// archives are already compressed, and `application/octet-stream` carries
/// LFS objects, release assets, and artifacts, which are large and usually
/// compressed too. Matched as prefixes.
const UNCOMPRESSED_CONTENT_TYPES: &[&str] = &[
    "application/x-git-",
    "application/octet-stream",
    "application/zip",
    "application/gzip",
    "application/x-gzip",
    "application/zstd",
    "application/x-xz",
    "application/x-bzip2",
    "application/x-7z-compressed",
];

/// The default response compression rules, minus the content types above.
#[derive(Clone, Copy)]
struct Compressible;

impl Predicate for Compressible {
    fn should_compress<B>(&self, response: &axum::http::Response<B>) -> bool
    where
        B: axum::body::HttpBody,
    {
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        !UNCOMPRESSED_CONTENT_TYPES.iter().any(|excluded| {
            content_type
                .get(..excluded.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(excluded))
        }) && DefaultPredicate::new().should_compress(response)
    }
}

/// How long shutdown waits for open HTTP connections to drain, and then for
/// background tasks to finish, before giving up on them. Without a bound a
/// single stalled connection holds the process until the supervisor kills it.

/// The origin of the configured public URL, e.g. `https://git.example.com`.
#[derive(Clone)]
struct PublicOrigin(std::sync::Arc<str>);

/// Whether a request that would be authenticated by the session cookie was
/// started by another site.
///
/// The session cookie is `SameSite=Lax`, which still lets a page on another
/// port or subdomain of the same site send state-changing requests with it.
/// Only cookie-authenticated mutations are checked: a request carrying an
/// `Authorization` header never falls back to the cookie, so tokens and the
/// Git, LFS, registry, and runner protocols are unaffected, as is anything
/// sent before a session exists (sign-in, registration, the OIDC callback).
/// A request with neither `Sec-Fetch-Site` nor `Origin` passes, which keeps
/// non-browser clients and old browsers working.
fn is_cross_site_cookie_request(
    method: &Method,
    headers: &HeaderMap,
    authority: Option<&str>,
    public_origin: &str,
) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
        || headers.contains_key(header::AUTHORIZATION)
        || axum_extra::extract::cookie::CookieJar::from_headers(headers)
            .get(identity::SESSION_COOKIE)
            .is_none()
    {
        return false;
    }
    if let Some(site) = headers.get("sec-fetch-site") {
        match site.as_bytes() {
            // The browser itself vouches for the request; no need to compare
            // origins, which a proxy that rewrites `Host` could confuse.
            b"same-origin" => return false,
            // Started by the user directly, such as from the address bar.
            b"none" => {}
            _ => return true,
        }
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return false;
    };
    let Ok(origin) = origin.to_str() else {
        return true;
    };
    if origin.eq_ignore_ascii_case(public_origin) {
        return false;
    }
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or(authority);
    let origin_host = origin.split_once("://").map(|(_, host)| host);
    !matches!(
        (origin_host, host),
        (Some(origin_host), Some(host)) if origin_host.eq_ignore_ascii_case(host)
    )
}

async fn reject_cross_site_cookie_requests(
    State(PublicOrigin(public_origin)): State<PublicOrigin>,
    request: Request,
    next: Next,
) -> Response {
    if is_cross_site_cookie_request(
        request.method(),
        request.headers(),
        request
            .uri()
            .authority()
            .map(|authority| authority.as_str()),
        &public_origin,
    ) {
        tracing::warn!(
            method = %request.method(),
            path = request.uri().path(),
            "refused a cross-site request carrying the session cookie"
        );
        return identity::ApiError::forbidden("Cross-site requests are not allowed.")
            .into_response();
    }
    next.run(request).await
}

/// Build the HTTP and protocol adapters from already constructed handles.
pub fn http_router(
    settings: &Settings,
    identity_state: &IdentityState,
    repository_state: &RepositoryState,
    actions_state: &ActionsState,
) -> Router {
    let git_http_state = GitHttpState::new(repository_state.clone(), actions_state.clone());
    let api_router = Router::new()
        .merge(crate::openapi::instance_router())
        .merge(identity::router().with_state(identity_state.clone()))
        .merge(repository::router().with_state(repository_state.clone()))
        .merge(actions_state.api_router());
    Router::new()
        .merge(
            Router::new()
                .route("/healthz", get(health))
                .with_state(identity_state.clone()),
        )
        .merge(identity::oauth_router().with_state(identity_state.clone()))
        .nest("/api/v1", api_router)
        .merge(actions_state.artifact_router())
        .nest("/api/actions", actions_state.protocol_router())
        .merge(repository::git_http_router().with_state(git_http_state))
        .fallback(get(frontend))
        .layer(CompressionLayer::new().compress_when(Compressible))
        .layer(middleware::from_fn_with_state(
            crate::registry::router(repository_state.clone()),
            dispatch_registry,
        ))
        .layer(middleware::from_fn_with_state(
            PublicOrigin(
                settings
                    .server
                    .public_url
                    .origin()
                    .ascii_serialization()
                    .into(),
            ),
            reject_cross_site_cookie_requests,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(
            TrustedProxies(Arc::from(settings.server.trusted_proxies.as_slice())),
            request_context,
        ))
}

#[derive(Clone)]
struct MaintenanceStatusState {
    operation_id: uuid::Uuid,
    receiver: watch::Receiver<MaintenanceProgress>,
}

/// Render maintenance progress without owning the maintenance operation.
pub fn maintenance_router(
    operation_id: uuid::Uuid,
    receiver: watch::Receiver<MaintenanceProgress>,
) -> Router {
    Router::new()
        .route(
            "/api/v1/admin/backups/progress/{operation_id}",
            get(maintenance_progress),
        )
        .route(
            "/api/v1/admin/storage/progress/{operation_id}",
            get(maintenance_progress),
        )
        .with_state(MaintenanceStatusState {
            operation_id,
            receiver,
        })
}

async fn maintenance_progress(
    Path(operation_id): Path<uuid::Uuid>,
    State(state): State<MaintenanceStatusState>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    if operation_id != state.operation_id {
        return Err(StatusCode::NOT_FOUND);
    }
    let events = stream::unfold(
        (state.receiver, true),
        |(mut receiver, initial)| async move {
            if !initial && receiver.changed().await.is_err() {
                return None;
            }
            let progress = receiver.borrow_and_update().clone();
            let data = serde_json::to_string(&progress).ok()?;
            Some((
                Ok(Event::default()
                    .data(data)
                    .retry(Duration::from_millis(500))),
                (receiver, false),
            ))
        },
    );
    Ok(Sse::new(events).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(1))
            .text("maintenance"),
    ))
}

async fn health(State(state): State<IdentityState>) -> Result<Json<HealthResponse>, StatusCode> {
    state.database().ping().await.map_err(|error| {
        tracing::error!(%error, "database health check failed");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    Ok(Json(HealthResponse {
        status: "ok",
        database: "ok",
    }))
}

async fn frontend(uri: Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');
    let asset_name = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };

    if let Some(asset) = frontend_asset(asset_name).await {
        return asset_response(asset_name, asset);
    }

    if !requested.contains('.')
        && let Some(index) = frontend_asset("index.html").await
    {
        return asset_response("index.html", index);
    }

    if frontend_asset("index.html").await.is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Frontend assets are not built. Run `bun run --cwd frontend build`.",
        )
            .into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

fn asset_response(path: &str, data: Cow<'static, [u8]>) -> Response {
    let content_security_policy = (path == "index.html")
        .then(|| frontend_content_security_policy(data.as_ref()))
        .flatten();
    let bytes = match data {
        Cow::Borrowed(bytes) => Bytes::from_static(bytes),
        Cow::Owned(bytes) => Bytes::from(bytes),
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    let cache_control = if path.starts_with("_app/immutable/") {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    } else if path == "index.html" {
        HeaderValue::from_static("no-cache, max-age=0, must-revalidate")
    } else {
        HeaderValue::from_static("public, max-age=3600")
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, cache_control);
    if let Some(policy) = content_security_policy {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, policy);
    }
    response
}

fn frontend_content_security_policy(data: &[u8]) -> Option<HeaderValue> {
    let Ok(html) = std::str::from_utf8(data) else {
        return None;
    };
    let mut hashes = Vec::new();
    let mut remaining = html;
    while let Some(start) = remaining.find("<script") {
        remaining = &remaining[start + "<script".len()..];
        let Some(open_end) = remaining.find('>') else {
            break;
        };
        let attributes = &remaining[..open_end];
        remaining = &remaining[open_end + 1..];
        let Some(close) = remaining.find("</script>") else {
            break;
        };
        let script = &remaining[..close];
        if !attributes.contains("src=") {
            hashes.push(format!(
                "'sha256-{}'",
                STANDARD.encode(Sha256::digest(script.as_bytes()))
            ));
        }
        remaining = &remaining[close + "</script>".len()..];
    }
    let hashes = hashes.join(" ");
    let policy = format!(
        "default-src 'self'; script-src 'self' {hashes}; style-src 'self' 'unsafe-inline'; img-src {IMAGE_SOURCES}; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'"
    );
    HeaderValue::from_str(&policy).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forwarded(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(
                HeaderName::from_static("x-forwarded-for"),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    fn networks(values: &[&str]) -> Vec<IpNetwork> {
        values.iter().map(|value| value.parse().unwrap()).collect()
    }

    #[test]
    fn untrusted_peers_are_the_client_whatever_they_forward() {
        let headers = forwarded(&["203.0.113.9"]);
        assert_eq!(
            client_address(Some(ip("198.51.100.4")), &headers, &[]),
            Some(ip("198.51.100.4"))
        );
        assert_eq!(
            client_address(
                Some(ip("198.51.100.4")),
                &headers,
                &networks(&["10.0.0.0/8"])
            ),
            Some(ip("198.51.100.4"))
        );
        assert_eq!(
            client_address(None, &headers, &networks(&["10.0.0.0/8"])),
            None
        );
    }

    #[test]
    fn trusted_peers_yield_the_rightmost_untrusted_forwarded_address() {
        let trusted = networks(&["10.0.0.0/8", "127.0.0.1"]);
        let peer = Some(ip("10.0.0.2"));
        // The client's own header entry sits left of what the proxy appended.
        assert_eq!(
            client_address(peer, &forwarded(&["1.1.1.1, 203.0.113.9"]), &trusted),
            Some(ip("203.0.113.9"))
        );
        // Two trusted hops, and the header split over two lines.
        assert_eq!(
            client_address(peer, &forwarded(&["203.0.113.9", "10.0.0.7"]), &trusted),
            Some(ip("203.0.113.9"))
        );
        // Ports and bracketed IPv6 as some proxies write them.
        assert_eq!(
            client_address(peer, &forwarded(&["[2001:db8::5]:4433"]), &trusted),
            Some(ip("2001:db8::5"))
        );
        assert_eq!(
            client_address(peer, &forwarded(&["203.0.113.9:51234"]), &trusted),
            Some(ip("203.0.113.9"))
        );
        // Nothing untrusted, no header, or garbage: the peer stands.
        assert_eq!(
            client_address(peer, &forwarded(&["10.0.0.7, 127.0.0.1"]), &trusted),
            peer
        );
        assert_eq!(client_address(peer, &HeaderMap::new(), &trusted), peer);
        assert_eq!(
            client_address(peer, &forwarded(&["203.0.113.9, not-an-address"]), &trusted),
            peer
        );
    }

    fn image_directive(policy: &str) -> &str {
        policy
            .split("; ")
            .find(|directive| directive.starts_with("img-src "))
            .expect("policy has an img-src directive")
    }

    #[test]
    fn both_policies_allow_only_the_configured_image_sources() {
        let expected = format!("img-src {IMAGE_SOURCES}");
        assert_eq!(image_directive(CONTENT_SECURITY_POLICY), expected);

        let document = frontend_content_security_policy(b"<script>window.x = 1</script>")
            .expect("document policy");
        assert_eq!(image_directive(document.to_str().unwrap()), expected);
    }

    async fn content_encoding(content_type: &'static str) -> Option<HeaderValue> {
        let app =
            Router::new()
                .route(
                    "/",
                    get(move || async move {
                        ([(header::CONTENT_TYPE, content_type)], vec![b'a'; 4096])
                    }),
                )
                .layer(CompressionLayer::new().compress_when(Compressible));
        let request = Request::builder()
            .uri("/")
            .header(header::ACCEPT_ENCODING, "gzip")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        response.headers().get(header::CONTENT_ENCODING).cloned()
    }

    #[tokio::test]
    async fn packs_binaries_and_archives_are_served_uncompressed() {
        for content_type in [
            "application/x-git-upload-pack-result",
            "application/x-git-receive-pack-result",
            "application/octet-stream",
            "Application/Octet-Stream",
            "application/zip",
            "application/gzip",
            "image/png",
        ] {
            assert_eq!(content_encoding(content_type).await, None, "{content_type}");
        }
        for content_type in [
            "application/json",
            "text/html; charset=utf-8",
            "image/svg+xml",
        ] {
            assert_eq!(
                content_encoding(content_type).await,
                Some(HeaderValue::from_static("gzip")),
                "{content_type}"
            );
        }
    }

    const PUBLIC_ORIGIN: &str = "https://git.example.com";

    fn cross_site(method: Method, headers: &[(&'static str, &str)]) -> bool {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        is_cross_site_cookie_request(&method, &map, None, PUBLIC_ORIGIN)
    }

    const SESSION: (&str, &str) = ("cookie", "theme=dark; gitadel_session=secret");

    #[test]
    fn cookie_mutations_from_other_sites_are_refused() {
        // Another port or subdomain of the same site.
        assert!(cross_site(
            Method::POST,
            &[SESSION, ("sec-fetch-site", "same-site")]
        ));
        assert!(cross_site(
            Method::DELETE,
            &[SESSION, ("sec-fetch-site", "cross-site")]
        ));
        assert!(cross_site(
            Method::POST,
            &[
                SESSION,
                ("host", "git.example.com"),
                ("origin", "https://git.example.com:8443"),
            ]
        ));
        assert!(cross_site(
            Method::POST,
            &[
                SESSION,
                ("host", "git.example.com"),
                ("origin", "https://evil.example.com")
            ]
        ));
        assert!(cross_site(
            Method::POST,
            &[SESSION, ("host", "git.example.com"), ("origin", "null")]
        ));
        // A user-initiated request still has its Origin checked.
        assert!(cross_site(
            Method::POST,
            &[
                SESSION,
                ("sec-fetch-site", "none"),
                ("host", "git.example.com"),
                ("origin", "https://evil.example.com"),
            ]
        ));
    }

    #[test]
    fn same_origin_and_non_browser_requests_pass() {
        // Browsers on the instance itself.
        assert!(!cross_site(
            Method::POST,
            &[
                SESSION,
                ("sec-fetch-site", "same-origin"),
                ("origin", PUBLIC_ORIGIN)
            ]
        ));
        assert!(!cross_site(
            Method::POST,
            &[SESSION, ("origin", PUBLIC_ORIGIN)]
        ));
        // Reached under a host other than the configured public URL.
        assert!(!cross_site(
            Method::POST,
            &[
                SESSION,
                ("host", "kiwi.netbird.cloud:3030"),
                ("origin", "http://kiwi.netbird.cloud:3030"),
            ]
        ));
        // curl with a cookie jar and old browsers send neither header.
        assert!(!cross_site(Method::POST, &[SESSION]));
        // Safe methods are never checked.
        assert!(!cross_site(
            Method::GET,
            &[SESSION, ("sec-fetch-site", "cross-site")]
        ));
        assert!(!cross_site(
            Method::HEAD,
            &[SESSION, ("sec-fetch-site", "cross-site")]
        ));
    }

    #[test]
    fn requests_not_authenticated_by_the_session_cookie_pass() {
        // Tokens: API clients, Git, LFS, registry, and runners.
        assert!(!cross_site(
            Method::POST,
            &[
                SESSION,
                ("authorization", "Bearer token"),
                ("sec-fetch-site", "cross-site"),
                ("origin", "https://evil.example.com"),
            ]
        ));
        // No session yet: sign-in, registration, an identity provider posting
        // back to the OIDC callback.
        assert!(!cross_site(
            Method::POST,
            &[
                ("cookie", "gitadel_oidc_transaction=abc"),
                ("sec-fetch-site", "cross-site"),
                ("origin", "https://idp.example.com"),
            ]
        ));
        assert!(!cross_site(
            Method::POST,
            &[("sec-fetch-site", "cross-site")]
        ));
    }

    #[tokio::test]
    async fn cross_site_cookie_mutation_is_answered_with_403() {
        let app = Router::new()
            .route("/delete", axum::routing::post(|| async { "deleted" }))
            .layer(middleware::from_fn_with_state(
                PublicOrigin(PUBLIC_ORIGIN.into()),
                reject_cross_site_cookie_requests,
            ));
        let request = |site: &'static str| {
            Request::builder()
                .method(Method::POST)
                .uri("/delete")
                .header(header::COOKIE, "gitadel_session=secret")
                .header("sec-fetch-site", site)
                .body(Body::empty())
                .unwrap()
        };

        let refused = app.clone().oneshot(request("same-site")).await.unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let allowed = app.oneshot(request("same-origin")).await.unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    #[test]
    fn images_are_not_open_to_every_https_host() {
        assert!(
            IMAGE_SOURCES
                .split(' ')
                .all(|source| source != "https:" && source != "*")
        );
        assert!(IMAGE_SOURCES.contains("https://img.shields.io"));
    }
}
