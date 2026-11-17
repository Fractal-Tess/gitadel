//! OAuth application policy, consent and one-use grant exchange.
use crate::{
    IdentityError, Principal, SCOPE_REPOSITORY_READ, hash_secret, random_secret,
    runtime::AuthService, validate_name,
};
use chrono::Utc;
use gitadel_db::entity::{
    dokploy_source_binding, oauth_access_token, oauth_application, oauth_authorization_code,
    oauth_refresh_token,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, TransactionTrait, sea_query::Expr,
};
use std::{collections::BTreeMap, time::Instant};
use url::Url;
use uuid::Uuid;
const AUTHORIZATION_CODE_LIFETIME: chrono::Duration = chrono::Duration::minutes(10);
/// Clients trade their refresh token for a new pair when this passes.
pub const ACCESS_TOKEN_LIFETIME: chrono::Duration = chrono::Duration::days(30);
/// Refresh tokens rotate on every use, so this only lapses for a client that
/// has not refreshed for a year.
const REFRESH_TOKEN_LIFETIME: chrono::Duration = chrono::Duration::days(365);
const MAX_OAUTH_PARAMETER_LENGTH: usize = 2_048;

pub struct AuthorizationRequest {
    application_id: Uuid,
    user_id: Uuid,
    redirect_uri: String,
    scope: String,
    state: Option<String>,
    pub created_at: Instant,
}

pub struct OauthApplicationResponse {
    pub id: Uuid,
    pub name: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub created_at: chrono::DateTime<Utc>,
}
pub struct CreateOauthApplicationRequest {
    pub name: String,
    pub redirect_uri: String,
}
pub struct CreatedOauthApplicationResponse {
    pub client_secret: String,
    pub application: OauthApplicationResponse,
}
pub struct AuthorizeQuery {
    pub client_id: String,
    pub redirect_uri: String,
    pub response_type: String,
    pub scope: Option<String>,
    pub state: Option<String>,
}
pub struct ApproveRequest {
    pub consent_token: String,
    pub decision: String,
}
pub struct TokenRequest {
    pub grant_type: String,
    pub client_id: String,
    pub client_secret: String,
    // Optional so an unsupported grant (Gitea clients try refresh_token) is
    // answered with an OAuth error object instead of a form-decoding failure.
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub refresh_token: Option<String>,
}

pub struct AuthorizationResponse {
    pub application_name: String,
    pub client_id: String,
    /// Every scope any of the live tokens carries.
    pub scopes: Vec<String>,
    /// When the oldest live token was issued.
    pub created_at: chrono::DateTime<Utc>,
    pub last_used_at: Option<chrono::DateTime<Utc>>,
    /// When the newest live token expires; the client must re-authorise then.
    pub expires_at: chrono::DateTime<Utc>,
    pub token_count: usize,
}
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
}
impl TokenPair {
    pub fn generate() -> Self {
        Self {
            access_token: format!("gto_{}", random_secret(32)),
            refresh_token: format!("gtr_{}", random_secret(32)),
        }
    }

    /// Store the pair for `user_id` and return when the access token expires.
    pub async fn store<C: ConnectionTrait>(
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
pub async fn find_application(
    state: &AuthService<'_>,
    client_id: &str,
    redirect_uri: &str,
) -> Result<Option<oauth_application::Model>, sea_orm::DbErr> {
    oauth_application::Entity::find()
        .filter(oauth_application::Column::ClientId.eq(client_id))
        .filter(oauth_application::Column::RedirectUri.eq(redirect_uri))
        .one(state.database())
        .await
}
pub fn validate_redirect_uri(value: &str) -> Result<String, IdentityError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_OAUTH_PARAMETER_LENGTH {
        return Err(IdentityError::bad_request(
            "Redirect URI must contain between 1 and 2048 characters.",
        ));
    }
    let parsed = Url::parse(value)
        .map_err(|_| IdentityError::bad_request("Redirect URI must be an absolute HTTP(S) URL."))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(IdentityError::bad_request(
            "Redirect URI must be an absolute HTTP(S) URL without credentials or a fragment.",
        ));
    }
    Ok(value.to_owned())
}
pub fn authorize_query_lengths_valid(query: &AuthorizeQuery) -> bool {
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
pub fn oauth_scope_bits(scope: &str) -> i32 {
    if scope
        .split_ascii_whitespace()
        .any(|scope| matches!(scope, "read:repository" | "repo"))
    {
        SCOPE_REPOSITORY_READ
    } else {
        0
    }
}
pub fn normalize_scope(scope: &str) -> Result<String, &'static str> {
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
pub async fn list_applications(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<Vec<OauthApplicationResponse>, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    let applications = oauth_application::Entity::find()
        .filter(oauth_application::Column::UserId.eq(actor.user.id))
        .order_by_asc(oauth_application::Column::CreatedAt)
        .all(state.database())
        .await?;
    Ok(applications
        .into_iter()
        .map(OauthApplicationResponse::from)
        .collect())
}
pub async fn create_application(
    state: &AuthService<'_>,
    actor: &Principal,
    request: CreateOauthApplicationRequest,
) -> Result<CreatedOauthApplicationResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    if actor.via_api_token {
        return Err(IdentityError::forbidden(
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

    Ok(CreatedOauthApplicationResponse {
        client_secret,
        application: application.into(),
    })
}
pub async fn delete_application(
    state: &AuthService<'_>,
    actor: &Principal,
    id: Uuid,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    if actor.via_api_token {
        return Err(IdentityError::forbidden(
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
        return Err(IdentityError::not_found());
    }
    if dokploy_source_binding::Entity::find()
        .filter(dokploy_source_binding::Column::OauthApplicationId.eq(id))
        .one(&transaction)
        .await?
        .is_some()
    {
        return Err(IdentityError::conflict(
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
    Ok(())
}
pub async fn list_authorizations(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<Vec<AuthorizationResponse>, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    let now = Utc::now();
    let tokens = oauth_access_token::Entity::find()
        .filter(oauth_access_token::Column::UserId.eq(actor.user.id))
        .filter(oauth_access_token::Column::RevokedAt.is_null())
        .filter(oauth_access_token::Column::ExpiresAt.gt(now))
        .order_by_asc(oauth_access_token::Column::CreatedAt)
        .all(state.database())
        .await?;
    if tokens.is_empty() {
        return Ok(Vec::new());
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
    Ok(authorizations)
}
pub async fn revoke_authorization(
    state: &AuthService<'_>,
    actor: &Principal,
    client_id: String,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let application = oauth_application::Entity::find()
        .filter(oauth_application::Column::ClientId.eq(&client_id))
        .one(state.database())
        .await?
        .ok_or_else(IdentityError::not_found)?;
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
        return Err(IdentityError::not_found());
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
    Ok(())
}
pub enum OAuthOutcome {
    Failure(IdentityError),
    Rejected {
        unauthorized: bool,
        error: String,
        description: String,
    },
    Redirect(String),
    RedirectError {
        redirect_uri: String,
        error: String,
        description: String,
        state: Option<String>,
    },
    Authorized {
        redirect_uri: String,
        code: String,
        state: Option<String>,
    },
    Tokens {
        pair: TokenPair,
        scope: String,
    },
}
fn oauth_error(unauthorized: bool, error: &str, description: &str) -> OAuthOutcome {
    OAuthOutcome::Rejected {
        unauthorized,
        error: error.to_owned(),
        description: description.to_owned(),
    }
}
fn authorization_error_redirect(
    uri: &str,
    error: &str,
    description: &str,
    state: Option<&str>,
) -> OAuthOutcome {
    OAuthOutcome::RedirectError {
        redirect_uri: uri.to_owned(),
        error: error.to_owned(),
        description: description.to_owned(),
        state: state.map(str::to_owned),
    }
}
fn authorization_success_redirect(uri: &str, code: &str, state: Option<&str>) -> OAuthOutcome {
    OAuthOutcome::Authorized {
        redirect_uri: uri.to_owned(),
        code: code.to_owned(),
        state: state.map(str::to_owned),
    }
}
fn token_response(pair: TokenPair, scope: String) -> OAuthOutcome {
    OAuthOutcome::Tokens { pair, scope }
}
pub async fn authorize(
    state: &AuthService<'_>,
    raw_session: Option<&str>,
    request_uri: &str,
    query: AuthorizeQuery,
) -> OAuthOutcome {
    if !authorize_query_lengths_valid(&query) {
        return oauth_error(
            false,
            "invalid_request",
            "The authorization request contains an invalid parameter.",
        );
    }
    let application = match find_application(state, &query.client_id, &query.redirect_uri).await {
        Ok(Some(application)) => application,
        Ok(None) => {
            return oauth_error(
                false,
                "invalid_request",
                "Unknown OAuth application or redirect URI.",
            );
        }
        Err(error) => return OAuthOutcome::Failure(error.into()),
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
    let account = match state.session_user(raw_session).await {
        Ok(Some(account)) => account,
        Ok(None) => {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            serializer.append_pair("returnTo", request_uri);
            return OAuthOutcome::Redirect(format!("/login?{}", serializer.finish()));
        }
        Err(error) => return OAuthOutcome::Failure(error),
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
    OAuthOutcome::Redirect(format!("/oauth/consent?{}", serializer.finish()))
}
pub async fn approve(
    state: &AuthService<'_>,
    raw_session: Option<&str>,
    form: ApproveRequest,
) -> OAuthOutcome {
    let account = match state.session_user(raw_session).await {
        Ok(Some(account)) => account,
        Ok(None) => {
            return oauth_error(
                true,
                "access_denied",
                "Sign in to authorize this application.",
            );
        }
        Err(error) => return OAuthOutcome::Failure(error),
    };
    let Some(request) = state
        .authorization_requests()
        .await
        .remove(&hash_secret(&form.consent_token))
    else {
        return oauth_error(
            false,
            "invalid_request",
            "The authorization request expired.",
        );
    };
    if request.user_id != account.id {
        return oauth_error(
            false,
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
        Err(error) => return OAuthOutcome::Failure(error.into()),
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
        return OAuthOutcome::Failure(error.into());
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
        return OAuthOutcome::Failure(error);
    }
    if let Err(error) = transaction.commit().await {
        return OAuthOutcome::Failure(error.into());
    }

    authorization_success_redirect(&request.redirect_uri, &code, request.state.as_deref())
}
pub async fn access_token(state: &AuthService<'_>, request: TokenRequest) -> OAuthOutcome {
    if !matches!(
        request.grant_type.as_str(),
        "authorization_code" | "refresh_token"
    ) {
        return oauth_error(
            false,
            "unsupported_grant_type",
            "Gitadel supports the authorization_code and refresh_token grants.",
        );
    }
    if request.grant_type == "refresh_token" {
        return refresh_access_token(state, request).await;
    }
    let (Some(code), Some(redirect_uri)) = (request.code, request.redirect_uri) else {
        return oauth_error(
            false,
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
            false,
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
                true,
                "invalid_client",
                "The OAuth client credentials were not accepted.",
            );
        }
        Err(error) => return OAuthOutcome::Failure(error.into()),
    };

    let transaction = match state.database().begin().await {
        Ok(transaction) => transaction,
        Err(error) => return OAuthOutcome::Failure(error.into()),
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
                false,
                "invalid_grant",
                "The authorization code is invalid or expired.",
            );
        }
        Err(error) => return OAuthOutcome::Failure(error.into()),
    };
    if stored.expires_at <= Utc::now() || stored.redirect_uri != redirect_uri {
        if let Err(error) = oauth_authorization_code::Entity::delete_by_id(code_hash)
            .exec(&transaction)
            .await
        {
            return OAuthOutcome::Failure(error.into());
        }
        if let Err(error) = transaction.commit().await {
            return OAuthOutcome::Failure(error.into());
        }
        return oauth_error(
            false,
            "invalid_grant",
            "The authorization code is invalid or expired.",
        );
    }
    let deleted = match oauth_authorization_code::Entity::delete_by_id(code_hash)
        .exec(&transaction)
        .await
    {
        Ok(deleted) => deleted,
        Err(error) => return OAuthOutcome::Failure(error.into()),
    };
    if deleted.rows_affected != 1 {
        return oauth_error(
            false,
            "invalid_grant",
            "The authorization code is invalid or expired.",
        );
    }

    let pair = TokenPair::generate();
    if let Err(error) = pair
        .store(&transaction, application.id, stored.user_id, &stored.scope)
        .await
    {
        return OAuthOutcome::Failure(error.into());
    }
    if let Err(error) = transaction.commit().await {
        return OAuthOutcome::Failure(error.into());
    }
    token_response(pair, stored.scope)
}
pub async fn refresh_access_token(state: &AuthService<'_>, request: TokenRequest) -> OAuthOutcome {
    let Some(refresh_token) = request.refresh_token else {
        return oauth_error(
            false,
            "invalid_request",
            "The token request is missing the refresh_token parameter.",
        );
    };
    if [&request.client_id, &request.client_secret, &refresh_token]
        .iter()
        .any(|value| value.is_empty() || value.len() > MAX_OAUTH_PARAMETER_LENGTH)
    {
        return oauth_error(
            false,
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
                true,
                "invalid_client",
                "The OAuth client credentials were not accepted.",
            );
        }
        Err(error) => return OAuthOutcome::Failure(error.into()),
    };
    let transaction = match state.database().begin().await {
        Ok(transaction) => transaction,
        Err(error) => return OAuthOutcome::Failure(error.into()),
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
                false,
                "invalid_grant",
                "The refresh token is invalid or expired.",
            );
        }
        Err(error) => return OAuthOutcome::Failure(error.into()),
    };
    match oauth_refresh_token::Entity::delete_by_id(token_hash)
        .exec(&transaction)
        .await
    {
        Ok(deleted) if deleted.rows_affected == 1 => {}
        Ok(_) => {
            return oauth_error(
                false,
                "invalid_grant",
                "The refresh token is invalid or expired.",
            );
        }
        Err(error) => return OAuthOutcome::Failure(error.into()),
    }
    let pair = TokenPair::generate();
    if let Err(error) = pair
        .store(&transaction, application.id, stored.user_id, &stored.scope)
        .await
    {
        return OAuthOutcome::Failure(error.into());
    }
    if let Err(error) = transaction.commit().await {
        return OAuthOutcome::Failure(error.into());
    }
    token_response(pair, stored.scope)
}
