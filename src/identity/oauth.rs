use std::time::Instant;

use axum::{
    Form, Json,
    extract::{OriginalUri, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use super::{
    ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, hash_secret, random_secret, validate_name,
};
use crate::entity::{oauth_access_token, oauth_application, oauth_authorization_code};

const AUTHORIZATION_CODE_LIFETIME: chrono::Duration = chrono::Duration::minutes(10);
const MAX_OAUTH_PARAMETER_LENGTH: usize = 2_048;

pub(super) struct AuthorizationRequest {
    application_id: Uuid,
    user_id: Uuid,
    redirect_uri: String,
    scope: String,
    state: Option<String>,
    pub(super) created_at: Instant,
}

#[derive(Serialize)]
pub struct OauthApplicationResponse {
    id: Uuid,
    name: String,
    client_id: String,
    redirect_uri: String,
    created_at: chrono::DateTime<Utc>,
}

impl From<oauth_application::Model> for OauthApplicationResponse {
    fn from(application: oauth_application::Model) -> Self {
        Self {
            id: application.id,
            name: application.name,
            client_id: application.client_id,
            redirect_uri: application.redirect_uri,
            created_at: application.created_at,
        }
    }
}

#[derive(Deserialize)]
pub struct CreateOauthApplicationRequest {
    name: String,
    redirect_uri: String,
}

#[derive(Serialize)]
pub struct CreatedOauthApplicationResponse {
    client_secret: String,
    application: OauthApplicationResponse,
}

pub async fn list_applications(
    State(state): State<IdentityState>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<OauthApplicationResponse>>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    let applications = oauth_application::Entity::find()
        .filter(oauth_application::Column::UserId.eq(actor.user.id))
        .order_by_asc(oauth_application::Column::CreatedAt)
        .all(state.database())
        .await?;
    Ok(Json(
        applications
            .into_iter()
            .map(OauthApplicationResponse::from)
            .collect(),
    ))
}

pub async fn create_application(
    State(state): State<IdentityState>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateOauthApplicationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    if actor.via_api_token {
        return Err(ApiError::forbidden(
            "Create OAuth applications from a browser session.",
        ));
    }
    let name = validate_name(&request.name, "Application name")?;
    let redirect_uri = validate_redirect_uri(&request.redirect_uri)?;
    let client_id = format!("goc_{}", random_secret(24));
    let client_secret = format!("gcs_{}", random_secret(32));
    let now = Utc::now();
    let transaction = state.database().begin().await?;
    let application = oauth_application::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(actor.user.id),
        name: Set(name),
        client_id: Set(client_id),
        client_secret_hash: Set(hash_secret(&client_secret)),
        redirect_uri: Set(redirect_uri),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "oauth_application.create",
            Some(application.id.to_string()),
        )
        .await?;
    transaction.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(CreatedOauthApplicationResponse {
            client_secret,
            application: application.into(),
        }),
    ))
}

pub async fn delete_application(
    State(state): State<IdentityState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    if actor.via_api_token {
        return Err(ApiError::forbidden(
            "Delete OAuth applications from a browser session.",
        ));
    }
    let transaction = state.database().begin().await?;
    let deleted = oauth_application::Entity::delete_many()
        .filter(oauth_application::Column::Id.eq(id))
        .filter(oauth_application::Column::UserId.eq(actor.user.id))
        .exec(&transaction)
        .await?;
    if deleted.rows_affected == 0 {
        return Err(ApiError::not_found());
    }
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "oauth_application.delete",
            Some(id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
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
    if !authorize_query_lengths_valid(&query) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The authorization request contains an invalid parameter.",
        );
    }
    let application = match find_application(&state, &query.client_id, &query.redirect_uri).await {
        Ok(Some(application)) => application,
        Ok(None) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Unknown OAuth application or redirect URI.",
            );
        }
        Err(error) => return ApiError::from(error).into_response(),
    };
    if query.response_type != "code" {
        return authorization_error_redirect(
            &query.redirect_uri,
            "unsupported_response_type",
            "Gitadel supports only the authorization code flow.",
            query.state.as_deref(),
        );
    }
    let scope = match normalize_scope(query.scope.as_deref().unwrap_or("read:repository")) {
        Ok(scope) => scope,
        Err(description) => {
            return authorization_error_redirect(
                &query.redirect_uri,
                "invalid_scope",
                description,
                query.state.as_deref(),
            );
        }
    };
    let account = match state.session_user(&jar).await {
        Ok(Some(account)) => account,
        Ok(None) => {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            serializer.append_pair("returnTo", &uri.to_string());
            return Redirect::to(&format!("/login?{}", serializer.finish())).into_response();
        }
        Err(error) => return error.into_response(),
    };

    let consent_token = random_secret(32);
    state.authorization_requests().await.insert(
        hash_secret(&consent_token),
        AuthorizationRequest {
            application_id: application.id,
            user_id: account.id,
            redirect_uri: query.redirect_uri,
            scope: scope.clone(),
            state: query.state,
            created_at: Instant::now(),
        },
    );

    Html(consent_page(
        &application.name,
        &account.username,
        &scope,
        &consent_token,
    ))
    .into_response()
}

#[derive(Deserialize)]
pub struct ApproveRequest {
    consent_token: String,
    decision: String,
}

pub async fn approve(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Form(form): Form<ApproveRequest>,
) -> Response {
    let account = match state.session_user(&jar).await {
        Ok(Some(account)) => account,
        Ok(None) => {
            return oauth_error(
                StatusCode::UNAUTHORIZED,
                "access_denied",
                "Sign in to authorize this application.",
            );
        }
        Err(error) => return error.into_response(),
    };
    let Some(request) = state
        .authorization_requests()
        .await
        .remove(&hash_secret(&form.consent_token))
    else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The authorization request expired.",
        );
    };
    if request.user_id != account.id {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The authorization request is not valid for this account.",
        );
    }
    if form.decision != "allow" {
        return authorization_error_redirect(
            &request.redirect_uri,
            "access_denied",
            "The resource owner denied the request.",
            request.state.as_deref(),
        );
    }

    let code = format!("goc_{}", random_secret(32));
    let now = Utc::now();
    let stored = oauth_authorization_code::ActiveModel {
        code_hash: Set(hash_secret(&code)),
        application_id: Set(request.application_id),
        user_id: Set(request.user_id),
        redirect_uri: Set(request.redirect_uri.clone()),
        scope: Set(request.scope),
        expires_at: Set(now + AUTHORIZATION_CODE_LIFETIME),
        created_at: Set(now),
    }
    .insert(state.database())
    .await;
    if let Err(error) = stored {
        return ApiError::from(error).into_response();
    }

    authorization_success_redirect(&request.redirect_uri, &code, request.state.as_deref())
}

#[derive(Deserialize)]
pub struct TokenRequest {
    grant_type: String,
    client_id: String,
    client_secret: String,
    code: String,
    redirect_uri: String,
}

#[derive(Serialize)]
pub struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    scope: String,
}

pub async fn access_token(
    State(state): State<IdentityState>,
    Form(request): Form<TokenRequest>,
) -> Response {
    if request.grant_type != "authorization_code" {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "Gitadel supports only the authorization_code grant.",
        );
    }
    if [
        &request.client_id,
        &request.client_secret,
        &request.code,
        &request.redirect_uri,
    ]
    .iter()
    .any(|value| value.is_empty() || value.len() > MAX_OAUTH_PARAMETER_LENGTH)
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The token request contains an invalid parameter.",
        );
    }

    let application = match oauth_application::Entity::find()
        .filter(oauth_application::Column::ClientId.eq(&request.client_id))
        .filter(oauth_application::Column::ClientSecretHash.eq(hash_secret(&request.client_secret)))
        .one(state.database())
        .await
    {
        Ok(Some(application)) => application,
        Ok(None) => {
            return oauth_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "The OAuth client credentials were not accepted.",
            );
        }
        Err(error) => return ApiError::from(error).into_response(),
    };

    let transaction = match state.database().begin().await {
        Ok(transaction) => transaction,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let code_hash = hash_secret(&request.code);
    let stored = match oauth_authorization_code::Entity::find_by_id(&code_hash)
        .filter(oauth_authorization_code::Column::ApplicationId.eq(application.id))
        .one(&transaction)
        .await
    {
        Ok(Some(stored))
            if stored.expires_at > Utc::now() && stored.redirect_uri == request.redirect_uri =>
        {
            stored
        }
        Ok(_) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                "The authorization code is invalid or expired.",
            );
        }
        Err(error) => return ApiError::from(error).into_response(),
    };
    let deleted = match oauth_authorization_code::Entity::delete_by_id(code_hash)
        .exec(&transaction)
        .await
    {
        Ok(deleted) => deleted,
        Err(error) => return ApiError::from(error).into_response(),
    };
    if deleted.rows_affected != 1 {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "The authorization code is invalid or expired.",
        );
    }

    let raw_token = format!("gto_{}", random_secret(32));
    let insert = oauth_access_token::ActiveModel {
        token_hash: Set(hash_secret(&raw_token)),
        application_id: Set(application.id),
        user_id: Set(stored.user_id),
        scopes: Set(SCOPE_READ),
        scope: Set(stored.scope.clone()),
        created_at: Set(Utc::now()),
        last_used_at: Set(None),
        revoked_at: Set(None),
    }
    .insert(&transaction)
    .await;
    if let Err(error) = insert {
        return ApiError::from(error).into_response();
    }
    if let Err(error) = transaction.commit().await {
        return ApiError::from(error).into_response();
    }

    Json(TokenResponse {
        access_token: raw_token,
        token_type: "Bearer",
        scope: stored.scope,
    })
    .into_response()
}

async fn find_application(
    state: &IdentityState,
    client_id: &str,
    redirect_uri: &str,
) -> Result<Option<oauth_application::Model>, sea_orm::DbErr> {
    oauth_application::Entity::find()
        .filter(oauth_application::Column::ClientId.eq(client_id))
        .filter(oauth_application::Column::RedirectUri.eq(redirect_uri))
        .one(state.database())
        .await
}

fn validate_redirect_uri(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_OAUTH_PARAMETER_LENGTH {
        return Err(ApiError::bad_request(
            "Redirect URI must contain between 1 and 2048 characters.",
        ));
    }
    let parsed = Url::parse(value)
        .map_err(|_| ApiError::bad_request("Redirect URI must be an absolute HTTP(S) URL."))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(ApiError::bad_request(
            "Redirect URI must be an absolute HTTP(S) URL without credentials or a fragment.",
        ));
    }
    Ok(value.to_owned())
}

fn authorize_query_lengths_valid(query: &AuthorizeQuery) -> bool {
    ![
        Some(query.client_id.as_str()),
        Some(query.redirect_uri.as_str()),
        Some(query.response_type.as_str()),
        query.scope.as_deref(),
        query.state.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.is_empty() || value.len() > MAX_OAUTH_PARAMETER_LENGTH)
}

fn normalize_scope(scope: &str) -> Result<String, &'static str> {
    let mut scopes = Vec::new();
    for requested in scope.split_ascii_whitespace() {
        if !matches!(
            requested,
            "read:repository" | "read:user" | "read:organization" | "repo" | "user"
        ) {
            return Err("The application requested a scope Gitadel does not support.");
        }
        if !scopes.contains(&requested) {
            scopes.push(requested);
        }
    }
    if scopes.is_empty() {
        return Err("The application must request at least one scope.");
    }
    Ok(scopes.join(" "))
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

#[derive(Serialize)]
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

fn consent_page(application: &str, username: &str, scope: &str, consent_token: &str) -> String {
    let application = escape_html(application);
    let username = escape_html(username);
    let permissions = scope
        .split_ascii_whitespace()
        .map(scope_description)
        .map(|permission| format!("<li>{permission}</li>"))
        .collect::<String>();
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Authorize {application} · Gitadel</title><style>\
        :root{{color-scheme:dark;font-family:ui-sans-serif,system-ui,sans-serif;background:#0d0d0d;color:#f4f4f5}}*{{box-sizing:border-box}}body{{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px}}main{{width:min(100%,460px);border:1px solid #2b2b2f;border-radius:12px;background:#151516;padding:28px;box-shadow:0 24px 70px #0008}}p,li{{color:#a1a1aa;line-height:1.55}}h1{{font-size:24px;margin:8px 0}}.eyebrow{{font-size:12px;text-transform:uppercase;letter-spacing:.12em;color:#f97316}}ul{{padding-left:20px;margin:20px 0}}form{{display:grid;grid-template-columns:1fr 1fr;gap:10px;margin-top:24px}}button{{border:1px solid #34343a;border-radius:8px;padding:11px 14px;font:inherit;font-weight:600;cursor:pointer;background:#202024;color:#f4f4f5}}button[value=allow]{{background:#f4f4f5;color:#18181b;border-color:#f4f4f5}}small{{display:block;color:#71717a;margin-top:16px}}\
        </style></head><body><main><div class=\"eyebrow\">OAuth authorization</div><h1>Authorize {application}?</h1><p>Signed in as <strong>{username}</strong>. This application is requesting permission to:</p><ul>{permissions}</ul><form method=\"post\" action=\"/login/oauth/authorize\"><input type=\"hidden\" name=\"consent_token\" value=\"{consent_token}\"><button type=\"submit\" name=\"decision\" value=\"deny\">Cancel</button><button type=\"submit\" name=\"decision\" value=\"allow\">Authorize</button></form><small>You can revoke access by deleting this OAuth application in account settings.</small></main></body></html>"
    )
}

fn scope_description(scope: &str) -> &'static str {
    match scope {
        "read:repository" | "repo" => "Read and clone repositories you can access",
        "read:user" | "user" => "Read your account identity",
        "read:organization" => "Read your organization memberships",
        _ => "Access your Gitadel account",
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_scope_should_accept_dokploy_scopes() {
        let scope = normalize_scope("read:repository read:user read:organization").unwrap();
        assert_eq!(scope, "read:repository read:user read:organization");
    }

    #[test]
    fn normalize_scope_should_reject_write_scope() {
        let error = normalize_scope("write:repository").unwrap_err();
        assert_eq!(
            error,
            "The application requested a scope Gitadel does not support."
        );
    }

    #[test]
    fn validate_redirect_uri_should_reject_fragments() {
        let error = validate_redirect_uri("https://dokploy.example/callback#token").unwrap_err();
        assert_eq!(error.code, "bad_request");
    }
}
