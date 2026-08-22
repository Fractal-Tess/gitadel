use std::time::Instant;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, Passkey, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

use super::{
    ApiError, AuthenticationChallenge, IdentityState, RegistrationChallenge, UserResponse,
    bootstrap_admin, hash_password, hash_secret, random_secret, validate_name, validate_slug,
    verify_password,
};
use crate::entity::{invitation, namespace, passkey, repository, session, user};

#[derive(Serialize)]
pub struct AuthStatusResponse {
    setup_required: bool,
    authenticated: bool,
    user: Option<UserResponse>,
}

pub async fn status(
    State(state): State<IdentityState>,
    jar: CookieJar,
) -> Result<Json<AuthStatusResponse>, ApiError> {
    let setup_required = user::Entity::find().count(state.database()).await? == 0;
    let account = state.session_user(&jar).await?;
    Ok(Json(AuthStatusResponse {
        setup_required,
        authenticated: account.is_some(),
        user: account.map(UserResponse::from),
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

pub async fn setup(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<CredentialsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let account = bootstrap_admin(state.database(), &request.username, request.password).await?;
    let (_, cookie) = state.create_session(account.id).await?;
    Ok((
        StatusCode::CREATED,
        jar.add(cookie),
        Json(AuthResponse {
            user: account.into(),
        }),
    ))
}

pub async fn login(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<CredentialsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let username = validate_slug(&request.username, "Username")?;
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(username))
        .filter(user::Column::DisabledAt.is_null())
        .one(state.database())
        .await?;
    let Some(account) = account else {
        return Err(invalid_credentials());
    };
    if !verify_password(request.password, account.password_hash.clone()).await? {
        return Err(invalid_credentials());
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
    current_password: String,
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
    verify_current_password(&actor.user, request.current_password).await?;

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

fn require_browser_session(via_api_token: bool) -> Result<(), ApiError> {
    if via_api_token {
        return Err(ApiError::forbidden(
            "Update account credentials from a browser session.",
        ));
    }
    Ok(())
}

async fn rename_account(
    state: &IdentityState,
    account: user::Model,
    username: String,
) -> Result<user::Model, ApiError> {
    let transaction = state.database().begin().await?;
    if namespace::Entity::find_by_id(&username)
        .one(&transaction)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("That username is already in use."));
    }

    transaction
        .execute_unprepared("PRAGMA defer_foreign_keys = ON")
        .await?;
    repository::Entity::update_many()
        .col_expr(repository::Column::Namespace, Expr::value(username.clone()))
        .filter(repository::Column::Namespace.eq(&account.username))
        .exec(&transaction)
        .await?;
    let namespace_update = namespace::Entity::update_many()
        .col_expr(namespace::Column::Slug, Expr::value(username.clone()))
        .filter(namespace::Column::UserId.eq(account.id))
        .exec(&transaction)
        .await?;
    if namespace_update.rows_affected != 1 {
        return Err(ApiError::internal("the user namespace is missing"));
    }

    let previous_username = account.username.clone();
    let mut active: user::ActiveModel = account.into();
    active.username = Set(username.clone());
    active.updated_at = Set(Utc::now());
    let account = active.update(&transaction).await?;
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
    let password_hash = hash_password(request.password).await?;
    let transaction = state.database().begin().await?;
    let invitation = invitation::Entity::find_by_id(hash_secret(&request.token))
        .one(&transaction)
        .await?
        .filter(|invitation| invitation.used_at.is_none() && invitation.expires_at > Utc::now())
        .ok_or_else(|| ApiError::bad_request("The invitation is invalid or has expired."))?;
    if namespace::Entity::find_by_id(&username)
        .one(&transaction)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("That username is already in use."));
    }

    let now = Utc::now();
    let account = user::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set(username.clone()),
        password_hash: Set(password_hash),
        is_admin: Set(false),
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
    let mut used: invitation::ActiveModel = invitation.into();
    used.used_at = Set(Some(now));
    used.update(&transaction).await?;
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
    let actor = state
        .authenticate(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    if actor.via_api_token {
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
    let (options, registration) = state
        .webauthn()
        .start_passkey_registration(
            actor.user.id,
            &actor.user.username,
            &actor.user.username,
            Some(excluded),
        )
        .map_err(ApiError::internal)?;
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
        .filter(|challenge| challenge.user_id == actor.user.id)
        .ok_or_else(|| ApiError::bad_request("The passkey registration challenge expired."))?;
    let credential = state
        .webauthn()
        .finish_passkey_registration(&request.credential, &challenge.state)
        .map_err(|_| ApiError::bad_request("The passkey registration could not be verified."))?;
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

#[derive(Deserialize)]
pub struct StartPasskeyLoginRequest {
    username: String,
}

#[derive(Serialize)]
pub struct PasskeyRequestResponse {
    challenge_id: String,
    options: RequestChallengeResponse,
}

pub async fn start_passkey_login(
    State(state): State<IdentityState>,
    Json(request): Json<StartPasskeyLoginRequest>,
) -> Result<Json<PasskeyRequestResponse>, ApiError> {
    let username = validate_slug(&request.username, "Username")?;
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(username))
        .filter(user::Column::DisabledAt.is_null())
        .one(state.database())
        .await?
        .ok_or_else(invalid_credentials)?;
    let rows = passkey::Entity::find()
        .filter(passkey::Column::UserId.eq(account.id))
        .all(state.database())
        .await?;
    if rows.is_empty() {
        return Err(invalid_credentials());
    }
    let credentials = rows
        .into_iter()
        .map(|row| serde_json::from_str(&row.credential).map_err(ApiError::internal))
        .collect::<Result<Vec<Passkey>, ApiError>>()?;
    let (options, authentication) = state
        .webauthn()
        .start_passkey_authentication(&credentials)
        .map_err(ApiError::internal)?;
    let challenge_id = random_secret(24);
    state.authentication_challenges().await.insert(
        challenge_id.clone(),
        AuthenticationChallenge {
            user_id: account.id,
            state: authentication,
            created_at: Instant::now(),
        },
    );
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
        .remove(&request.challenge_id)
        .ok_or_else(|| ApiError::bad_request("The passkey login challenge expired."))?;
    let result = state
        .webauthn()
        .finish_passkey_authentication(&request.credential, &challenge.state)
        .map_err(|_| invalid_credentials())?;
    let credential_id = URL_SAFE_NO_PAD.encode(result.cred_id());
    let transaction = state.database().begin().await?;
    let stored = passkey::Entity::find()
        .filter(passkey::Column::UserId.eq(challenge.user_id))
        .filter(passkey::Column::CredentialId.eq(credential_id))
        .one(&transaction)
        .await?
        .ok_or_else(invalid_credentials)?;
    let mut credential: Passkey =
        serde_json::from_str(&stored.credential).map_err(ApiError::internal)?;
    credential.update_credential(&result);
    let mut active: passkey::ActiveModel = stored.into();
    active.credential = Set(serde_json::to_string(&credential).map_err(ApiError::internal)?);
    active.last_used_at = Set(Some(Utc::now()));
    active.update(&transaction).await?;

    let account = user::Entity::find_by_id(challenge.user_id)
        .filter(user::Column::DisabledAt.is_null())
        .one(&transaction)
        .await?
        .ok_or_else(invalid_credentials)?;
    let (_, cookie) = state.create_session_on(&transaction, account.id).await?;
    state
        .audit_on(&transaction, Some(account.id), "auth.login.passkey", None)
        .await?;
    transaction.commit().await?;
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
