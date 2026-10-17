use std::{collections::BTreeMap, time::Instant};

use axum::{
    Form, Json,
    extract::{OriginalUri, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use super::{
    ApiError, IdentityState, SCOPE_READ, SCOPE_REPOSITORY_READ, SCOPE_WRITE, hash_secret,
    random_secret, validate_name,
};
use crate::entity::{
    dokploy_source_binding, oauth_access_token, oauth_application, oauth_authorization_code,
    oauth_refresh_token,
};

const AUTHORIZATION_CODE_LIFETIME: chrono::Duration = chrono::Duration::minutes(10);
/// Clients trade their refresh token for a new pair when this passes.
pub(crate) const ACCESS_TOKEN_LIFETIME: chrono::Duration = chrono::Duration::days(30);
/// Refresh tokens rotate on every use, so this only lapses for a client that
/// has not refreshed for a year.
const REFRESH_TOKEN_LIFETIME: chrono::Duration = chrono::Duration::days(365);
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
    if oauth_application::Entity::find_by_id(id)
        .filter(oauth_application::Column::UserId.eq(actor.user.id))
        .one(&transaction)
        .await?
        .is_none()
    {
        return Err(ApiError::not_found());
    }
    if dokploy_source_binding::Entity::find()
        .filter(dokploy_source_binding::Column::OauthApplicationId.eq(id))
        .one(&transaction)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict(
            "This OAuth application is managed by a Dokploy integration. Disconnect that source first.",
        ));
    }
    oauth_application::Entity::delete_by_id(id)
        .exec(&transaction)
        .await?;
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

    if application.user_id != account.id {
        return authorization_error_redirect(
            &query.redirect_uri,
            "access_denied",
            "Gitadel OAuth applications can authorize only their owner.",
            query.state.as_deref(),
        );
    }

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

    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("consent_token", &consent_token);
    serializer.append_pair("application", &application.name);
    serializer.append_pair("username", &account.username);
    serializer.append_pair("scope", &scope);
    Redirect::to(&format!("/oauth/consent?{}", serializer.finish())).into_response()
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
    let transaction = match state.database().begin().await {
        Ok(transaction) => transaction,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let stored = oauth_authorization_code::ActiveModel {
        code_hash: Set(hash_secret(&code)),
        application_id: Set(request.application_id),
        user_id: Set(request.user_id),
        redirect_uri: Set(request.redirect_uri.clone()),
        scope: Set(request.scope),
        expires_at: Set(now + AUTHORIZATION_CODE_LIFETIME),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await;
    if let Err(error) = stored {
        return ApiError::from(error).into_response();
    }
    if let Err(error) = state
        .audit_on(
            &transaction,
            Some(request.user_id),
            "oauth_application.authorize",
            Some(request.application_id.to_string()),
        )
        .await
    {
        return error.into_response();
    }
    if let Err(error) = transaction.commit().await {
        return ApiError::from(error).into_response();
    }

    authorization_success_redirect(&request.redirect_uri, &code, request.state.as_deref())
}

#[derive(Deserialize)]
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

#[derive(Serialize)]
pub struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    /// Seconds until the token expires (RFC 6749 section 5.1).
    expires_in: i64,
    refresh_token: String,
    scope: String,
}

/// A freshly generated access and refresh token. Only their hashes are stored.
pub(crate) struct TokenPair {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
}

impl TokenPair {
    pub(crate) fn generate() -> Self {
        Self {
            access_token: format!("gto_{}", random_secret(32)),
            refresh_token: format!("gtr_{}", random_secret(32)),
        }
    }

    /// Store the pair for `user_id` and return when the access token expires.
    pub(crate) async fn store<C: ConnectionTrait>(
        &self,
        connection: &C,
        application_id: Uuid,
        user_id: Uuid,
        scope: &str,
    ) -> Result<chrono::DateTime<Utc>, DbErr> {
        let now = Utc::now();
        let expires_at = now + ACCESS_TOKEN_LIFETIME;
        oauth_access_token::ActiveModel {
            token_hash: Set(hash_secret(&self.access_token)),
            application_id: Set(application_id),
            user_id: Set(user_id),
            scopes: Set(oauth_scope_bits(scope)),
            scope: Set(scope.to_owned()),
            created_at: Set(now),
            expires_at: Set(expires_at),
            last_used_at: Set(None),
            revoked_at: Set(None),
        }
        .insert(connection)
        .await?;
        oauth_refresh_token::ActiveModel {
            token_hash: Set(hash_secret(&self.refresh_token)),
            application_id: Set(application_id),
            user_id: Set(user_id),
            scope: Set(scope.to_owned()),
            created_at: Set(now),
            expires_at: Set(now + REFRESH_TOKEN_LIFETIME),
        }
        .insert(connection)
        .await?;
        Ok(expires_at)
    }
}

pub async fn access_token(
    State(state): State<IdentityState>,
    Form(request): Form<TokenRequest>,
) -> Response {
    if !matches!(
        request.grant_type.as_str(),
        "authorization_code" | "refresh_token"
    ) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "Gitadel supports the authorization_code and refresh_token grants.",
        );
    }
    if request.grant_type == "refresh_token" {
        return refresh_access_token(&state, request).await;
    }
    let (Some(code), Some(redirect_uri)) = (request.code, request.redirect_uri) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The token request is missing the code or redirect_uri parameter.",
        );
    };
    if [
        &request.client_id,
        &request.client_secret,
        &code,
        &redirect_uri,
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
    let code_hash = hash_secret(&code);
    let stored = match oauth_authorization_code::Entity::find_by_id(&code_hash)
        .filter(oauth_authorization_code::Column::ApplicationId.eq(application.id))
        .one(&transaction)
        .await
    {
        Ok(Some(stored)) => stored,
        Ok(None) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                "The authorization code is invalid or expired.",
            );
        }
        Err(error) => return ApiError::from(error).into_response(),
    };
    if stored.expires_at <= Utc::now() || stored.redirect_uri != redirect_uri {
        if let Err(error) = oauth_authorization_code::Entity::delete_by_id(code_hash)
            .exec(&transaction)
            .await
        {
            return ApiError::from(error).into_response();
        }
        if let Err(error) = transaction.commit().await {
            return ApiError::from(error).into_response();
        }
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "The authorization code is invalid or expired.",
        );
    }
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

    let pair = TokenPair::generate();
    if let Err(error) = pair
        .store(&transaction, application.id, stored.user_id, &stored.scope)
        .await
    {
        return ApiError::from(error).into_response();
    }
    if let Err(error) = transaction.commit().await {
        return ApiError::from(error).into_response();
    }
    token_response(pair, stored.scope)
}

/// Trade a refresh token for a new pair. The old refresh token is consumed,
/// so a stolen copy stops working as soon as the client refreshes.
async fn refresh_access_token(state: &IdentityState, request: TokenRequest) -> Response {
    let Some(refresh_token) = request.refresh_token else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The token request is missing the refresh_token parameter.",
        );
    };
    if [&request.client_id, &request.client_secret, &refresh_token]
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
    let token_hash = hash_secret(&refresh_token);
    let stored = match oauth_refresh_token::Entity::find_by_id(&token_hash)
        .filter(oauth_refresh_token::Column::ApplicationId.eq(application.id))
        .one(&transaction)
        .await
    {
        Ok(Some(stored)) if stored.expires_at > Utc::now() => stored,
        Ok(_) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                "The refresh token is invalid or expired.",
            );
        }
        Err(error) => return ApiError::from(error).into_response(),
    };
    match oauth_refresh_token::Entity::delete_by_id(token_hash)
        .exec(&transaction)
        .await
    {
        Ok(deleted) if deleted.rows_affected == 1 => {}
        Ok(_) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                "The refresh token is invalid or expired.",
            );
        }
        Err(error) => return ApiError::from(error).into_response(),
    }
    let pair = TokenPair::generate();
    if let Err(error) = pair
        .store(&transaction, application.id, stored.user_id, &stored.scope)
        .await
    {
        return ApiError::from(error).into_response();
    }
    if let Err(error) = transaction.commit().await {
        return ApiError::from(error).into_response();
    }
    token_response(pair, stored.scope)
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
#[derive(Serialize)]
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
pub async fn list_authorizations(
    State(state): State<IdentityState>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<AuthorizationResponse>>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    let now = Utc::now();
    let tokens = oauth_access_token::Entity::find()
        .filter(oauth_access_token::Column::UserId.eq(actor.user.id))
        .filter(oauth_access_token::Column::RevokedAt.is_null())
        .filter(oauth_access_token::Column::ExpiresAt.gt(now))
        .order_by_asc(oauth_access_token::Column::CreatedAt)
        .all(state.database())
        .await?;
    if tokens.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let applications = oauth_application::Entity::find()
        .filter(
            oauth_application::Column::Id.is_in(tokens.iter().map(|token| token.application_id)),
        )
        .all(state.database())
        .await?
        .into_iter()
        .map(|application| (application.id, application))
        .collect::<BTreeMap<_, _>>();
    let mut grouped: BTreeMap<Uuid, AuthorizationResponse> = BTreeMap::new();
    for token in tokens {
        let Some(application) = applications.get(&token.application_id) else {
            continue;
        };
        let entry = grouped
            .entry(token.application_id)
            .or_insert_with(|| AuthorizationResponse {
                application_name: application.name.clone(),
                client_id: application.client_id.clone(),
                scopes: Vec::new(),
                created_at: token.created_at,
                last_used_at: None,
                expires_at: token.expires_at,
                token_count: 0,
            });
        for scope in token.scope.split_ascii_whitespace() {
            if !entry.scopes.iter().any(|known| known == scope) {
                entry.scopes.push(scope.to_owned());
            }
        }
        entry.created_at = entry.created_at.min(token.created_at);
        entry.expires_at = entry.expires_at.max(token.expires_at);
        entry.last_used_at = entry.last_used_at.max(token.last_used_at);
        entry.token_count += 1;
    }
    let mut authorizations = grouped.into_values().collect::<Vec<_>>();
    authorizations.sort_by_key(|entry| entry.created_at);
    Ok(Json(authorizations))
}

/// Revokes every access token the application holds for the user.
pub async fn revoke_authorization(
    State(state): State<IdentityState>,
    axum::extract::Path(client_id): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let application = oauth_application::Entity::find()
        .filter(oauth_application::Column::ClientId.eq(&client_id))
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let transaction = state.database().begin().await?;
    let revoked = oauth_access_token::Entity::update_many()
        .col_expr(
            oauth_access_token::Column::RevokedAt,
            Expr::value(Some(Utc::now())),
        )
        .filter(oauth_access_token::Column::UserId.eq(actor.user.id))
        .filter(oauth_access_token::Column::ApplicationId.eq(application.id))
        .filter(oauth_access_token::Column::RevokedAt.is_null())
        .exec(&transaction)
        .await?;
    // Without this the client could mint a fresh access token right away.
    let refreshes = oauth_refresh_token::Entity::delete_many()
        .filter(oauth_refresh_token::Column::UserId.eq(actor.user.id))
        .filter(oauth_refresh_token::Column::ApplicationId.eq(application.id))
        .exec(&transaction)
        .await?;
    if revoked.rows_affected == 0 && refreshes.rows_affected == 0 {
        return Err(ApiError::not_found());
    }
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "oauth_authorization.revoke",
            Some(application.id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
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

fn oauth_scope_bits(scope: &str) -> i32 {
    if scope
        .split_ascii_whitespace()
        .any(|scope| matches!(scope, "read:repository" | "repo"))
    {
        SCOPE_REPOSITORY_READ
    } else {
        0
    }
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
