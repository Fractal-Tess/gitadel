use axum::{
    Form, Json,
    extract::{OriginalUri, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
#[cfg(test)]
use sea_orm::{ActiveModelTrait, EntityTrait};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE};

pub use gitadel_identity::oauth::ACCESS_TOKEN_LIFETIME;
#[derive(Serialize, ToSchema)]
pub struct OauthApplicationResponse {
    id: Uuid,
    name: String,
    client_id: String,
    redirect_uri: String,
    created_at: chrono::DateTime<Utc>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateOauthApplicationRequest {
    name: String,
    redirect_uri: String,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedOauthApplicationResponse {
    client_secret: String,
    application: OauthApplicationResponse,
}

#[utoipa::path(
    get, path = "/me/oauth-applications",
    operation_id = "get_me_oauth_applications",
    summary = "List OAuth applications",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = Vec<OauthApplicationResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_applications(
    State(state): State<IdentityState>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<OauthApplicationResponse>>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::list_applications(&state.auth_service(), &actor)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[utoipa::path(
    post, path = "/me/oauth-applications",
    operation_id = "post_me_oauth_applications",
    summary = "Create an OAuth application",
    tag = "Account",

    responses(
        (status = 201, description = "Success", body = crate::identity::oauth::CreatedOauthApplicationResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_application(
    State(state): State<IdentityState>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateOauthApplicationRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json::<CreatedOauthApplicationResponse>(
            domain::create_application(
                &state.auth_service(),
                &actor,
                domain::CreateOauthApplicationRequest {
                    name: request.name,
                    redirect_uri: request.redirect_uri,
                },
            )
            .await?
            .into(),
        ),
    ))
}

#[utoipa::path(
    delete, path = "/me/oauth-applications/{id}",
    operation_id = "delete_me_oauth_applications_id",
    summary = "Delete an OAuth application",
    tag = "Account",
    params(
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_application(
    State(state): State<IdentityState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::delete_application(&state.auth_service(), &actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AuthorizeQuery {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    scope: Option<String>,
    state: Option<String>,
}

pub async fn applications_settings_redirect() -> Redirect {
    Redirect::temporary("/settings?view=applications")
}

pub async fn authorize(
    State(state): State<IdentityState>,
    jar: CookieJar,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<AuthorizeQuery>,
) -> Response {
    render_outcome(
        domain::authorize(
            &state.auth_service(),
            jar.get(super::SESSION_COOKIE).map(|c| c.value()),
            &uri.to_string(),
            domain::AuthorizeQuery {
                client_id: query.client_id,
                redirect_uri: query.redirect_uri,
                response_type: query.response_type,
                scope: query.scope,
                state: query.state,
            },
        )
        .await,
    )
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ApproveRequest {
    consent_token: String,
    decision: String,
}

pub async fn approve(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Form(form): Form<ApproveRequest>,
) -> Response {
    render_outcome(
        domain::approve(
            &state.auth_service(),
            jar.get(super::SESSION_COOKIE).map(|c| c.value()),
            domain::ApproveRequest {
                consent_token: form.consent_token,
                decision: form.decision,
            },
        )
        .await,
    )
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct TokenRequest {
    grant_type: String,
    client_id: String,
    client_secret: String,
    // Optional so an unsupported grant (Gitea clients try refresh_token) is
    // answered with an OAuth error object instead of a form-decoding failure.
    code: Option<String>,
    redirect_uri: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    /// Seconds until the token expires (RFC 6749 section 5.1).
    expires_in: i64,
    refresh_token: String,
    scope: String,
}

/// A freshly generated access and refresh token. Only their hashes are stored.
pub use gitadel_identity::oauth::TokenPair;

pub async fn access_token(
    State(state): State<IdentityState>,
    Form(request): Form<TokenRequest>,
) -> Response {
    render_outcome(
        domain::access_token(
            &state.auth_service(),
            domain::TokenRequest {
                grant_type: request.grant_type,
                client_id: request.client_id,
                client_secret: request.client_secret,
                code: request.code,
                redirect_uri: request.redirect_uri,
                refresh_token: request.refresh_token,
            },
        )
        .await,
    )
}

fn token_response(pair: TokenPair, scope: String) -> Response {
    let mut response = Json(TokenResponse {
        access_token: pair.access_token,
        token_type: "Bearer",
        expires_in: ACCESS_TOKEN_LIFETIME.num_seconds(),
        refresh_token: pair.refresh_token,
        scope,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

/// An application the user has live access tokens for.
#[derive(Serialize, ToSchema)]
pub struct AuthorizationResponse {
    application_name: String,
    client_id: String,
    /// Every scope any of the live tokens carries.
    scopes: Vec<String>,
    /// When the oldest live token was issued.
    created_at: chrono::DateTime<Utc>,
    last_used_at: Option<chrono::DateTime<Utc>>,
    /// When the newest live token expires; the client must re-authorise then.
    expires_at: chrono::DateTime<Utc>,
    token_count: usize,
}

/// The applications holding unexpired, unrevoked access tokens for the user,
/// one entry per application.
#[utoipa::path(
    get, path = "/me/authorizations",
    operation_id = "get_me_authorizations",
    summary = "List authorized OAuth applications",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = Vec<AuthorizationResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_authorizations(
    State(state): State<IdentityState>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<AuthorizationResponse>>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::list_authorizations(&state.auth_service(), &actor)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

/// Revokes every access token the application holds for the user.
#[utoipa::path(
    delete, path = "/me/authorizations/{client_id}",
    operation_id = "delete_me_authorizations_client_id",
    summary = "Revoke an OAuth application's access",
    tag = "Account",
    params(
        ("client_id" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn revoke_authorization(
    State(state): State<IdentityState>,
    axum::extract::Path(client_id): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::revoke_authorization(&state.auth_service(), &actor, client_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn authorization_success_redirect(redirect_uri: &str, code: &str, state: Option<&str>) -> Response {
    let mut url = Url::parse(redirect_uri).expect("stored OAuth redirect URIs are valid URLs");
    let mut query = url.query_pairs_mut();
    query.append_pair("code", code);
    if let Some(state) = state {
        query.append_pair("state", state);
    }
    drop(query);
    Redirect::to(url.as_str()).into_response()
}

fn authorization_error_redirect(
    redirect_uri: &str,
    error: &str,
    description: &str,
    state: Option<&str>,
) -> Response {
    let mut url = Url::parse(redirect_uri).expect("validated OAuth redirect URIs are valid URLs");
    let mut query = url.query_pairs_mut();
    query.append_pair("error", error);
    query.append_pair("error_description", description);
    if let Some(state) = state {
        query.append_pair("state", state);
    }
    drop(query);
    Redirect::to(url.as_str()).into_response()
}

#[derive(Serialize, utoipa::ToSchema)]
struct OauthErrorResponse<'a> {
    error: &'a str,
    error_description: &'a str,
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    (
        status,
        Json(OauthErrorResponse {
            error,
            error_description: description,
        }),
    )
        .into_response()
}

#[cfg(test)]
use domain::oauth_scope_bits;
use gitadel_identity::oauth as domain;
fn render_outcome(outcome: domain::OAuthOutcome) -> Response {
    match outcome {
        domain::OAuthOutcome::Failure(error) => ApiError::from(error).into_response(),
        domain::OAuthOutcome::Rejected {
            unauthorized,
            error,
            description,
        } => oauth_error(
            if unauthorized {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::BAD_REQUEST
            },
            &error,
            &description,
        ),
        domain::OAuthOutcome::Redirect(location) => Redirect::to(&location).into_response(),
        domain::OAuthOutcome::RedirectError {
            redirect_uri,
            error,
            description,
            state,
        } => authorization_error_redirect(&redirect_uri, &error, &description, state.as_deref()),
        domain::OAuthOutcome::Authorized {
            redirect_uri,
            code,
            state,
        } => authorization_success_redirect(&redirect_uri, &code, state.as_deref()),
        domain::OAuthOutcome::Tokens { pair, scope } => token_response(pair, scope),
    }
}
impl From<domain::OauthApplicationResponse> for OauthApplicationResponse {
    fn from(v: domain::OauthApplicationResponse) -> Self {
        Self {
            id: v.id,
            name: v.name,
            client_id: v.client_id,
            redirect_uri: v.redirect_uri,
            created_at: v.created_at,
        }
    }
}

#[cfg(test)]
use super::SCOPE_REPOSITORY_READ;
#[cfg(test)]
use super::hash_secret;
#[cfg(test)]
use super::random_secret;
#[cfg(test)]
use crate::entity::oauth_access_token;
#[cfg(test)]
use crate::entity::oauth_application;
#[cfg(test)]
use sea_orm::Set;
impl From<domain::CreatedOauthApplicationResponse> for CreatedOauthApplicationResponse {
    fn from(v: domain::CreatedOauthApplicationResponse) -> Self {
        Self {
            client_secret: v.client_secret,
            application: v.application.into(),
        }
    }
}
impl From<domain::AuthorizationResponse> for AuthorizationResponse {
    fn from(v: domain::AuthorizationResponse) -> Self {
        Self {
            application_name: v.application_name,
            client_id: v.client_id,
            scopes: v.scopes,
            created_at: v.created_at,
            last_used_at: v.last_used_at,
            expires_at: v.expires_at,
            token_count: v.token_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, StatusCode};
    use serde_json::{Value, json};

    use super::*;
    use crate::{entity::user, identity::test_support::Harness};

    pub(super) async fn application(
        harness: &Harness,
        owner: &user::Model,
        name: &str,
        redirect_uri: &str,
    ) -> (oauth_application::Model, String) {
        let client_secret = format!("gcs_{}", random_secret(32));
        let application = oauth_application::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(owner.id),
            name: Set(name.to_owned()),
            client_id: Set(format!("goc_{}", random_secret(24))),
            client_secret_hash: Set(hash_secret(&client_secret)),
            redirect_uri: Set(redirect_uri.to_owned()),
            created_at: Set(Utc::now()),
        }
        .insert(&harness.database)
        .await
        .unwrap();
        (application, client_secret)
    }

    /// Inserts an access token as the token endpoint would, with a chosen
    /// expiry; returns the raw bearer value.
    pub(super) async fn token(
        harness: &Harness,
        application: &oauth_application::Model,
        account: &user::Model,
        scope: &str,
        expires_at: chrono::DateTime<Utc>,
    ) -> String {
        let raw = format!("gto_{}", random_secret(32));
        oauth_access_token::ActiveModel {
            token_hash: Set(hash_secret(&raw)),
            application_id: Set(application.id),
            user_id: Set(account.id),
            scopes: Set(oauth_scope_bits(scope)),
            scope: Set(scope.to_owned()),
            created_at: Set(Utc::now()),
            expires_at: Set(expires_at),
            last_used_at: Set(None),
            revoked_at: Set(None),
        }
        .insert(&harness.database)
        .await
        .unwrap();
        raw
    }

    fn in_days(days: i64) -> chrono::DateTime<Utc> {
        Utc::now() + chrono::Duration::days(days)
    }

    #[tokio::test]
    async fn expired_access_tokens_are_refused() {
        let harness = Harness::new().await;
        let alice = harness.user("alice").await;
        let (application, _) =
            application(&harness, &alice, "Deploy", "https://client.test/callback").await;
        let live = token(
            &harness,
            &application,
            &alice,
            "read:repository",
            in_days(1),
        )
        .await;
        let expired = token(
            &harness,
            &application,
            &alice,
            "read:repository",
            in_days(-1),
        )
        .await;

        let authenticated = harness
            .state
            .authenticate_token(&live, SCOPE_REPOSITORY_READ)
            .await
            .unwrap();
        assert_eq!(authenticated.user.id, alice.id);
        assert!(authenticated.via_api_token);
        let refused = harness
            .state
            .authenticate_token(&expired, SCOPE_REPOSITORY_READ)
            .await
            .err()
            .expect("an expired token is refused");
        assert_eq!(refused.to_string(), ApiError::unauthorized().to_string());

        // The same through the API, as a client would see it: a live token
        // is authenticated (and then refused for lacking the read scope,
        // which OAuth tokens never carry), an expired one is not.
        let request = |bearer: &str| {
            axum::http::Request::builder()
                .method(Method::GET)
                .uri("/api/v1/me/commit-identity")
                .header(
                    axum::http::header::AUTHORIZATION,
                    format!("Bearer {bearer}"),
                )
                .body(axum::body::Body::empty())
                .unwrap()
        };
        assert_eq!(
            harness.send(request(&live)).await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            harness.send(request(&expired)).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn authorizations_list_live_tokens_grouped_by_application() {
        let harness = Harness::new().await;
        let alice = harness.user("alice").await;
        let bob = harness.user("bob").await;
        let (deploy, _) = application(&harness, &alice, "Deploy", "https://d.test/cb").await;
        let (ci, _) = application(&harness, &alice, "CI", "https://ci.test/cb").await;
        token(&harness, &deploy, &alice, "read:repository", in_days(10)).await;
        token(&harness, &deploy, &alice, "read:user", in_days(20)).await;
        token(&harness, &deploy, &alice, "repo", in_days(-1)).await;
        token(&harness, &ci, &alice, "read:repository", in_days(5)).await;
        // Another user's token is not listed.
        token(&harness, &ci, &bob, "read:repository", in_days(5)).await;

        let cookie = harness.login(&alice).await;
        let (status, body) = harness
            .request(
                Method::GET,
                "/api/v1/me/authorizations",
                Some(&cookie),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let entries = body.as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let deploy_entry = entries
            .iter()
            .find(|entry| entry["client_id"] == deploy.client_id)
            .unwrap();
        assert_eq!(deploy_entry["application_name"], "Deploy");
        assert_eq!(
            deploy_entry["scopes"],
            json!(["read:repository", "read:user"]),
            "the expired token's scope is not counted"
        );
        assert_eq!(deploy_entry["token_count"], 2);
        assert!(deploy_entry["last_used_at"].is_null());
        let expires = deploy_entry["expires_at"].as_str().unwrap();
        assert!(
            chrono::DateTime::parse_from_rfc3339(expires).unwrap() > in_days(19),
            "the newest live expiry is reported"
        );
        let ci_entry = entries
            .iter()
            .find(|entry| entry["client_id"] == ci.client_id)
            .unwrap();
        assert_eq!(ci_entry["token_count"], 1);

        let bob_cookie = harness.login(&bob).await;
        let (_, body) = harness
            .request(
                Method::GET,
                "/api/v1/me/authorizations",
                Some(&bob_cookie),
                None,
            )
            .await;
        assert_eq!(body.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn revoking_an_authorization_kills_every_token_of_that_application() {
        let harness = Harness::new().await;
        let alice = harness.user("alice").await;
        let (deploy, _) = application(&harness, &alice, "Deploy", "https://d.test/cb").await;
        let (ci, _) = application(&harness, &alice, "CI", "https://ci.test/cb").await;
        let first = token(&harness, &deploy, &alice, "read:repository", in_days(10)).await;
        let second = token(&harness, &deploy, &alice, "repo", in_days(20)).await;
        let other = token(&harness, &ci, &alice, "read:repository", in_days(5)).await;
        let cookie = harness.login(&alice).await;
        for live in [&first, &second, &other] {
            assert!(
                harness
                    .state
                    .authenticate_token(live, SCOPE_REPOSITORY_READ)
                    .await
                    .is_ok()
            );
        }

        let (status, _) = harness
            .request(
                Method::DELETE,
                &format!("/api/v1/me/authorizations/{}", deploy.client_id),
                Some(&cookie),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        for revoked in [&first, &second] {
            assert!(
                harness
                    .state
                    .authenticate_token(revoked, SCOPE_REPOSITORY_READ)
                    .await
                    .is_err()
            );
        }
        assert!(
            harness
                .state
                .authenticate_token(&other, SCOPE_REPOSITORY_READ)
                .await
                .is_ok(),
            "another application's token keeps working"
        );
        let (_, body) = harness
            .request(
                Method::GET,
                "/api/v1/me/authorizations",
                Some(&cookie),
                None,
            )
            .await;
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["client_id"], Value::String(ci.client_id.clone()));

        // Nothing left to revoke, and unknown clients, are not found.
        let (status, _) = harness
            .request(
                Method::DELETE,
                &format!("/api/v1/me/authorizations/{}", deploy.client_id),
                Some(&cookie),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = harness
            .request(
                Method::DELETE,
                "/api/v1/me/authorizations/goc_unknown",
                Some(&cookie),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn expired_access_tokens_are_swept_and_live_ones_kept() {
        let harness = Harness::new().await;
        let alice = harness.user("alice").await;
        let (application, _) = application(&harness, &alice, "Deploy", "https://d.test/cb").await;
        token(&harness, &application, &alice, "read:user", in_days(-1)).await;
        token(&harness, &application, &alice, "read:user", in_days(-90)).await;
        let live = token(&harness, &application, &alice, "read:user", in_days(1)).await;

        let removed = oauth_access_token::delete_expired(&harness.database, Utc::now())
            .await
            .unwrap();

        let remaining = oauth_access_token::Entity::find()
            .all(&harness.database)
            .await
            .unwrap();
        assert_eq!(removed, 2);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].token_hash, hash_secret(&live));
    }

    mod flow {
        use axum::{
            body::Body,
            http::{Request, header},
        };

        use super::*;
        use crate::identity::test_support::json_body;

        const REDIRECT_URI: &str = "https://client.test/callback";

        fn query(location: &str, name: &str) -> Option<String> {
            let url = Url::parse(location)
                .or_else(|_| Url::parse("https://gitadel.test").unwrap().join(location))
                .unwrap();
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        }

        fn location(response: &axum::response::Response) -> String {
            assert!(
                response.status().is_redirection(),
                "expected a redirect, got {}",
                response.status()
            );
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        }

        async fn form(
            harness: &Harness,
            uri: &str,
            cookie: Option<&str>,
            fields: &[(&str, &str)],
        ) -> axum::response::Response {
            let body = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(fields)
                .finish();
            let mut builder = Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            if let Some(cookie) = cookie {
                builder = builder.header(header::COOKIE, cookie);
            }
            harness.send(builder.body(Body::from(body)).unwrap()).await
        }

        /// Runs the browser half of the flow and returns the authorization
        /// code the client receives.
        async fn authorize(
            harness: &Harness,
            cookie: &str,
            application: &oauth_application::Model,
        ) -> String {
            let uri = format!(
                "/login/oauth/authorize?client_id={}&redirect_uri={}&response_type=code&scope=read:repository&state=xyz",
                application.client_id,
                urlencoding(REDIRECT_URI)
            );
            let response = harness
                .send(
                    Request::builder()
                        .uri(&uri)
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
            let consent = location(&response);
            assert!(consent.starts_with("/oauth/consent?"), "{consent}");
            let consent_token = query(&consent, "consent_token").unwrap();
            assert_eq!(query(&consent, "scope").as_deref(), Some("read:repository"));

            let response = form(
                harness,
                "/login/oauth/authorize",
                Some(cookie),
                &[("consent_token", &consent_token), ("decision", "allow")],
            )
            .await;
            let callback = location(&response);
            assert!(callback.starts_with(REDIRECT_URI), "{callback}");
            assert_eq!(query(&callback, "state").as_deref(), Some("xyz"));
            query(&callback, "code").expect("the callback carries a code")
        }

        fn urlencoding(value: &str) -> String {
            url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
        }

        async fn exchange(
            harness: &Harness,
            application: &oauth_application::Model,
            client_secret: &str,
            code: &str,
            redirect_uri: &str,
        ) -> (StatusCode, Value) {
            let response = form(
                harness,
                "/login/oauth/access_token",
                None,
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", &application.client_id),
                    ("client_secret", client_secret),
                    ("code", code),
                    ("redirect_uri", redirect_uri),
                ],
            )
            .await;
            let status = response.status();
            (status, json_body(response).await)
        }

        #[tokio::test]
        async fn code_flow_issues_a_token_that_works_until_revoked() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;
            let (application, client_secret) =
                application(&harness, &alice, "Deploy", REDIRECT_URI).await;

            // Signed-out browsers are sent to log in first.
            let response = harness
                .send(
                    Request::builder()
                        .uri(format!(
                            "/login/oauth/authorize?client_id={}&redirect_uri={}&response_type=code",
                            application.client_id,
                            urlencoding(REDIRECT_URI)
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
            assert!(location(&response).starts_with("/login?returnTo="));

            let code = authorize(&harness, &cookie, &application).await;
            let (status, body) =
                exchange(&harness, &application, &client_secret, &code, REDIRECT_URI).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["token_type"], "Bearer");
            assert_eq!(body["scope"], "read:repository");
            assert_eq!(body["expires_in"], ACCESS_TOKEN_LIFETIME.num_seconds());
            let token = body["access_token"].as_str().unwrap().to_owned();
            assert!(token.starts_with("gto_"));

            let authenticated = harness
                .state
                .authenticate_token(&token, SCOPE_REPOSITORY_READ)
                .await
                .unwrap();
            assert_eq!(authenticated.user.id, alice.id);
            assert!(
                harness
                    .state
                    .authenticate_token(&token, SCOPE_WRITE)
                    .await
                    .is_err(),
                "the token carries only the granted scope"
            );

            // The code was consumed by the exchange.
            let (status, body) =
                exchange(&harness, &application, &client_secret, &code, REDIRECT_URI).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "invalid_grant");

            // The authorization is listed and can be revoked.
            let (_, body) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/authorizations",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(
                body[0]["client_id"],
                Value::String(application.client_id.clone())
            );
            let (status, _) = harness
                .request(
                    Method::DELETE,
                    &format!("/api/v1/me/authorizations/{}", application.client_id),
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
            assert!(
                harness
                    .state
                    .authenticate_token(&token, SCOPE_REPOSITORY_READ)
                    .await
                    .is_err()
            );
        }

        async fn refresh(
            harness: &Harness,
            application: &oauth_application::Model,
            client_secret: &str,
            refresh_token: &str,
        ) -> (StatusCode, Value) {
            let response = form(
                harness,
                "/login/oauth/access_token",
                None,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", &application.client_id),
                    ("client_secret", client_secret),
                    ("refresh_token", refresh_token),
                ],
            )
            .await;
            let status = response.status();
            (status, json_body(response).await)
        }

        #[tokio::test]
        async fn refresh_tokens_rotate_and_die_with_the_authorization() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;
            let (application, client_secret) =
                application(&harness, &alice, "Deploy", REDIRECT_URI).await;
            let code = authorize(&harness, &cookie, &application).await;
            let (_, body) =
                exchange(&harness, &application, &client_secret, &code, REDIRECT_URI).await;
            let first = body["refresh_token"].as_str().unwrap().to_owned();
            assert!(first.starts_with("gtr_"));

            let (status, body) = refresh(&harness, &application, "wrong", &first).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

            let (status, body) = refresh(&harness, &application, &client_secret, &first).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["scope"], "read:repository");
            let token = body["access_token"].as_str().unwrap().to_owned();
            let second = body["refresh_token"].as_str().unwrap().to_owned();
            assert_ne!(second, first);
            assert_eq!(
                harness
                    .state
                    .authenticate_token(&token, SCOPE_REPOSITORY_READ)
                    .await
                    .unwrap()
                    .user
                    .id,
                alice.id
            );

            // A refresh token is single use.
            let (status, body) = refresh(&harness, &application, &client_secret, &first).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "invalid_grant");

            // Revoking the authorization also stops refreshing.
            let (status, _) = harness
                .request(
                    Method::DELETE,
                    &format!("/api/v1/me/authorizations/{}", application.client_id),
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
            let (status, body) = refresh(&harness, &application, &client_secret, &second).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["error"], "invalid_grant");
        }

        #[tokio::test]
        async fn a_code_is_bound_to_its_redirect_uri_and_client() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;
            let (application, client_secret) =
                application(&harness, &alice, "Deploy", REDIRECT_URI).await;

            let code = authorize(&harness, &cookie, &application).await;
            let (status, body) = exchange(
                &harness,
                &application,
                &client_secret,
                &code,
                "https://client.test/other",
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["error"], "invalid_grant");
            // A mismatch burns the code; the right URI no longer redeems it.
            let (status, _) =
                exchange(&harness, &application, &client_secret, &code, REDIRECT_URI).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);

            let code = authorize(&harness, &cookie, &application).await;
            let (status, body) =
                exchange(&harness, &application, "gcs_wrong", &code, REDIRECT_URI).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
            assert_eq!(body["error"], "invalid_client");
            let (other, other_secret) =
                super::application(&harness, &alice, "Other", REDIRECT_URI).await;
            let (status, body) =
                exchange(&harness, &other, &other_secret, &code, REDIRECT_URI).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["error"], "invalid_grant");

            // Authorization requests with an unregistered redirect URI stop
            // before any redirect happens.
            let response = harness
                .send(
                    Request::builder()
                        .uri(format!(
                            "/login/oauth/authorize?client_id={}&redirect_uri={}&response_type=code",
                            application.client_id,
                            urlencoding("https://evil.test/callback")
                        ))
                        .header(header::COOKIE, &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(json_body(response).await["error"], "invalid_request");

            let response = form(
                &harness,
                "/login/oauth/access_token",
                None,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", &application.client_id),
                    ("client_secret", &client_secret),
                    ("refresh_token", "none"),
                ],
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(json_body(response).await["error"], "invalid_grant");

            let response = form(
                &harness,
                "/login/oauth/access_token",
                None,
                &[
                    ("grant_type", "client_credentials"),
                    ("client_id", &application.client_id),
                    ("client_secret", &client_secret),
                ],
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(json_body(response).await["error"], "unsupported_grant_type");
        }
    }
}
