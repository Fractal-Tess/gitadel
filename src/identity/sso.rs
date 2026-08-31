use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    reqwest,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

use crate::entity::{instance, namespace, oidc_identity, oidc_provider, passkey, user};

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, hash_password, random_secret};

const OIDC_REQUEST_LIFETIME: Duration = Duration::from_secs(10 * 60);

pub(super) struct OidcAuthorization {
    provider_id: Uuid,
    nonce: Nonce,
    pkce_verifier: PkceCodeVerifier,
    return_to: String,
    created_at: Instant,
}

pub(super) type OidcAuthorizations = Mutex<HashMap<String, OidcAuthorization>>;

#[derive(Clone, Serialize)]
pub struct PublicOidcProvider {
    id: Uuid,
    name: String,
}

#[derive(Serialize)]
pub struct AuthenticationConfiguration {
    pub(super) password_enabled: bool,
    pub(super) passkey_enabled: bool,
    providers: Vec<PublicOidcProvider>,
}

pub async fn public_configuration(
    database: &sea_orm::DatabaseConnection,
) -> Result<AuthenticationConfiguration, ApiError> {
    let settings = instance::Entity::find_by_id(1)
        .one(database)
        .await?
        .ok_or_else(|| ApiError::internal("instance settings row is missing"))?;
    let providers = oidc_provider::Entity::find()
        .filter(oidc_provider::Column::Enabled.eq(true))
        .all(database)
        .await?
        .into_iter()
        .map(|provider| PublicOidcProvider {
            id: provider.id,
            name: provider.name,
        })
        .collect();
    Ok(AuthenticationConfiguration {
        password_enabled: settings.password_login_enabled,
        passkey_enabled: settings.passkey_login_enabled,
        providers,
    })
}

#[derive(Deserialize)]
pub struct StartQuery {
    #[serde(default, rename = "returnTo")]
    return_to: Option<String>,
}

pub async fn start(
    State(state): State<IdentityState>,
    Path(provider_id): Path<Uuid>,
    Query(query): Query<StartQuery>,
) -> Result<Redirect, ApiError> {
    let provider = enabled_provider(state.database(), provider_id).await?;
    let client = oidc_client(&state, &provider).await?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (authorization_url, csrf, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("email".to_owned()))
        .add_scope(Scope::new("profile".to_owned()))
        .set_pkce_challenge(pkce_challenge)
        .url();
    let return_to = safe_return_to(query.return_to.as_deref());
    state.oidc_authorizations().lock().await.insert(
        csrf.secret().clone(),
        OidcAuthorization {
            provider_id,
            nonce,
            pkce_verifier,
            return_to,
            created_at: Instant::now(),
        },
    );
    Ok(Redirect::to(authorization_url.as_str()))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub async fn callback(
    State(state): State<IdentityState>,
    Path(provider_id): Path<Uuid>,
    jar: CookieJar,
    Query(query): Query<CallbackQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if let Some(error) = query.error {
        let description = query.error_description.unwrap_or(error);
        return Err(ApiError::bad_request(format!(
            "The identity provider rejected the login: {description}"
        )));
    }
    let csrf = query
        .state
        .ok_or_else(|| ApiError::bad_request("The OIDC state is missing."))?;
    let authorization = state
        .oidc_authorizations()
        .lock()
        .await
        .remove(&csrf)
        .ok_or_else(|| ApiError::bad_request("The OIDC login request is missing or expired."))?;
    if authorization.provider_id != provider_id
        || authorization.created_at.elapsed() > OIDC_REQUEST_LIFETIME
    {
        return Err(ApiError::bad_request(
            "The OIDC login request is missing or expired.",
        ));
    }
    let code = query
        .code
        .ok_or_else(|| ApiError::bad_request("The authorization code is missing."))?;
    let provider = enabled_provider(state.database(), provider_id).await?;
    let client = oidc_client(&state, &provider).await?;
    let http_client = oidc_http_client()?;
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|error| {
            ApiError::internal(format!("OIDC token endpoint is unavailable: {error}"))
        })?
        .set_pkce_verifier(authorization.pkce_verifier)
        .request_async(&http_client)
        .await
        .map_err(|error| ApiError::internal(format!("OIDC code exchange failed: {error}")))?;
    let id_token = token.extra_fields().id_token().ok_or_else(|| {
        ApiError::bad_request("The identity provider did not return an ID token.")
    })?;
    let claims = id_token
        .claims(&client.id_token_verifier(), &authorization.nonce)
        .map_err(|error| {
            ApiError::bad_request(format!(
                "The identity provider returned an invalid ID token: {error}"
            ))
        })?;
    let subject = claims.subject().as_str().to_owned();
    let username_hint = claims
        .preferred_username()
        .map(|value| value.as_str().to_owned())
        .or_else(|| claims.email().map(|value| value.as_str().to_owned()))
        .unwrap_or_else(|| format!("sso-{}", &subject[..subject.len().min(12)]));
    let account = resolve_account(&state, &provider, &subject, &username_hint).await?;
    let transaction = state.database().begin().await?;
    let (_, cookie) = state.create_session_on(&transaction, account.id).await?;
    state
        .audit_on(
            &transaction,
            Some(account.id),
            "auth.login.oidc",
            Some(provider.id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    tracing::info!(user_id = %account.id, provider_id = %provider.id, "OIDC login completed");
    let return_to = successful_return_to(&authorization.return_to, provider.id);
    Ok((jar.add(cookie), Redirect::to(&return_to)))
}

async fn enabled_provider(
    database: &sea_orm::DatabaseConnection,
    provider_id: Uuid,
) -> Result<oidc_provider::Model, ApiError> {
    oidc_provider::Entity::find_by_id(provider_id)
        .filter(oidc_provider::Column::Enabled.eq(true))
        .one(database)
        .await?
        .ok_or_else(|| ApiError::bad_request("The identity provider is unavailable."))
}

async fn oidc_client(
    state: &IdentityState,
    provider: &oidc_provider::Model,
) -> Result<
    CoreClient<
        EndpointSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointMaybeSet,
        EndpointMaybeSet,
    >,
    ApiError,
> {
    let issuer = IssuerUrl::new(provider.issuer_url.clone())
        .map_err(|error| ApiError::bad_request(format!("Invalid issuer URL: {error}")))?;
    let metadata = CoreProviderMetadata::discover_async(issuer, &oidc_http_client()?)
        .await
        .map_err(|error| ApiError::bad_request(format!("OIDC discovery failed: {error}")))?;
    let secret = (!provider.client_secret.is_empty())
        .then(|| ClientSecret::new(provider.client_secret.clone()));
    let redirect_url = state
        .public_url()
        .join(&format!("api/v1/auth/oidc/{}/callback", provider.id))
        .map_err(ApiError::internal)?;
    Ok(CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(provider.client_id.clone()),
        secret,
    )
    .set_redirect_uri(
        RedirectUrl::new(redirect_url.to_string())
            .map_err(|error| ApiError::internal(format!("invalid OIDC callback URL: {error}")))?,
    ))
}

fn oidc_http_client() -> Result<reqwest::Client, ApiError> {
    reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(ApiError::internal)
}

fn safe_return_to(value: Option<&str>) -> String {
    match value {
        Some(value) if value.starts_with('/') && !value.starts_with("//") => value.to_owned(),
        _ => "/".to_owned(),
    }
}

fn successful_return_to(return_to: &str, provider_id: Uuid) -> String {
    let separator = if return_to.contains('?') { '&' } else { '?' };
    format!("{return_to}{separator}loginMethod=sso%3A{provider_id}")
}

async fn resolve_account(
    state: &IdentityState,
    provider: &oidc_provider::Model,
    subject: &str,
    username_hint: &str,
) -> Result<user::Model, ApiError> {
    if let Some(identity) = oidc_identity::Entity::find()
        .filter(oidc_identity::Column::ProviderId.eq(provider.id))
        .filter(oidc_identity::Column::Subject.eq(subject))
        .one(state.database())
        .await?
    {
        return user::Entity::find_by_id(identity.user_id)
            .filter(user::Column::DisabledAt.is_null())
            .one(state.database())
            .await?
            .ok_or_else(|| ApiError::forbidden("This Gitadel account is disabled."));
    }
    if !provider.auto_provision {
        return Err(ApiError::forbidden(
            "This identity is not linked to a Gitadel account.",
        ));
    }
    provision_account(state, provider.id, subject, username_hint).await
}

async fn provision_account(
    state: &IdentityState,
    provider_id: Uuid,
    subject: &str,
    username_hint: &str,
) -> Result<user::Model, ApiError> {
    let transaction = state.database().begin().await?;
    let username = available_username(&transaction, username_hint).await?;
    let password_hash = hash_password(random_secret(32)).await?;
    let now = Utc::now();
    let account = user::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set(username.clone()),
        password_hash: Set(password_hash),
        is_admin: Set(false),
        default_repository_visibility: Set("private".to_owned()),
        theme_preference: Set("system".to_owned()),
        disabled_at: Set(None),
        avatar_updated_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    namespace::ActiveModel {
        slug: Set(username.clone()),
        kind: Set("user".to_owned()),
        user_id: Set(Some(account.id)),
        organization_id: Set(None),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    oidc_identity::ActiveModel {
        id: Set(Uuid::new_v4()),
        provider_id: Set(provider_id),
        subject: Set(subject.to_owned()),
        user_id: Set(account.id),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    transaction.commit().await?;
    Ok(account)
}

async fn available_username(
    transaction: &DatabaseTransaction,
    hint: &str,
) -> Result<String, ApiError> {
    let local_part = hint.split('@').next().unwrap_or(hint);
    let mut base = local_part
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    while base.contains("--") {
        base = base.replace("--", "-");
    }
    base = base.trim_matches('-').chars().take(30).collect();
    if base.is_empty() {
        base = "sso-user".to_owned();
    }
    if user::Entity::find()
        .filter(user::Column::Username.eq(&base))
        .count(transaction)
        .await?
        == 0
    {
        return Ok(base);
    }
    for _ in 0..16 {
        let suffix = &Uuid::new_v4().simple().to_string()[..8];
        let candidate = format!("{base}-{suffix}");
        if user::Entity::find()
            .filter(user::Column::Username.eq(&candidate))
            .count(transaction)
            .await?
            == 0
        {
            return Ok(candidate);
        }
    }
    Err(ApiError::internal(
        "could not allocate a username for the OIDC identity",
    ))
}

#[derive(Serialize)]
pub struct AdminOidcProvider {
    id: Uuid,
    name: String,
    issuer_url: String,
    client_id: String,
    has_client_secret: bool,
    enabled: bool,
    auto_provision: bool,
    callback_url: String,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

fn admin_provider(
    state: &IdentityState,
    provider: oidc_provider::Model,
) -> Result<AdminOidcProvider, ApiError> {
    let callback_url = state
        .public_url()
        .join(&format!("api/v1/auth/oidc/{}/callback", provider.id))
        .map_err(ApiError::internal)?
        .to_string();
    Ok(AdminOidcProvider {
        id: provider.id,
        name: provider.name,
        issuer_url: provider.issuer_url,
        client_id: provider.client_id,
        has_client_secret: !provider.client_secret.is_empty(),
        enabled: provider.enabled,
        auto_provision: provider.auto_provision,
        callback_url,
        created_at: provider.created_at,
        updated_at: provider.updated_at,
    })
}

pub async fn admin_configuration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<AuthenticationConfiguration>, ApiError> {
    super::admin::require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(public_configuration(state.database()).await?))
}

#[derive(Deserialize)]
pub struct UpdateAuthenticationConfiguration {
    password_enabled: bool,
    passkey_enabled: bool,
}

pub async fn update_configuration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateAuthenticationConfiguration>,
) -> Result<Json<AuthenticationConfiguration>, ApiError> {
    let actor = super::admin::require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    validate_method_configuration(&state, &request).await?;
    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ApiError::internal("instance settings row is missing"))?;
    let mut active: instance::ActiveModel = settings.into();
    active.password_login_enabled = Set(request.password_enabled);
    active.passkey_login_enabled = Set(request.passkey_enabled);
    active.updated_at = Set(Utc::now());
    active.update(state.database()).await?;
    state
        .audit(Some(actor.user.id), "admin.authentication.update", None)
        .await?;
    Ok(Json(public_configuration(state.database()).await?))
}

async fn validate_method_configuration(
    state: &IdentityState,
    request: &UpdateAuthenticationConfiguration,
) -> Result<(), ApiError> {
    let enabled_provider_count = oidc_provider::Entity::find()
        .filter(oidc_provider::Column::Enabled.eq(true))
        .count(state.database())
        .await?;
    if !request.password_enabled && !request.passkey_enabled && enabled_provider_count == 0 {
        return Err(ApiError::bad_request(
            "At least one login method must remain enabled.",
        ));
    }
    if !request.password_enabled
        && request.passkey_enabled
        && enabled_provider_count == 0
        && passkey::Entity::find().count(state.database()).await? == 0
    {
        return Err(ApiError::bad_request(
            "Register at least one passkey before disabling password login.",
        ));
    }
    Ok(())
}

pub async fn list_providers(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<AdminOidcProvider>>, ApiError> {
    super::admin::require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let providers = oidc_provider::Entity::find().all(state.database()).await?;
    Ok(Json(
        providers
            .into_iter()
            .map(|provider| admin_provider(&state, provider))
            .collect::<Result<_, _>>()?,
    ))
}

#[derive(Deserialize)]
pub struct SaveProviderRequest {
    name: String,
    issuer_url: String,
    client_id: String,
    client_secret: Option<String>,
    enabled: bool,
    auto_provision: bool,
}

pub async fn create_provider(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<SaveProviderRequest>,
) -> Result<(StatusCode, Json<AdminOidcProvider>), ApiError> {
    let actor = super::admin::require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let values = validate_provider_request(request, None)?;
    validate_discovery(&values.issuer_url).await?;
    let now = Utc::now();
    let provider = oidc_provider::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set(values.name),
        issuer_url: Set(values.issuer_url),
        client_id: Set(values.client_id),
        client_secret: Set(values.client_secret.unwrap_or_default()),
        enabled: Set(values.enabled),
        auto_provision: Set(values.auto_provision),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(state.database())
    .await?;
    state
        .audit(
            Some(actor.user.id),
            "admin.oidc_provider.create",
            Some(provider.id.to_string()),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(admin_provider(&state, provider)?)))
}

pub async fn update_provider(
    State(state): State<IdentityState>,
    Path(provider_id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<SaveProviderRequest>,
) -> Result<Json<AdminOidcProvider>, ApiError> {
    let actor = super::admin::require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let provider = oidc_provider::Entity::find_by_id(provider_id)
        .one(state.database())
        .await?
        .ok_or_else(|| ApiError::bad_request("The identity provider does not exist."))?;
    let values = validate_provider_request(request, Some(provider.client_secret.as_str()))?;
    validate_discovery(&values.issuer_url).await?;
    if !values.enabled {
        ensure_provider_removal_is_safe(&state, provider_id).await?;
    }
    let mut active: oidc_provider::ActiveModel = provider.into();
    active.name = Set(values.name);
    active.issuer_url = Set(values.issuer_url);
    active.client_id = Set(values.client_id);
    active.client_secret = Set(values.client_secret.unwrap_or_default());
    active.enabled = Set(values.enabled);
    active.auto_provision = Set(values.auto_provision);
    active.updated_at = Set(Utc::now());
    let provider = active.update(state.database()).await?;
    state
        .audit(
            Some(actor.user.id),
            "admin.oidc_provider.update",
            Some(provider.id.to_string()),
        )
        .await?;
    Ok(Json(admin_provider(&state, provider)?))
}

pub async fn delete_provider(
    State(state): State<IdentityState>,
    Path(provider_id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<axum::http::StatusCode, ApiError> {
    let actor = super::admin::require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    ensure_provider_removal_is_safe(&state, provider_id).await?;
    let deleted = oidc_provider::Entity::delete_by_id(provider_id)
        .exec(state.database())
        .await?;
    if deleted.rows_affected == 0 {
        return Err(ApiError::bad_request(
            "The identity provider does not exist.",
        ));
    }
    state
        .audit(
            Some(actor.user.id),
            "admin.oidc_provider.delete",
            Some(provider_id.to_string()),
        )
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn ensure_provider_removal_is_safe(
    state: &IdentityState,
    provider_id: Uuid,
) -> Result<(), ApiError> {
    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ApiError::internal("instance settings row is missing"))?;
    if settings.password_login_enabled
        || (settings.passkey_login_enabled
            && passkey::Entity::find().count(state.database()).await? > 0)
    {
        return Ok(());
    }
    let remaining = oidc_provider::Entity::find()
        .filter(oidc_provider::Column::Enabled.eq(true))
        .filter(oidc_provider::Column::Id.ne(provider_id))
        .count(state.database())
        .await?;
    if remaining == 0 {
        return Err(ApiError::bad_request(
            "Enable another login method before removing the last identity provider.",
        ));
    }
    Ok(())
}

fn validate_provider_request(
    mut request: SaveProviderRequest,
    existing_secret: Option<&str>,
) -> Result<SaveProviderRequest, ApiError> {
    request.name = request.name.trim().to_owned();
    request.issuer_url = request.issuer_url.trim().trim_end_matches('/').to_owned();
    request.client_id = request.client_id.trim().to_owned();
    if request.name.is_empty() || request.name.len() > 80 {
        return Err(ApiError::bad_request(
            "Provider name must contain 1 to 80 characters.",
        ));
    }
    if request.client_id.is_empty() || request.client_id.len() > 512 {
        return Err(ApiError::bad_request(
            "Client ID must contain 1 to 512 characters.",
        ));
    }
    let issuer = Url::parse(&request.issuer_url)
        .map_err(|_| ApiError::bad_request("Issuer URL must be an absolute URL."))?;
    let loopback = issuer.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if issuer.scheme() != "https" && !(issuer.scheme() == "http" && loopback) {
        return Err(ApiError::bad_request(
            "Issuer URL must use HTTPS, except for loopback development providers.",
        ));
    }
    request.client_secret = match request.client_secret.take() {
        Some(secret) if !secret.trim().is_empty() => Some(secret.trim().to_owned()),
        Some(_) | None => existing_secret.map(str::to_owned),
    };
    Ok(request)
}

async fn validate_discovery(issuer_url: &str) -> Result<(), ApiError> {
    let issuer = IssuerUrl::new(issuer_url.to_owned())
        .map_err(|error| ApiError::bad_request(format!("Invalid issuer URL: {error}")))?;
    CoreProviderMetadata::discover_async(issuer, &oidc_http_client()?)
        .await
        .map_err(|error| ApiError::bad_request(format!("OIDC discovery failed: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_request(issuer_url: &str) -> SaveProviderRequest {
        SaveProviderRequest {
            name: " Example Identity ".to_owned(),
            issuer_url: issuer_url.to_owned(),
            client_id: " gitadel ".to_owned(),
            client_secret: None,
            enabled: true,
            auto_provision: true,
        }
    }

    #[test]
    fn return_targets_stay_on_the_gitadel_origin() {
        assert_eq!(
            safe_return_to(Some("/settings?view=security")),
            "/settings?view=security"
        );
        assert_eq!(safe_return_to(Some("//attacker.example/path")), "/");
        assert_eq!(safe_return_to(Some("https://attacker.example")), "/");
        assert_eq!(safe_return_to(None), "/");
    }

    #[test]
    fn successful_return_records_the_provider_without_losing_query_parameters() {
        let provider_id = Uuid::nil();
        assert_eq!(
            successful_return_to("/", provider_id),
            "/?loginMethod=sso%3A00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(
            successful_return_to("/settings?view=security", provider_id),
            "/settings?view=security&loginMethod=sso%3A00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn provider_validation_requires_https_except_on_loopback() {
        assert!(
            validate_provider_request(provider_request("https://identity.example/realm/"), None)
                .is_ok()
        );
        assert!(
            validate_provider_request(provider_request("http://127.0.0.1:18080/realm/"), None)
                .is_ok()
        );
        assert!(
            validate_provider_request(provider_request("http://identity.example/realm/"), None)
                .is_err()
        );
    }

    #[test]
    fn provider_updates_retain_an_existing_secret_when_omitted() {
        let validated = validate_provider_request(
            provider_request("https://identity.example/realm/"),
            Some("existing-secret"),
        )
        .expect("valid provider");

        assert_eq!(validated.name, "Example Identity");
        assert_eq!(validated.issuer_url, "https://identity.example/realm");
        assert_eq!(validated.client_id, "gitadel");
        assert_eq!(validated.client_secret.as_deref(), Some("existing-secret"));
    }
}
