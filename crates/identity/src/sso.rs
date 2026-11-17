use std::{
    collections::HashMap,
    net::IpAddr,
    time::{Duration, Instant},
};

use chrono::Utc;
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    reqwest,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseTransaction,
    EntityTrait, PaginatorTrait, QueryFilter, TransactionTrait,
};
use subtle::ConstantTimeEq as _;
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

use gitadel_db::entity::{instance, namespace, oidc_identity, oidc_provider, passkey, user};

use crate::{
    IdentityError, Principal, hash_password, hash_secret, random_secret, runtime::AuthService,
};
use gitadel_secrets::SecretKind;
const OIDC_REQUEST_LIFETIME: Duration = Duration::from_secs(10 * 60);
const OIDC_AUTHORIZATION_CAPACITY: usize = 1_024;
const OIDC_SOURCE_CAPACITY: usize = 16;

pub struct OidcAuthorization {
    provider_id: Uuid,
    nonce: Nonce,
    pkce_verifier: PkceCodeVerifier,
    return_to: String,
    created_at: Instant,
    transaction_hash: String,
    remote_address: Option<IpAddr>,
}

pub type OidcAuthorizations = Mutex<HashMap<String, OidcAuthorization>>;

pub struct PublicOidcProvider {
    pub id: Uuid,
    pub name: String,
}

pub struct AuthenticationConfiguration {
    pub password_enabled: bool,
    pub passkey_enabled: bool,
    pub providers: Vec<PublicOidcProvider>,
}

pub struct StartQuery {
    pub return_to: Option<String>,
}

pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

pub struct AdminOidcProvider {
    pub id: Uuid,
    pub name: String,
    pub issuer_url: String,
    pub client_id: String,
    pub has_client_secret: bool,
    pub enabled: bool,
    pub auto_provision: bool,
    pub callback_url: String,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

pub struct UpdateAuthenticationConfiguration {
    pub password_enabled: bool,
    pub passkey_enabled: bool,
}

pub struct SaveProviderRequest {
    pub name: String,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub enabled: bool,
    pub auto_provision: bool,
}

pub async fn public_configuration(
    database: &sea_orm::DatabaseConnection,
) -> Result<AuthenticationConfiguration, IdentityError> {
    let settings = instance::Entity::find_by_id(1)
        .one(database)
        .await?
        .ok_or_else(|| IdentityError::internal("instance settings row is missing"))?;
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
pub(crate) async fn enabled_provider(
    database: &sea_orm::DatabaseConnection,
    provider_id: Uuid,
) -> Result<oidc_provider::Model, IdentityError> {
    oidc_provider::Entity::find_by_id(provider_id)
        .filter(oidc_provider::Column::Enabled.eq(true))
        .one(database)
        .await?
        .ok_or_else(|| IdentityError::bad_request("The identity provider is unavailable."))
}
pub(crate) async fn oidc_client(
    state: &AuthService<'_>,
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
    IdentityError,
> {
    let issuer = IssuerUrl::new(provider.issuer_url.clone())
        .map_err(|error| IdentityError::bad_request(format!("Invalid issuer URL: {error}")))?;
    let metadata = CoreProviderMetadata::discover_async(issuer, &oidc_http_client()?)
        .await
        .map_err(|error| IdentityError::bad_request(format!("OIDC discovery failed: {error}")))?;
    let secret = if provider.client_secret.is_empty() {
        None
    } else {
        Some(ClientSecret::new(state.secret_cipher().open_text(
            SecretKind::OidcClientSecret,
            &provider.id,
            &provider.client_secret,
        )?))
    };
    let redirect_url = state
        .public_url()
        .join(&format!("api/v1/auth/oidc/{}/callback", provider.id))
        .map_err(IdentityError::internal)?;
    Ok(CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(provider.client_id.clone()),
        secret,
    )
    .set_redirect_uri(
        RedirectUrl::new(redirect_url.to_string()).map_err(|error| {
            IdentityError::internal(format!("invalid OIDC callback URL: {error}"))
        })?,
    ))
}
pub(crate) fn oidc_http_client() -> Result<reqwest::Client, IdentityError> {
    reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(IdentityError::internal)
}
pub(crate) fn safe_return_to(public_url: &Url, value: Option<&str>) -> String {
    let Some(value) = value.filter(|value| {
        value.starts_with('/')
            && !value.starts_with("//")
            && !value.contains('\\')
            && !value.chars().any(char::is_control)
    }) else {
        return "/".to_owned();
    };
    let Ok(candidate) = public_url.join(value) else {
        return "/".to_owned();
    };
    if candidate.scheme() != public_url.scheme()
        || candidate.host_str() != public_url.host_str()
        || candidate.port_or_known_default() != public_url.port_or_known_default()
    {
        return "/".to_owned();
    }
    let mut target = candidate.path().to_owned();
    if let Some(query) = candidate.query() {
        target.push('?');
        target.push_str(query);
    }
    if let Some(fragment) = candidate.fragment() {
        target.push('#');
        target.push_str(fragment);
    }
    target
}
pub(crate) fn successful_return_to(return_to: &str, provider_id: Uuid) -> String {
    let (target, fragment) = return_to
        .split_once('#')
        .map_or((return_to, None), |(target, fragment)| {
            (target, Some(fragment))
        });
    let separator = if target.contains('?') { '&' } else { '?' };
    let mut result = format!("{target}{separator}loginMethod=sso%3A{provider_id}");
    if let Some(fragment) = fragment {
        result.push('#');
        result.push_str(fragment);
    }
    result
}
pub(crate) async fn resolve_account(
    state: &AuthService<'_>,
    provider: &oidc_provider::Model,
    subject: &str,
    username_hint: &str,
) -> Result<user::Model, IdentityError> {
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
            .ok_or_else(|| IdentityError::forbidden("This Gitadel account is disabled."));
    }
    if !provider.auto_provision {
        return Err(IdentityError::forbidden(
            "This identity is not linked to a Gitadel account.",
        ));
    }
    provision_account(state, provider.id, subject, username_hint).await
}
pub(crate) async fn provision_account(
    state: &AuthService<'_>,
    provider_id: Uuid,
    subject: &str,
    username_hint: &str,
) -> Result<user::Model, IdentityError> {
    let transaction = state.database().begin().await?;
    let username = available_username(&transaction, username_hint).await?;
    let password_hash = hash_password(random_secret(32)).await?;
    let now = Utc::now();
    let account =
        crate::accounts::insert_account_on(&transaction, &username, password_hash, false, now)
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
pub(crate) async fn available_username(
    transaction: &DatabaseTransaction,
    hint: &str,
) -> Result<String, IdentityError> {
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
    if !username_taken(transaction, &base).await? {
        return Ok(base);
    }
    for _ in 0..16 {
        let suffix = &Uuid::new_v4().simple().to_string()[..8];
        let candidate = format!("{base}-{suffix}");
        if !username_taken(transaction, &candidate).await? {
            return Ok(candidate);
        }
    }
    Err(IdentityError::internal(
        "could not allocate a username for the OIDC identity",
    ))
}
pub(crate) async fn username_taken(
    transaction: &DatabaseTransaction,
    candidate: &str,
) -> Result<bool, IdentityError> {
    if user::Entity::find()
        .filter(user::Column::Username.eq(candidate))
        .count(transaction)
        .await?
        > 0
    {
        return Ok(true);
    }
    Ok(namespace::Entity::find_by_id(candidate)
        .one(transaction)
        .await?
        .is_some())
}
pub(crate) fn admin_provider(
    state: &AuthService<'_>,
    provider: oidc_provider::Model,
) -> Result<AdminOidcProvider, IdentityError> {
    let callback_url = state
        .public_url()
        .join(&format!("api/v1/auth/oidc/{}/callback", provider.id))
        .map_err(IdentityError::internal)?
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
pub(crate) async fn ensure_admin_login_available<C: ConnectionTrait>(
    connection: &C,
    password_enabled: bool,
    passkey_enabled: bool,
) -> Result<(), IdentityError> {
    let administrator_ids = user::Entity::find()
        .filter(user::Column::IsAdmin.eq(true))
        .filter(user::Column::DisabledAt.is_null())
        .all(connection)
        .await?
        .into_iter()
        .map(|account| account.id)
        .collect::<Vec<_>>();
    if password_enabled && !administrator_ids.is_empty() {
        return Ok(());
    }
    if passkey_enabled
        && passkey::Entity::find()
            .filter(passkey::Column::UserId.is_in(administrator_ids.clone()))
            .count(connection)
            .await?
            > 0
    {
        return Ok(());
    }
    let provider_ids = oidc_provider::Entity::find()
        .filter(oidc_provider::Column::Enabled.eq(true))
        .all(connection)
        .await?
        .into_iter()
        .map(|provider| provider.id)
        .collect::<Vec<_>>();
    if !provider_ids.is_empty()
        && oidc_identity::Entity::find()
            .filter(oidc_identity::Column::UserId.is_in(administrator_ids))
            .filter(oidc_identity::Column::ProviderId.is_in(provider_ids))
            .count(connection)
            .await?
            > 0
    {
        return Ok(());
    }
    Err(IdentityError::bad_request(
        "At least one enabled administrator login credential must remain.",
    ))
}
pub(crate) fn validate_provider_request(
    mut request: SaveProviderRequest,
    existing_secret: Option<&str>,
) -> Result<SaveProviderRequest, IdentityError> {
    request.name = request.name.trim().to_owned();
    request.issuer_url = request.issuer_url.trim().trim_end_matches('/').to_owned();
    request.client_id = request.client_id.trim().to_owned();
    if request.name.is_empty() || request.name.len() > 80 {
        return Err(IdentityError::bad_request(
            "Provider name must contain 1 to 80 characters.",
        ));
    }
    if request.client_id.is_empty() || request.client_id.len() > 512 {
        return Err(IdentityError::bad_request(
            "Client ID must contain 1 to 512 characters.",
        ));
    }
    let issuer = Url::parse(&request.issuer_url)
        .map_err(|_| IdentityError::bad_request("Issuer URL must be an absolute URL."))?;
    let loopback = issuer.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if issuer.scheme() != "https" && !(issuer.scheme() == "http" && loopback) {
        return Err(IdentityError::bad_request(
            "Issuer URL must use HTTPS, except for loopback development providers.",
        ));
    }
    request.client_secret = match request.client_secret.take() {
        Some(secret) if !secret.trim().is_empty() => Some(secret.trim().to_owned()),
        Some(_) | None => existing_secret.map(str::to_owned),
    };
    Ok(request)
}
pub(crate) fn seal_client_secret(
    state: &AuthService<'_>,
    provider_id: Uuid,
    secret: Option<&str>,
) -> Result<String, IdentityError> {
    match secret.filter(|secret| !secret.is_empty()) {
        Some(secret) => state
            .secret_cipher()
            .seal_text(SecretKind::OidcClientSecret, &provider_id, secret)
            .map_err(IdentityError::internal),
        None => Ok(String::new()),
    }
}
pub(crate) async fn validate_discovery(issuer_url: &str) -> Result<(), IdentityError> {
    let issuer = IssuerUrl::new(issuer_url.to_owned())
        .map_err(|error| IdentityError::bad_request(format!("Invalid issuer URL: {error}")))?;
    CoreProviderMetadata::discover_async(issuer, &oidc_http_client()?)
        .await
        .map_err(|error| IdentityError::bad_request(format!("OIDC discovery failed: {error}")))?;
    Ok(())
}
pub async fn complete_callback(
    state: &AuthService<'_>,
    provider_id: Uuid,
    query: CallbackQuery,
    transaction_secret: Option<&str>,
) -> Result<(String, String), IdentityError> {
    if let Some(error) = query.error {
        let description = query.error_description.unwrap_or(error);
        return Err(IdentityError::bad_request(format!(
            "The identity provider rejected the login: {description}"
        )));
    }
    let csrf = query
        .state
        .ok_or_else(|| IdentityError::bad_request("The OIDC state is missing."))?;
    let presented_hash = transaction_secret.map(hash_secret);
    let mut authorizations = state.oidc_authorizations().lock().await;
    let authorization = authorizations.get(&csrf).ok_or_else(|| {
        IdentityError::bad_request("The OIDC login request is missing or expired.")
    })?;
    let transaction_matches = presented_hash.as_ref().is_some_and(|presented| {
        bool::from(
            presented
                .as_bytes()
                .ct_eq(authorization.transaction_hash.as_bytes()),
        )
    });
    if authorization.provider_id != provider_id
        || authorization.created_at.elapsed() > OIDC_REQUEST_LIFETIME
        || !transaction_matches
    {
        return Err(IdentityError::bad_request(
            "The OIDC login request is missing or expired.",
        ));
    }
    let authorization = authorizations
        .remove(&csrf)
        .expect("checked OIDC authorization must remain present");
    drop(authorizations);
    let code = query
        .code
        .ok_or_else(|| IdentityError::bad_request("The authorization code is missing."))?;
    let provider = enabled_provider(state.database(), provider_id).await?;
    let client = oidc_client(state, &provider).await?;
    let http_client = oidc_http_client()?;
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|error| {
            IdentityError::internal(format!("OIDC token endpoint is unavailable: {error}"))
        })?
        .set_pkce_verifier(authorization.pkce_verifier)
        .request_async(&http_client)
        .await
        .map_err(|error| IdentityError::internal(format!("OIDC code exchange failed: {error}")))?;
    let id_token = token.extra_fields().id_token().ok_or_else(|| {
        IdentityError::bad_request("The identity provider did not return an ID token.")
    })?;
    let claims = id_token
        .claims(&client.id_token_verifier(), &authorization.nonce)
        .map_err(|error| {
            IdentityError::bad_request(format!(
                "The identity provider returned an invalid ID token: {error}"
            ))
        })?;
    let subject = claims.subject().as_str().to_owned();
    let username_hint = claims
        .preferred_username()
        .map(|value| value.as_str().to_owned())
        .or_else(|| claims.email().map(|value| value.as_str().to_owned()))
        .unwrap_or_else(|| format!("sso-{}", &subject[..subject.len().min(12)]));
    let account = resolve_account(state, &provider, &subject, &username_hint).await?;
    let transaction = state.database().begin().await?;
    let cookie = state.create_session_on(&transaction, account.id).await?;
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
    Ok((
        cookie,
        successful_return_to(&authorization.return_to, provider.id),
    ))
}
pub async fn admin_configuration(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<AuthenticationConfiguration, IdentityError> {
    let _actor = state.require_actor(actor, true, crate::SCOPE_READ).await?;
    public_configuration(state.database()).await
}
pub async fn update_configuration(
    state: &AuthService<'_>,
    actor: &Principal,
    request: UpdateAuthenticationConfiguration,
) -> Result<AuthenticationConfiguration, IdentityError> {
    let actor = state.require_actor(actor, true, crate::SCOPE_WRITE).await?;
    let transaction = state.database().begin().await?;
    let settings = instance::Entity::find_by_id(1)
        .one(&transaction)
        .await?
        .ok_or_else(|| IdentityError::internal("instance settings row is missing"))?;
    let mut active: instance::ActiveModel = settings.into();
    active.password_login_enabled = Set(request.password_enabled);
    active.passkey_login_enabled = Set(request.passkey_enabled);
    active.updated_at = Set(Utc::now());
    active.update(&transaction).await?;
    ensure_admin_login_available(
        &transaction,
        request.password_enabled,
        request.passkey_enabled,
    )
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "admin.authentication.update",
            None,
        )
        .await?;
    transaction.commit().await?;
    public_configuration(state.database()).await
}
pub async fn list_providers(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<Vec<AdminOidcProvider>, IdentityError> {
    let _actor = state.require_actor(actor, true, crate::SCOPE_READ).await?;
    let providers = oidc_provider::Entity::find().all(state.database()).await?;
    providers
        .into_iter()
        .map(|provider| admin_provider(state, provider))
        .collect::<Result<_, _>>()
}
pub async fn create_provider(
    state: &AuthService<'_>,
    actor: &Principal,
    request: SaveProviderRequest,
) -> Result<AdminOidcProvider, IdentityError> {
    let actor = state.require_actor(actor, true, crate::SCOPE_WRITE).await?;
    let values = validate_provider_request(request, None)?;
    validate_discovery(&values.issuer_url).await?;
    let now = Utc::now();
    let id = Uuid::new_v4();
    let client_secret = seal_client_secret(state, id, values.client_secret.as_deref())?;
    let provider = oidc_provider::ActiveModel {
        id: Set(id),
        name: Set(values.name),
        issuer_url: Set(values.issuer_url),
        client_id: Set(values.client_id),
        client_secret: Set(client_secret),
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
    admin_provider(state, provider)
}
pub async fn update_provider(
    state: &AuthService<'_>,
    actor: &Principal,
    provider_id: Uuid,
    request: SaveProviderRequest,
) -> Result<AdminOidcProvider, IdentityError> {
    let actor = state.require_actor(actor, true, crate::SCOPE_WRITE).await?;
    let current = oidc_provider::Entity::find_by_id(provider_id)
        .one(state.database())
        .await?
        .ok_or_else(|| IdentityError::bad_request("The identity provider does not exist."))?;
    let values = validate_provider_request(request, None)?;
    validate_discovery(&values.issuer_url).await?;
    // An omitted secret keeps the stored one, in whatever form it is stored.
    let client_secret = match values.client_secret.as_deref() {
        Some(secret) => seal_client_secret(state, provider_id, Some(secret))?,
        None => current.client_secret.clone(),
    };
    let transaction = state.database().begin().await?;
    let provider = oidc_provider::Entity::find_by_id(provider_id)
        .one(&transaction)
        .await?
        .ok_or_else(|| IdentityError::bad_request("The identity provider does not exist."))?;
    let mut active: oidc_provider::ActiveModel = provider.into();
    active.name = Set(values.name);
    active.issuer_url = Set(values.issuer_url);
    active.client_id = Set(values.client_id);
    active.client_secret = Set(client_secret);
    active.enabled = Set(values.enabled);
    active.auto_provision = Set(values.auto_provision);
    active.updated_at = Set(Utc::now());
    let provider = active.update(&transaction).await?;
    if !provider.enabled {
        let settings = instance::Entity::find_by_id(1)
            .one(&transaction)
            .await?
            .ok_or_else(|| IdentityError::internal("instance settings row is missing"))?;
        ensure_admin_login_available(
            &transaction,
            settings.password_login_enabled,
            settings.passkey_login_enabled,
        )
        .await?;
    }
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "admin.oidc_provider.update",
            Some(provider.id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    admin_provider(state, provider)
}
pub async fn delete_provider(
    state: &AuthService<'_>,
    actor: &Principal,
    provider_id: Uuid,
) -> Result<(), IdentityError> {
    let actor = state.require_actor(actor, true, crate::SCOPE_WRITE).await?;
    let transaction = state.database().begin().await?;
    let deleted = oidc_provider::Entity::delete_by_id(provider_id)
        .exec(&transaction)
        .await?;
    if deleted.rows_affected == 0 {
        return Err(IdentityError::bad_request(
            "The identity provider does not exist.",
        ));
    }
    let settings = instance::Entity::find_by_id(1)
        .one(&transaction)
        .await?
        .ok_or_else(|| IdentityError::internal("instance settings row is missing"))?;
    ensure_admin_login_available(
        &transaction,
        settings.password_login_enabled,
        settings.passkey_login_enabled,
    )
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "admin.oidc_provider.delete",
            Some(provider_id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}
pub async fn start(
    state: &AuthService<'_>,
    provider_id: Uuid,
    query: StartQuery,
) -> Result<(String, String), IdentityError> {
    state.enforce_auth_rate_limit("oidc-start", 30).await?;
    let provider = enabled_provider(state.database(), provider_id).await?;
    let client = oidc_client(state, &provider).await?;
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
    let return_to = safe_return_to(state.public_url(), query.return_to.as_deref());
    let transaction_secret = random_secret(32);
    let source = state.audit.remote_address;
    let mut authorizations = state.oidc_authorizations().lock().await;
    authorizations
        .retain(|_, authorization| authorization.created_at.elapsed() <= OIDC_REQUEST_LIFETIME);
    if authorizations.len() >= OIDC_AUTHORIZATION_CAPACITY
        || authorizations
            .values()
            .filter(|authorization| authorization.remote_address == source)
            .count()
            >= OIDC_SOURCE_CAPACITY
    {
        return Err(IdentityError::too_many_requests(
            "Too many OIDC login attempts. Try again later.",
        ));
    }
    authorizations.insert(
        csrf.secret().clone(),
        OidcAuthorization {
            provider_id,
            nonce,
            pkce_verifier,
            return_to,
            created_at: Instant::now(),
            transaction_hash: hash_secret(&transaction_secret),
            remote_address: source,
        },
    );
    drop(authorizations);
    Ok((transaction_secret, authorization_url.to_string()))
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
        let origin = Url::parse("https://gitadel.example").unwrap();
        assert_eq!(
            safe_return_to(&origin, Some("/settings?view=security")),
            "/settings?view=security"
        );
        assert_eq!(
            safe_return_to(&origin, Some("/\\attacker.example/path")),
            "/"
        );
        assert_eq!(
            safe_return_to(&origin, Some("//attacker.example/path")),
            "/"
        );
        assert_eq!(
            safe_return_to(&origin, Some("https://attacker.example")),
            "/"
        );
        assert_eq!(safe_return_to(&origin, None), "/");
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

    #[tokio::test]
    async fn provisioned_usernames_avoid_organization_namespaces() {
        let database = gitadel_db::test_support::database().await;
        let now = Utc::now();
        let organization_id = Uuid::new_v4();
        gitadel_db::entity::organization::ActiveModel {
            id: Set(organization_id),
            slug: Set("acme".to_owned()),
            display_name: Set("Acme".to_owned()),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        namespace::ActiveModel {
            slug: Set("acme".to_owned()),
            kind: Set("organization".to_owned()),
            user_id: Set(None),
            organization_id: Set(Some(organization_id)),
            created_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        gitadel_db::test_support::owner(&database, "taken").await;

        let transaction = database.begin().await.unwrap();
        let beside_organization = available_username(&transaction, "acme@example.com")
            .await
            .ok()
            .unwrap();
        let beside_user = available_username(&transaction, "taken")
            .await
            .ok()
            .unwrap();
        let free = available_username(&transaction, "New.Person@example.com")
            .await
            .ok()
            .unwrap();

        assert!(
            beside_organization.starts_with("acme-"),
            "{beside_organization}"
        );
        assert!(beside_user.starts_with("taken-"), "{beside_user}");
        assert_eq!(free, "new-person");
        // The chosen name must be insertable as a namespace.
        namespace::ActiveModel {
            slug: Set(beside_organization),
            kind: Set("user".to_owned()),
            user_id: Set(None),
            organization_id: Set(None),
            created_at: Set(now),
        }
        .insert(&transaction)
        .await
        .unwrap();
    }
}
