//! Account registration, credential changes and authentication workflows.
use crate::{
    IdentityError, Principal, hash_password, hash_secret, random_secret,
    runtime::{AuthService, RegistrationChallenge},
    validate_name, validate_slug, verify_password,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use gitadel_db::entity::{invitation, namespace, passkey, user};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait, sea_query::Expr,
};
use std::time::Instant;
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, DiscoverableKey, Passkey, PublicKeyCredential,
    RegisterPublicKeyCredential, RequestChallengeResponse,
};

pub struct SessionGrant {
    pub user: user::Model,
    pub token: String,
}
pub enum LoginOutcome {
    Authenticated(SessionGrant),
    TwoFactor { challenge_id: String },
}
pub struct CredentialsRequest {
    pub username: String,
    pub password: String,
}
pub struct TwoFactorLoginRequest {
    pub challenge_id: String,
    pub code: String,
}
pub struct UpdateUsernameRequest {
    pub username: String,
}
pub struct UpdateRepositoryPreferencesRequest {
    pub default_repository_visibility: String,
}
pub struct UpdateThemePreferenceRequest {
    pub theme_preference: String,
}
pub struct UpdateMotionPreferenceRequest {
    pub motion_preference: String,
}
pub struct UpdatePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}
pub struct SetupRequest {
    pub token: String,
    pub username: String,
    pub password: String,
}
pub struct InvitationRegistrationRequest {
    pub token: String,
    pub username: String,
    pub password: String,
}
pub struct CreateInvitationRequest {
    pub expires_in_hours: Option<i64>,
}
pub struct InvitationResponse {
    pub token: String,
    pub expires_at: chrono::DateTime<Utc>,
}
pub struct PasskeyResponse {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<Utc>,
    pub last_used_at: Option<chrono::DateTime<Utc>>,
}
pub struct StartPasskeyRegistrationRequest {
    pub name: String,
}
pub struct PasskeyCreationResponse {
    pub challenge_id: String,
    /// WebAuthn `PublicKeyCredentialCreationOptions`, wrapped in `publicKey`.
    pub options: CreationChallengeResponse,
}
pub struct FinishPasskeyRegistrationRequest {
    pub challenge_id: String,
    /// The WebAuthn `PublicKeyCredential` the browser produced.
    pub credential: RegisterPublicKeyCredential,
}
pub struct PasskeyRequestResponse {
    pub challenge_id: String,
    /// WebAuthn request options from webauthn-rs; external browser protocol.
    pub options: RequestChallengeResponse,
}
pub struct FinishPasskeyLoginRequest {
    pub challenge_id: String,
    /// WebAuthn credential from the browser; external browser protocol.
    pub credential: PublicKeyCredential,
}
pub async fn login(
    state: &AuthService<'_>,
    request: CredentialsRequest,
) -> Result<LoginOutcome, IdentityError> {
    if !crate::sso::public_configuration(state.database())
        .await?
        .password_enabled
    {
        return Err(IdentityError::forbidden("Password login is disabled."));
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
    if crate::two_factor::is_enabled(state.database(), account.id).await? {
        let challenge_id = state.insert_pending_login(account.id).await?;
        return Ok(LoginOutcome::TwoFactor { challenge_id });
    }

    let transaction = state.database().begin().await?;
    let token = state.create_session_on(&transaction, account.id).await?;
    state
        .audit_on(&transaction, Some(account.id), "auth.login.password", None)
        .await?;
    transaction.commit().await?;
    Ok(LoginOutcome::Authenticated(SessionGrant {
        user: account,
        token,
    }))
}
pub async fn finish_two_factor_login(
    state: &AuthService<'_>,
    request: TwoFactorLoginRequest,
) -> Result<SessionGrant, IdentityError> {
    state
        .enforce_auth_rate_limit("two-factor-source", 20)
        .await?;
    let Some(user_id) = state.pending_login_user(&request.challenge_id).await else {
        return Err(IdentityError::bad_request(
            "The sign-in attempt expired. Sign in again.",
        ));
    };
    state
        .enforce_account_auth_rate_limit(&format!("two-factor:{user_id}"), 8)
        .await?;

    let transaction = state.database().begin().await?;
    let Some(factor) = crate::two_factor::verify_second_factor(
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
        return Err(crate::two_factor::invalid_code());
    };
    let account = user::Entity::find_by_id(user_id)
        .filter(user::Column::DisabledAt.is_null())
        .one(&transaction)
        .await?
        .ok_or_else(invalid_credentials)?;
    let token = state.create_session_on(&transaction, account.id).await?;
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
    Ok(SessionGrant {
        user: account,
        token,
    })
}
pub async fn setup(
    state: &AuthService<'_>,
    request: SetupRequest,
) -> Result<SessionGrant, IdentityError> {
    if !state.setup_token_matches(&request.token).await {
        return Err(IdentityError::bad_request(
            "The setup link is invalid or was already used. Restart Gitadel to print a new one.",
        ));
    }
    // bootstrap_admin refuses once any account exists, so concurrent
    // submissions cannot create a second administrator.
    let account =
        crate::bootstrap_admin(state.database(), &request.username, request.password).await?;
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
    let token = state.create_session_on(&transaction, account.id).await?;
    transaction.commit().await?;
    Ok(SessionGrant {
        user: account,
        token,
    })
}
pub async fn register(
    state: &AuthService<'_>,
    request: InvitationRegistrationRequest,
) -> Result<SessionGrant, IdentityError> {
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
        return Err(IdentityError::bad_request(
            "The invitation is invalid or has expired.",
        ));
    }
    if namespace::Entity::find_by_id(&username)
        .one(&transaction)
        .await?
        .is_some()
    {
        return Err(IdentityError::conflict("That username is already in use."));
    }
    let password_hash = hash_password(request.password).await?;

    let account =
        crate::accounts::insert_account_on(&transaction, &username, password_hash, false, now)
            .await?;
    state
        .audit_on(
            &transaction,
            Some(account.id),
            "account.register",
            Some(username),
        )
        .await?;
    let token = state.create_session_on(&transaction, account.id).await?;
    transaction.commit().await?;
    Ok(SessionGrant {
        user: account,
        token,
    })
}
pub async fn update_username(
    state: &AuthService<'_>,
    actor: &Principal,
    request: UpdateUsernameRequest,
) -> Result<user::Model, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;

    let username = validate_slug(&request.username, "Username")?;
    if username == actor.user.username {
        return Ok(actor.user);
    }

    let account =
        crate::accounts::rename_account(&state.account_service(), actor.user, username).await?;
    Ok(account)
}
pub async fn update_repository_preferences(
    state: &AuthService<'_>,
    actor: &Principal,
    request: UpdateRepositoryPreferencesRequest,
) -> Result<user::Model, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;
    if !matches!(
        request.default_repository_visibility.as_str(),
        "public" | "private"
    ) {
        return Err(IdentityError::bad_request(
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
    Ok(account)
}
pub async fn update_theme_preference(
    state: &AuthService<'_>,
    actor: &Principal,
    request: UpdateThemePreferenceRequest,
) -> Result<user::Model, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;
    if !matches!(
        request.theme_preference.as_str(),
        "system" | "light" | "dark"
    ) {
        return Err(IdentityError::bad_request(
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
    Ok(account)
}
pub async fn update_motion_preference(
    state: &AuthService<'_>,
    actor: &Principal,
    request: UpdateMotionPreferenceRequest,
) -> Result<user::Model, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;
    if !matches!(request.motion_preference.as_str(), "system" | "reduce") {
        return Err(IdentityError::bad_request(
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
    Ok(account)
}
pub async fn update_password(
    state: &AuthService<'_>,
    actor: &Principal,
    request: UpdatePasswordRequest,
    current_session_hash: Option<String>,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;
    verify_current_password(&actor.user, request.current_password).await?;
    if verify_password(
        request.new_password.clone(),
        actor.user.password_hash.clone(),
    )
    .await?
    {
        return Err(IdentityError::bad_request(
            "Choose a password that differs from your current password.",
        ));
    }

    let password_hash = hash_password(request.new_password).await?;
    let current_session_hash = current_session_hash.ok_or_else(IdentityError::unauthorized)?;
    let transaction = state.database().begin().await?;
    crate::replace_password_on(
        &transaction,
        &actor.user,
        password_hash,
        &current_session_hash,
    )
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
    Ok(())
}
pub async fn create_invitation(
    state: &AuthService<'_>,
    actor: &Principal,
    request: CreateInvitationRequest,
) -> Result<InvitationResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    if !actor.user.is_admin {
        return Err(IdentityError::forbidden(
            "Only an administrator can create invitations.",
        ));
    }
    let lifetime = request
        .expires_in_hours
        .map(chrono::Duration::hours)
        .unwrap_or_else(|| state.invitation_lifetime());
    if lifetime <= chrono::Duration::zero() || lifetime > chrono::Duration::days(30) {
        return Err(IdentityError::bad_request(
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
    Ok(InvitationResponse { token, expires_at })
}
pub async fn list_passkeys(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<Vec<PasskeyResponse>, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    let rows = passkey::Entity::find()
        .filter(passkey::Column::UserId.eq(actor.user.id))
        .order_by_asc(passkey::Column::CreatedAt)
        .all(state.database())
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| PasskeyResponse {
            id: row.id,
            name: row.name,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
        })
        .collect())
}
pub async fn start_passkey_registration(
    state: &AuthService<'_>,
    actor: &Principal,
    request: StartPasskeyRegistrationRequest,
) -> Result<PasskeyCreationResponse, IdentityError> {
    check_passkey_registration(state).await?;
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    if actor.via_api_token {
        tracing::warn!(
            user_id = %actor.user.id,
            reason = "api_token_session",
            "passkey registration rejected"
        );
        return Err(IdentityError::forbidden(
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
            serde_json::from_str(&row.credential).map_err(IdentityError::internal)?;
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
            IdentityError::internal(error)
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
    Ok(PasskeyCreationResponse {
        challenge_id,
        options,
    })
}
pub async fn finish_passkey_registration(
    state: &AuthService<'_>,
    actor: &Principal,
    request: FinishPasskeyRegistrationRequest,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    require_browser_session(actor.via_api_token)?;
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
        return Err(IdentityError::bad_request(
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
            IdentityError::bad_request("The passkey registration could not be verified.")
        })?;
    let transaction = state.database().begin().await?;
    let row = passkey::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(actor.user.id),
        name: Set(challenge.name),
        credential_id: Set(URL_SAFE_NO_PAD.encode(credential.cred_id())),
        credential: Set(serde_json::to_string(&credential).map_err(IdentityError::internal)?),
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
    Ok(())
}
pub async fn delete_passkey(
    state: &AuthService<'_>,
    actor: &Principal,
    id: Uuid,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
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
        return Err(IdentityError::not_found());
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
    Ok(())
}
pub async fn start_passkey_login(
    state: &AuthService<'_>,
) -> Result<PasskeyRequestResponse, IdentityError> {
    if !crate::sso::public_configuration(state.database())
        .await?
        .passkey_enabled
    {
        return Err(IdentityError::forbidden("Passkey login is disabled."));
    }
    state.enforce_auth_rate_limit("passkey-start", 30).await?;
    let (options, authentication) = state
        .webauthn()?
        .start_discoverable_authentication()
        .map_err(|error| {
            tracing::warn!(%error, "could not create discoverable passkey login challenge");
            IdentityError::internal(error)
        })?;
    let challenge_id = random_secret(24);
    state
        .insert_authentication_challenge(challenge_id.clone(), authentication)
        .await?;
    tracing::info!("discoverable passkey login challenge issued");
    Ok(PasskeyRequestResponse {
        challenge_id,
        options,
    })
}
pub async fn finish_passkey_login(
    state: &AuthService<'_>,
    request: FinishPasskeyLoginRequest,
) -> Result<SessionGrant, IdentityError> {
    let challenge = state
        .authentication_challenges()
        .await
        .remove(&request.challenge_id);
    let Some(challenge) = challenge else {
        tracing::warn!(
            reason = "challenge_missing_or_expired",
            "passkey login rejected"
        );
        return Err(IdentityError::bad_request(
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
        serde_json::from_str(&stored.credential).map_err(IdentityError::internal)?;
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
    active.credential = Set(serde_json::to_string(&credential).map_err(IdentityError::internal)?);
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
    let token = state.create_session_on(&transaction, account.id).await?;
    state
        .audit_on(&transaction, Some(account.id), "auth.login.passkey", None)
        .await?;
    transaction.commit().await?;
    tracing::info!(
        user_id = %account.id,
        %passkey_id,
        "passkey login completed"
    );
    Ok(SessionGrant {
        user: account,
        token,
    })
}
pub fn require_browser_session(via_api_token: bool) -> Result<(), IdentityError> {
    if via_api_token {
        return Err(IdentityError::forbidden(
            "Update account credentials from a browser session.",
        ));
    }
    Ok(())
}
fn invalid_credentials() -> IdentityError {
    IdentityError::InvalidCredentials
}
async fn verify_current_password(
    account: &user::Model,
    current_password: String,
) -> Result<(), IdentityError> {
    crate::verify_current_password(account, current_password).await
}

pub struct AuthStatus {
    pub setup_required: bool,
    pub account: Option<user::Model>,
    pub authentication: crate::sso::AuthenticationConfiguration,
    pub email_enabled: bool,
}
pub async fn status(
    state: &AuthService<'_>,
    raw_session: Option<&str>,
) -> Result<AuthStatus, IdentityError> {
    let setup_required = user::Entity::find().count(state.database()).await? == 0;
    let account = state.session_user(raw_session).await?;
    let mut authentication = crate::sso::public_configuration(state.database()).await?;
    authentication.passkey_enabled &= state.passkeys_available();
    Ok(AuthStatus {
        setup_required,
        account,
        authentication,
        email_enabled: state.mailer().is_enabled(),
    })
}

pub async fn check_passkey_registration(state: &AuthService<'_>) -> Result<(), IdentityError> {
    if !crate::sso::public_configuration(state.database())
        .await?
        .passkey_enabled
    {
        return Err(IdentityError::forbidden("Passkeys are disabled."));
    }
    Ok(())
}
