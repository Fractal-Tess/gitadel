use std::time::Instant;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, DiscoverableKey, Passkey, PublicKeyCredential,
    RegisterPublicKeyCredential, RequestChallengeResponse,
};

use super::{
    ApiError, IdentityState, RegistrationChallenge, UserResponse, hash_password, hash_secret,
    random_secret, validate_name, validate_slug, verify_password,
};
use crate::entity::{
    action_runner, action_runner_registration_token, action_secret, action_variable, invitation,
    namespace, namespace_integration, namespace_mirror_identity, namespace_pin, passkey,
    repository, repository_alias, repository_import, repository_import_item, session, user,
};

#[derive(Serialize)]
pub struct AuthStatusResponse {
    setup_required: bool,
    authenticated: bool,
    user: Option<UserResponse>,
    authentication: super::sso::AuthenticationConfiguration,
    /// Whether outgoing email is configured; gates password reset and
    /// email settings in the interface.
    email_enabled: bool,
}

pub async fn status(
    State(state): State<IdentityState>,
    jar: CookieJar,
) -> Result<Json<AuthStatusResponse>, ApiError> {
    let setup_required = user::Entity::find().count(state.database()).await? == 0;
    let account = state.session_user(&jar).await?;
    let mut authentication = super::sso::public_configuration(state.database()).await?;
    authentication.passkey_enabled &= state.passkeys_available();
    Ok(Json(AuthStatusResponse {
        setup_required,
        authenticated: account.is_some(),
        authentication,
        user: account.map(UserResponse::from),
        email_enabled: state.mailer().is_enabled(),
    }))
}

#[derive(Deserialize)]
pub struct CredentialsRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    user: UserResponse,
}

#[derive(Serialize)]
pub struct TwoFactorChallengeResponse {
    two_factor_required: bool,
    challenge_id: String,
}

pub async fn login(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<CredentialsRequest>,
) -> Result<Response, ApiError> {
    if !super::sso::public_configuration(state.database())
        .await?
        .password_enabled
    {
        return Err(ApiError::forbidden("Password login is disabled."));
    }
    state.enforce_auth_rate_limit("password-source", 20).await?;
    let username = validate_slug(&request.username, "Username")?;
    state.enforce_account_auth_rate_limit(&username, 8).await?;
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(username))
        .filter(user::Column::DisabledAt.is_null())
        .one(state.database())
        .await?;
    let encoded = account
        .as_ref()
        .map(|account| account.password_hash.clone())
        .unwrap_or_else(|| state.dummy_password_hash());
    let valid = verify_password(request.password, encoded).await?;
    let Some(account) = account else {
        return Err(invalid_credentials());
    };
    if !valid {
        return Err(invalid_credentials());
    }

    // A correct password alone does not sign in an account with a second
    // factor; the browser receives a short-lived challenge instead.
    if super::two_factor::is_enabled(state.database(), account.id).await? {
        let challenge_id = state.insert_pending_login(account.id).await?;
        return Ok(Json(TwoFactorChallengeResponse {
            two_factor_required: true,
            challenge_id,
        })
        .into_response());
    }

    let transaction = state.database().begin().await?;
    let (_, cookie) = state.create_session_on(&transaction, account.id).await?;
    state
        .audit_on(&transaction, Some(account.id), "auth.login.password", None)
        .await?;
    transaction.commit().await?;
    Ok((
        jar.add(cookie),
        Json(AuthResponse {
            user: account.into(),
        }),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct TwoFactorLoginRequest {
    challenge_id: String,
    code: String,
}

pub async fn finish_two_factor_login(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<TwoFactorLoginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .enforce_auth_rate_limit("two-factor-source", 20)
        .await?;
    let Some(user_id) = state.pending_login_user(&request.challenge_id).await else {
        return Err(ApiError::bad_request(
            "The sign-in attempt expired. Sign in again.",
        ));
    };
    state
        .enforce_account_auth_rate_limit(&format!("two-factor:{user_id}"), 8)
        .await?;

    let transaction = state.database().begin().await?;
    let Some(factor) = super::two_factor::verify_second_factor(
        &transaction,
        state.secret_cipher(),
        user_id,
        &request.code,
    )
    .await?
    else {
        drop(transaction);
        state
            .record_pending_login_failure(&request.challenge_id)
            .await;
        return Err(super::two_factor::invalid_code());
    };
    let account = user::Entity::find_by_id(user_id)
        .filter(user::Column::DisabledAt.is_null())
        .one(&transaction)
        .await?
        .ok_or_else(invalid_credentials)?;
    let (_, cookie) = state.create_session_on(&transaction, account.id).await?;
    state
        .audit_on(
            &transaction,
            Some(account.id),
            "auth.login.password",
            Some(format!("two_factor:{}", factor.label())),
        )
        .await?;
    transaction.commit().await?;
    state.finish_pending_login(&request.challenge_id).await;
    Ok((
        jar.add(cookie),
        Json(AuthResponse {
            user: account.into(),
        }),
    ))
}

pub async fn logout(
    State(state): State<IdentityState>,
    jar: CookieJar,
) -> Result<impl IntoResponse, ApiError> {
    let actor = state.session_user(&jar).await?;
    let removal = state.delete_session(&jar).await?;
    if let Some(actor) = actor {
        state.audit(Some(actor.id), "auth.logout", None).await?;
    }
    Ok((StatusCode::NO_CONTENT, jar.remove(removal)))
}

#[derive(Deserialize)]
pub struct UpdateUsernameRequest {
    username: String,
}

pub async fn update_username(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateUsernameRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;

    let username = validate_slug(&request.username, "Username")?;
    if username == actor.user.username {
        return Ok(Json(AuthResponse {
            user: actor.user.into(),
        }));
    }

    let account = rename_account(&state, actor.user, username).await?;
    Ok(Json(AuthResponse {
        user: account.into(),
    }))
}

#[derive(Deserialize)]
pub struct UpdateRepositoryPreferencesRequest {
    default_repository_visibility: String,
}

pub async fn update_repository_preferences(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateRepositoryPreferencesRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;
    if !matches!(
        request.default_repository_visibility.as_str(),
        "public" | "private"
    ) {
        return Err(ApiError::bad_request(
            "Default repository visibility must be public or private.",
        ));
    }

    let actor_id = actor.user.id;
    let transaction = state.database().begin().await?;
    let mut account: user::ActiveModel = actor.user.into();
    account.default_repository_visibility = Set(request.default_repository_visibility);
    account.updated_at = Set(Utc::now());
    let account = account.update(&transaction).await?;
    state
        .audit_on(
            &transaction,
            Some(actor_id),
            "account.repository_preferences.update",
            None,
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(AuthResponse {
        user: account.into(),
    }))
}

#[derive(Deserialize)]
pub struct UpdateThemePreferenceRequest {
    theme_preference: String,
}

pub async fn update_theme_preference(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateThemePreferenceRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;
    if !matches!(
        request.theme_preference.as_str(),
        "system" | "light" | "dark"
    ) {
        return Err(ApiError::bad_request(
            "Theme preference must be system, light, or dark.",
        ));
    }

    let actor_id = actor.user.id;
    let theme_preference = request.theme_preference;
    let transaction = state.database().begin().await?;
    let mut account: user::ActiveModel = actor.user.into();
    account.theme_preference = Set(theme_preference.clone());
    account.updated_at = Set(Utc::now());
    let account = account.update(&transaction).await?;
    state
        .audit_on(
            &transaction,
            Some(actor_id),
            "account.theme_preference.update",
            Some(theme_preference),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(AuthResponse {
        user: account.into(),
    }))
}

#[derive(Deserialize)]
pub struct UpdateMotionPreferenceRequest {
    motion_preference: String,
}

/// Whether the interface animates: `system` follows the browser's
/// reduced-motion setting, `reduce` turns animation off everywhere.
pub async fn update_motion_preference(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateMotionPreferenceRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;
    if !matches!(request.motion_preference.as_str(), "system" | "reduce") {
        return Err(ApiError::bad_request(
            "Motion preference must be system or reduce.",
        ));
    }

    let actor_id = actor.user.id;
    let motion_preference = request.motion_preference;
    let transaction = state.database().begin().await?;
    let mut account: user::ActiveModel = actor.user.into();
    account.motion_preference = Set(motion_preference.clone());
    account.updated_at = Set(Utc::now());
    let account = account.update(&transaction).await?;
    state
        .audit_on(
            &transaction,
            Some(actor_id),
            "account.motion_preference.update",
            Some(motion_preference),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(AuthResponse {
        user: account.into(),
    }))
}

#[derive(Deserialize)]
pub struct UpdatePasswordRequest {
    current_password: String,
    new_password: String,
}

pub async fn update_password(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdatePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;
    verify_current_password(&actor.user, request.current_password).await?;
    if verify_password(
        request.new_password.clone(),
        actor.user.password_hash.clone(),
    )
    .await?
    {
        return Err(ApiError::bad_request(
            "Choose a password that differs from your current password.",
        ));
    }

    let password_hash = hash_password(request.new_password).await?;
    let current_session_hash = jar
        .get(super::SESSION_COOKIE)
        .map(|cookie| super::hash_secret(cookie.value()))
        .ok_or_else(ApiError::unauthorized)?;
    let transaction = state.database().begin().await?;
    let mut account: user::ActiveModel = actor.user.clone().into();
    account.password_hash = Set(password_hash);
    account.updated_at = Set(Utc::now());
    account.update(&transaction).await?;
    session::Entity::delete_many()
        .filter(session::Column::UserId.eq(actor.user.id))
        .filter(session::Column::TokenHash.ne(current_session_hash))
        .exec(&transaction)
        .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.password.update",
            None,
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn verify_current_password(
    account: &user::Model,
    current_password: String,
) -> Result<(), ApiError> {
    if !verify_password(current_password, account.password_hash.clone()).await? {
        return Err(ApiError::bad_request("The current password is incorrect."));
    }
    Ok(())
}

pub(super) fn require_browser_session(via_api_token: bool) -> Result<(), ApiError> {
    if via_api_token {
        return Err(ApiError::forbidden(
            "Update account credentials from a browser session.",
        ));
    }
    Ok(())
}

/// Point every row that names a namespace by slug at its new slug.
///
/// Shared by user and organisation renames so the two cannot drift apart: a
/// table missed here keeps referencing the old slug, which either fails the
/// rename on a foreign key or silently orphans the rows. The caller owns the
/// `namespaces` row itself.
pub(super) async fn move_namespace_resources<C: ConnectionTrait>(
    connection: &C,
    current_slug: &str,
    next_slug: &str,
) -> Result<(), sea_orm::DbErr> {
    macro_rules! move_rows {
        ($($entity:ident . $column:ident),+ $(,)?) => {$(
            $entity::Entity::update_many()
                .col_expr($entity::Column::$column, Expr::value(next_slug))
                .filter($entity::Column::$column.eq(current_slug))
                .exec(connection)
                .await?;
        )+};
    }
    move_rows!(
        repository.Namespace,
        repository_alias.Namespace,
        repository_import.TargetNamespace,
        repository_import_item.TargetNamespace,
        namespace_integration.Namespace,
        namespace_mirror_identity.Namespace,
        namespace_pin.Namespace,
        action_runner.Namespace,
        action_runner_registration_token.Namespace,
        action_secret.Namespace,
        action_variable.Namespace,
    );
    Ok(())
}

async fn rename_account(
    state: &IdentityState,
    account: user::Model,
    username: String,
) -> Result<user::Model, ApiError> {
    let transaction = state.database().begin().await?;
    let previous_username = account.username.clone();
    let account = rename_user_namespace(&transaction, account, &username).await?;
    state
        .audit_on(
            &transaction,
            Some(account.id),
            "account.username.update",
            Some(format!("{previous_username} -> {username}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(account)
}

/// Rename a user and their personal namespace inside the caller's transaction.
async fn rename_user_namespace(
    transaction: &DatabaseTransaction,
    account: user::Model,
    username: &str,
) -> Result<user::Model, ApiError> {
    if namespace::Entity::find_by_id(username)
        .one(transaction)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("That username is already in use."));
    }

    // The slug is a primary key that other tables reference, so the children
    // and the parent cannot both be valid until every statement has run.
    transaction
        .execute_unprepared("PRAGMA defer_foreign_keys = ON")
        .await?;
    move_namespace_resources(transaction, &account.username, username).await?;
    let namespace_update = namespace::Entity::update_many()
        .col_expr(namespace::Column::Slug, Expr::value(username))
        .filter(namespace::Column::UserId.eq(account.id))
        .exec(transaction)
        .await?;
    if namespace_update.rows_affected != 1 {
        return Err(ApiError::internal("the user namespace is missing"));
    }

    let mut active: user::ActiveModel = account.into();
    active.username = Set(username.to_owned());
    active.updated_at = Set(Utc::now());
    Ok(active.update(transaction).await?)
}

#[derive(Deserialize)]
pub struct SetupRequest {
    token: String,
    username: String,
    password: String,
}

/// Creates the first administrator from the one-time link printed at startup.
pub async fn setup(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<SetupRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if !state.setup_token_matches(&request.token).await {
        return Err(ApiError::bad_request(
            "The setup link is invalid or was already used. Restart Gitadel to print a new one.",
        ));
    }
    // bootstrap_admin refuses once any account exists, so concurrent
    // submissions cannot create a second administrator.
    let account =
        super::bootstrap_admin(state.database(), &request.username, request.password).await?;
    state.clear_setup_token().await;
    let transaction = state.database().begin().await?;
    state
        .audit_on(
            &transaction,
            Some(account.id),
            "account.setup",
            Some(account.username.clone()),
        )
        .await?;
    let (_, cookie) = state.create_session_on(&transaction, account.id).await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        jar.add(cookie),
        Json(AuthResponse {
            user: account.into(),
        }),
    ))
}

#[derive(Deserialize)]
pub struct InvitationRegistrationRequest {
    token: String,
    username: String,
    password: String,
}

pub async fn register(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<InvitationRegistrationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let username = validate_slug(&request.username, "Username")?;
    let transaction = state.database().begin().await?;
    let now = Utc::now();
    let reserved = invitation::Entity::update_many()
        .col_expr(invitation::Column::UsedAt, Expr::value(Some(now)))
        .filter(invitation::Column::TokenHash.eq(hash_secret(&request.token)))
        .filter(invitation::Column::UsedAt.is_null())
        .filter(invitation::Column::ExpiresAt.gt(now))
        .exec(&transaction)
        .await?;
    if reserved.rows_affected != 1 {
        return Err(ApiError::bad_request(
            "The invitation is invalid or has expired.",
        ));
    }
    if namespace::Entity::find_by_id(&username)
        .one(&transaction)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("That username is already in use."));
    }
    let password_hash = hash_password(request.password).await?;

    let account = user::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set(username.clone()),
        password_hash: Set(password_hash),
        is_admin: Set(false),
        default_repository_visibility: Set("private".to_owned()),
        theme_preference: Set("system".to_owned()),
        motion_preference: Set("system".to_owned()),
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
    state
        .audit_on(
            &transaction,
            Some(account.id),
            "account.register",
            Some(username),
        )
        .await?;
    let (_, cookie) = state.create_session_on(&transaction, account.id).await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        jar.add(cookie),
        Json(AuthResponse {
            user: account.into(),
        }),
    ))
}

#[derive(Deserialize)]
pub struct CreateInvitationRequest {
    expires_in_hours: Option<i64>,
}

#[derive(Serialize)]
pub struct InvitationResponse {
    token: String,
    expires_at: chrono::DateTime<Utc>,
}

pub async fn create_invitation(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateInvitationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    if !actor.user.is_admin {
        return Err(ApiError::forbidden(
            "Only an administrator can create invitations.",
        ));
    }
    let lifetime = request
        .expires_in_hours
        .map(chrono::Duration::hours)
        .unwrap_or_else(|| state.invitation_lifetime());
    if lifetime <= chrono::Duration::zero() || lifetime > chrono::Duration::days(30) {
        return Err(ApiError::bad_request(
            "Invitation lifetime must be between 1 hour and 30 days.",
        ));
    }

    let token = format!("gti_{}", random_secret(32));
    let now = Utc::now();
    let expires_at = now + lifetime;
    let transaction = state.database().begin().await?;
    invitation::ActiveModel {
        token_hash: Set(hash_secret(&token)),
        created_by: Set(actor.user.id),
        expires_at: Set(expires_at),
        used_at: Set(None),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(&transaction, Some(actor.user.id), "invitation.create", None)
        .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(InvitationResponse { token, expires_at }),
    ))
}

#[derive(Serialize)]
pub struct PasskeyResponse {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<Utc>,
    last_used_at: Option<chrono::DateTime<Utc>>,
}

pub async fn list_passkeys(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<PasskeyResponse>>, ApiError> {
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_READ)
        .await?;
    let rows = passkey::Entity::find()
        .filter(passkey::Column::UserId.eq(actor.user.id))
        .order_by_asc(passkey::Column::CreatedAt)
        .all(state.database())
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| PasskeyResponse {
                id: row.id,
                name: row.name,
                created_at: row.created_at,
                last_used_at: row.last_used_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct StartPasskeyRegistrationRequest {
    name: String,
}

#[derive(Serialize)]
pub struct PasskeyCreationResponse {
    challenge_id: String,
    options: CreationChallengeResponse,
}

pub async fn start_passkey_registration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<StartPasskeyRegistrationRequest>,
) -> Result<Json<PasskeyCreationResponse>, ApiError> {
    if !super::sso::public_configuration(state.database())
        .await?
        .passkey_enabled
    {
        return Err(ApiError::forbidden("Passkeys are disabled."));
    }
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    if actor.via_api_token {
        tracing::warn!(
            user_id = %actor.user.id,
            reason = "api_token_session",
            "passkey registration rejected"
        );
        return Err(ApiError::forbidden(
            "Register passkeys from a browser session.",
        ));
    }
    let name = validate_name(&request.name, "Passkey name")?;
    let stored = passkey::Entity::find()
        .filter(passkey::Column::UserId.eq(actor.user.id))
        .all(state.database())
        .await?;
    let mut excluded = Vec::with_capacity(stored.len());
    for row in stored {
        let credential: Passkey =
            serde_json::from_str(&row.credential).map_err(ApiError::internal)?;
        excluded.push(credential.cred_id().clone());
    }
    let excluded_count = excluded.len();
    let (options, registration) = state
        .webauthn()?
        .start_passkey_registration(
            actor.user.id,
            &actor.user.username,
            &actor.user.username,
            Some(excluded),
        )
        .map_err(|error| {
            tracing::warn!(
                %error,
                user_id = %actor.user.id,
                "could not create passkey registration challenge"
            );
            ApiError::internal(error)
        })?;
    let challenge_id = random_secret(24);
    state.registration_challenges().await.insert(
        challenge_id.clone(),
        RegistrationChallenge {
            user_id: actor.user.id,
            name,
            state: registration,
            created_at: Instant::now(),
        },
    );
    tracing::info!(
        user_id = %actor.user.id,
        excluded_credentials = excluded_count,
        "passkey registration challenge issued"
    );
    Ok(Json(PasskeyCreationResponse {
        challenge_id,
        options,
    }))
}

#[derive(Deserialize)]
pub struct FinishPasskeyRegistrationRequest {
    challenge_id: String,
    credential: RegisterPublicKeyCredential,
}

pub async fn finish_passkey_registration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<FinishPasskeyRegistrationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    let challenge = state
        .registration_challenges()
        .await
        .remove(&request.challenge_id)
        .filter(|challenge| challenge.user_id == actor.user.id);
    let Some(challenge) = challenge else {
        tracing::warn!(
            user_id = %actor.user.id,
            reason = "challenge_missing_or_wrong_user",
            "passkey registration rejected"
        );
        return Err(ApiError::bad_request(
            "The passkey registration challenge expired.",
        ));
    };
    let credential = state
        .webauthn()?
        .finish_passkey_registration(&request.credential, &challenge.state)
        .map_err(|error| {
            tracing::warn!(
                %error,
                user_id = %actor.user.id,
                "passkey registration verification failed"
            );
            ApiError::bad_request("The passkey registration could not be verified.")
        })?;
    let transaction = state.database().begin().await?;
    let row = passkey::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(actor.user.id),
        name: Set(challenge.name),
        credential_id: Set(URL_SAFE_NO_PAD.encode(credential.cred_id())),
        credential: Set(serde_json::to_string(&credential).map_err(ApiError::internal)?),
        created_at: Set(Utc::now()),
        last_used_at: Set(None),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "passkey.create",
            Some(row.id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    tracing::info!(
        user_id = %actor.user.id,
        passkey_id = %row.id,
        "passkey registration completed"
    );
    Ok(StatusCode::CREATED)
}

pub async fn delete_passkey(
    State(state): State<IdentityState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    // Removing a sign-in credential is as sensitive as adding one: a leaked
    // write token must not be able to strip an account's passkeys.
    require_browser_session(actor.via_api_token)?;
    let transaction = state.database().begin().await?;
    let result = passkey::Entity::delete_many()
        .filter(passkey::Column::Id.eq(id))
        .filter(passkey::Column::UserId.eq(actor.user.id))
        .exec(&transaction)
        .await?;
    if result.rows_affected == 0 {
        return Err(ApiError::not_found());
    }
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "passkey.delete",
            Some(id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct PasskeyRequestResponse {
    challenge_id: String,
    options: RequestChallengeResponse,
}

pub async fn start_passkey_login(
    State(state): State<IdentityState>,
) -> Result<Json<PasskeyRequestResponse>, ApiError> {
    if !super::sso::public_configuration(state.database())
        .await?
        .passkey_enabled
    {
        return Err(ApiError::forbidden("Passkey login is disabled."));
    }
    state.enforce_auth_rate_limit("passkey-start", 30).await?;
    let (options, authentication) = state
        .webauthn()?
        .start_discoverable_authentication()
        .map_err(|error| {
            tracing::warn!(%error, "could not create discoverable passkey login challenge");
            ApiError::internal(error)
        })?;
    let challenge_id = random_secret(24);
    state
        .insert_authentication_challenge(challenge_id.clone(), authentication)
        .await?;
    tracing::info!("discoverable passkey login challenge issued");
    Ok(Json(PasskeyRequestResponse {
        challenge_id,
        options,
    }))
}

#[derive(Deserialize)]
pub struct FinishPasskeyLoginRequest {
    challenge_id: String,
    credential: PublicKeyCredential,
}

pub async fn finish_passkey_login(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<FinishPasskeyLoginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let challenge = state
        .authentication_challenges()
        .await
        .remove(&request.challenge_id);
    let Some(challenge) = challenge else {
        tracing::warn!(
            reason = "challenge_missing_or_expired",
            "passkey login rejected"
        );
        return Err(ApiError::bad_request(
            "The passkey login challenge expired.",
        ));
    };
    let (user_id, credential_id) = state
        .webauthn()?
        .identify_discoverable_authentication(&request.credential)
        .map_err(|error| {
            tracing::warn!(%error, "could not identify discoverable passkey");
            invalid_credentials()
        })?;
    let credential_id = URL_SAFE_NO_PAD.encode(credential_id);
    let transaction = state.database().begin().await?;
    let stored = passkey::Entity::find()
        .filter(passkey::Column::UserId.eq(user_id))
        .filter(passkey::Column::CredentialId.eq(credential_id))
        .one(&transaction)
        .await?;
    let Some(stored) = stored else {
        tracing::warn!(
            %user_id,
            reason = "credential_not_registered",
            "discoverable passkey login rejected"
        );
        return Err(invalid_credentials());
    };
    let passkey_id = stored.id;
    let mut credential: Passkey =
        serde_json::from_str(&stored.credential).map_err(ApiError::internal)?;
    let discoverable = DiscoverableKey::from(&credential);
    let result = state
        .webauthn()?
        .finish_discoverable_authentication(&request.credential, challenge.state, &[discoverable])
        .map_err(|error| {
            tracing::warn!(
                %error,
                %user_id,
                "discoverable passkey login verification failed"
            );
            invalid_credentials()
        })?;
    credential.update_credential(&result);
    let mut active: passkey::ActiveModel = stored.into();
    active.credential = Set(serde_json::to_string(&credential).map_err(ApiError::internal)?);
    active.last_used_at = Set(Some(Utc::now()));
    active.update(&transaction).await?;

    let account = user::Entity::find_by_id(user_id)
        .filter(user::Column::DisabledAt.is_null())
        .one(&transaction)
        .await?;
    let Some(account) = account else {
        tracing::warn!(
            %user_id,
            reason = "account_disabled_or_deleted",
            "discoverable passkey login rejected"
        );
        return Err(invalid_credentials());
    };
    let (_, cookie) = state.create_session_on(&transaction, account.id).await?;
    state
        .audit_on(&transaction, Some(account.id), "auth.login.passkey", None)
        .await?;
    transaction.commit().await?;
    tracing::info!(
        user_id = %account.id,
        %passkey_id,
        "passkey login completed"
    );
    Ok((
        jar.add(cookie),
        Json(AuthResponse {
            user: account.into(),
        }),
    ))
}

fn invalid_credentials() -> ApiError {
    ApiError {
        status: StatusCode::UNAUTHORIZED,
        code: "invalid_credentials",
        message: "The username or credential was not accepted.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectOptions, Database, DatabaseConnection, NotSet};
    use sea_orm_migration::MigratorTrait;

    use super::*;
    use crate::{actions::test_support, migration::Migrator};

    async fn database() -> DatabaseConnection {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let database = Database::connect(options).await.unwrap();
        database
            .execute_unprepared("PRAGMA foreign_keys = ON")
            .await
            .unwrap();
        Migrator::up(&database, None).await.unwrap();
        database
    }

    /// Every namespace-scoped table a personal namespace can own rows in.
    async fn namespace_resources(database: &DatabaseConnection, owner: &user::Model) {
        let now = Utc::now();
        let slug = owner.username.clone();
        let repository = test_support::repository(database, owner, "project").await;
        repository_alias::ActiveModel {
            namespace: Set(slug.clone()),
            name: Set("former-name".to_owned()),
            repository_id: Set(repository.id),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        namespace_pin::ActiveModel {
            namespace: Set(slug.clone()),
            repository_id: Set(repository.id),
            position: Set(0),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        namespace_integration::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set(slug.clone()),
            provider: Set("dokploy".to_owned()),
            name: Set("Dokploy".to_owned()),
            enabled: Set(true),
            url: Set("https://dokploy.example".to_owned()),
            api_key: Set("secret".to_owned()),
            dokploy_internal_url: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        namespace_mirror_identity::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set(slug.clone()),
            name: Set("Mirror token".to_owned()),
            kind: Set("token".to_owned()),
            username: Set(None),
            secret: Set("secret".to_owned()),
            provider: Set(Some("github".to_owned())),
            instance_url: Set(Some("https://api.github.com".to_owned())),
            public_key: Set(None),
            fingerprint: Set(None),
            last_used_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        action_runner::ActiveModel {
            id: NotSet,
            uuid: Set(Uuid::new_v4().to_string()),
            namespace: Set(Some(slug.clone())),
            name: Set("runner".to_owned()),
            token_hash: Set("runner-token".to_owned()),
            approved_labels: Set("[]".to_owned()),
            version: Set("test".to_owned()),
            ephemeral: Set(false),
            disabled_at: Set(None),
            deleted_at: Set(None),
            last_seen_at: Set(None),
            created_by: Set(Some(owner.id)),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        action_runner_registration_token::ActiveModel {
            id: Set(Uuid::new_v4()),
            token_hash: Set("registration-token".to_owned()),
            namespace: Set(Some(slug.clone())),
            runner_name: Set("runner".to_owned()),
            approved_labels: Set("[]".to_owned()),
            expires_at: Set(now),
            used_at: Set(None),
            created_by: Set(Some(owner.id)),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        action_secret::ActiveModel {
            id: NotSet,
            repository_id: Set(None),
            namespace: Set(Some(slug.clone())),
            name: Set("TOKEN".to_owned()),
            sealed: Set(vec![1, 2, 3]),
            updated_by: Set(Some(owner.id)),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        action_variable::ActiveModel {
            id: NotSet,
            repository_id: Set(None),
            namespace: Set(Some(slug)),
            name: Set("REGION".to_owned()),
            value: Set("eu".to_owned()),
            updated_by: Set(Some(owner.id)),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn user_rename_moves_every_namespace_resource() {
        let database = database().await;
        let owner = test_support::owner(&database, "old-name").await;
        namespace_resources(&database, &owner).await;

        let transaction = database.begin().await.unwrap();
        let renamed = rename_user_namespace(&transaction, owner, "new-name")
            .await
            .unwrap();
        // Deferred foreign keys are checked here: a missed table fails the commit.
        transaction.commit().await.unwrap();

        assert_eq!(renamed.username, "new-name");
        assert!(
            namespace::Entity::find_by_id("old-name")
                .one(&database)
                .await
                .unwrap()
                .is_none()
        );
        macro_rules! namespace_of {
            ($entity:ident) => {
                $entity::Entity::find()
                    .one(&database)
                    .await
                    .unwrap()
                    .map(|row| Option::<String>::from(row.namespace))
            };
        }
        let moved = Some(Some("new-name".to_owned()));
        assert_eq!(namespace_of!(repository), moved);
        assert_eq!(namespace_of!(repository_alias), moved);
        assert_eq!(namespace_of!(namespace_pin), moved);
        assert_eq!(namespace_of!(namespace_integration), moved);
        assert_eq!(namespace_of!(namespace_mirror_identity), moved);
        assert_eq!(namespace_of!(action_runner), moved);
        assert_eq!(namespace_of!(action_runner_registration_token), moved);
        assert_eq!(namespace_of!(action_secret), moved);
        assert_eq!(namespace_of!(action_variable), moved);
    }

    #[tokio::test]
    async fn expired_sessions_are_swept_and_live_ones_kept() {
        let database = database().await;
        let owner = test_support::owner(&database, "owner").await;
        let now = Utc::now();
        for (token, expires_at) in [
            ("expired", now - chrono::Duration::seconds(1)),
            ("long-expired", now - chrono::Duration::days(90)),
            ("live", now + chrono::Duration::hours(1)),
        ] {
            session::ActiveModel {
                token_hash: Set(token.to_owned()),
                user_id: Set(owner.id),
                expires_at: Set(expires_at),
                created_at: Set(now),
                last_seen_at: Set(now),
            }
            .insert(&database)
            .await
            .unwrap();
        }

        let removed = session::delete_expired(&database, now).await.unwrap();

        let remaining = session::Entity::find().all(&database).await.unwrap();
        assert_eq!(removed, 2);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].token_hash, "live");
    }

    #[test]
    fn credential_changes_refuse_api_tokens() {
        assert!(require_browser_session(false).is_ok());
        assert!(require_browser_session(true).is_err());
    }

    #[tokio::test]
    async fn user_rename_refuses_a_slug_another_namespace_owns() {
        let database = database().await;
        let owner = test_support::owner(&database, "old-name").await;
        test_support::owner(&database, "taken").await;

        let transaction = database.begin().await.unwrap();
        assert!(
            rename_user_namespace(&transaction, owner, "taken")
                .await
                .is_err()
        );
    }

    mod handlers {
        use axum::http::{Method, StatusCode, header};
        use sea_orm::{ActiveModelTrait, IntoActiveModel, Set};
        use serde_json::{Value, json};

        use super::super::*;
        use crate::{
            entity::api_token,
            identity::{
                SCOPE_READ, SCOPE_WRITE, SESSION_COOKIE,
                test_support::{Harness, PASSWORD},
            },
        };

        async fn login(harness: &Harness, username: &str, password: &str) -> (StatusCode, Value) {
            harness
                .request(
                    Method::POST,
                    "/api/v1/auth/login",
                    None,
                    Some(json!({ "username": username, "password": password })),
                )
                .await
        }

        #[tokio::test]
        async fn login_accepts_the_password_and_refuses_everything_else() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;

            let request = axum::http::Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    json!({ "username": "Alice", "password": PASSWORD }).to_string(),
                ))
                .unwrap();
            let response = harness.send(request).await;
            assert_eq!(response.status(), StatusCode::OK);
            let cookie = response
                .headers()
                .get(header::SET_COOKIE)
                .expect("a session cookie is set")
                .to_str()
                .unwrap()
                .to_owned();
            assert!(cookie.starts_with(&format!("{SESSION_COOKIE}=")));
            assert!(cookie.contains("HttpOnly"));
            let body = crate::identity::test_support::json_body(response).await;
            assert_eq!(body["user"]["username"], "alice");

            // The cookie signs the user in.
            let session = cookie.split(';').next().unwrap().to_owned();
            let (status, body) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/commit-identity",
                    Some(&session),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");

            let (status, body) = login(&harness, "alice", "not the password").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body["error"]["code"], "invalid_credentials");
            let (status, body) = login(&harness, "nobody", PASSWORD).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "unknown users look the same"
            );
            assert_eq!(body["error"]["code"], "invalid_credentials");
            let (status, _) = login(&harness, "not a slug!", PASSWORD).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);

            let mut disabled = alice.into_active_model();
            disabled.disabled_at = Set(Some(Utc::now()));
            disabled.update(&harness.database).await.unwrap();
            let (status, _) = login(&harness, "alice", PASSWORD).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "disabled accounts cannot sign in"
            );
        }

        #[tokio::test]
        async fn repeated_failures_lock_the_account_even_for_the_right_password() {
            let harness = Harness::new().await;
            harness.user("alice").await;
            harness.user("bob").await;
            for _ in 0..8 {
                let (status, _) = login(&harness, "alice", "wrong password").await;
                assert_eq!(status, StatusCode::UNAUTHORIZED);
            }
            let (status, body) = login(&harness, "alice", PASSWORD).await;
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
            assert_eq!(body["error"]["code"], "rate_limited");
            // The lock is per account, not per instance.
            let (status, _) = login(&harness, "bob", PASSWORD).await;
            assert_eq!(status, StatusCode::OK);
        }

        #[tokio::test]
        async fn changing_the_password_keeps_this_session_and_ends_the_others() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            let current = harness.login(&alice).await;
            let other = harness.login(&alice).await;
            let new_password = "a different long password";

            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/password",
                    Some(&current),
                    Some(json!({ "current_password": "wrong", "new_password": new_password })),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/password",
                    Some(&current),
                    Some(json!({ "current_password": PASSWORD, "new_password": "short" })),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/password",
                    Some(&current),
                    Some(json!({ "current_password": PASSWORD, "new_password": new_password })),
                )
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

            let (status, _) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/commit-identity",
                    Some(&current),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "the session that changed it stays");
            let (status, _) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/commit-identity",
                    Some(&other),
                    None,
                )
                .await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "other sessions are signed out"
            );
            let (status, _) = login(&harness, "alice", PASSWORD).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            let (status, _) = login(&harness, "alice", new_password).await;
            assert_eq!(status, StatusCode::OK);

            // API tokens cannot change credentials.
            let raw = "gat_test_token";
            api_token::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(alice.id),
                name: Set("script".to_owned()),
                token_hash: Set(hash_secret(raw)),
                scopes: Set(SCOPE_READ | SCOPE_WRITE),
                expires_at: Set(None),
                created_at: Set(Utc::now()),
                last_used_at: Set(None),
                revoked_at: Set(None),
            }
            .insert(&harness.database)
            .await
            .unwrap();
            let request = axum::http::Request::builder()
                .method(Method::PUT)
                .uri("/api/v1/me/password")
                .header(header::AUTHORIZATION, format!("Bearer {raw}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    json!({ "current_password": new_password, "new_password": PASSWORD })
                        .to_string(),
                ))
                .unwrap();
            assert_eq!(harness.send(request).await.status(), StatusCode::FORBIDDEN);
        }

        #[tokio::test]
        async fn renaming_moves_the_personal_namespace_and_refuses_taken_names() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            harness.user("taken").await;
            let cookie = harness.login(&alice).await;

            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/username",
                    Some(&cookie),
                    Some(json!({ "username": "Alice-Two" })),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["user"]["username"], "alice-two");
            let (status, body) = harness
                .request(Method::GET, "/api/v1/namespaces/alice-two", None, None)
                .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["kind"], "user");
            let (status, _) = harness
                .request(Method::GET, "/api/v1/namespaces/alice", None, None)
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            // The session survives the rename.
            let (status, _) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/commit-identity",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK);

            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/username",
                    Some(&cookie),
                    Some(json!({ "username": "taken" })),
                )
                .await;
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            let (status, _) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/username",
                    Some(&cookie),
                    Some(json!({ "username": "-bad-" })),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let (status, body) = login(&harness, "alice-two", PASSWORD).await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
    }
}
