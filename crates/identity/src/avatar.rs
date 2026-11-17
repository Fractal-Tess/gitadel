//! Account and organization avatar policy and persistence.
use crate::{AccountService, IdentityError, Principal};
use chrono::Utc;
use gitadel_db::entity::{organization, organization_avatar, user, user_avatar};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set, TransactionTrait};
use uuid::Uuid;
const MAX_AVATAR_BYTES: usize = 4 * 1024 * 1024;
const AVATAR_EDGE: u32 = 512;
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
pub async fn public_avatar(
    state: &AccountService<'_>,
    user_id: Uuid,
) -> Result<Vec<u8>, IdentityError> {
    let avatar = user_avatar::Entity::find_by_id(user_id)
        .one(state.database())
        .await?
        .ok_or_else(IdentityError::not_found)?;

    Ok(avatar.content)
}
pub async fn public_organization_avatar(
    state: &AccountService<'_>,
    slug: String,
) -> Result<Vec<u8>, IdentityError> {
    let organization = organization::Entity::find()
        .filter(organization::Column::Slug.eq(slug))
        .one(state.database())
        .await?
        .ok_or_else(IdentityError::not_found)?;
    let avatar = organization_avatar::Entity::find_by_id(organization.id)
        .one(state.database())
        .await?
        .ok_or_else(IdentityError::not_found)?;
    Ok(avatar.content)
}
pub async fn update_avatar(
    state: &AccountService<'_>,
    actor: &Principal,
    image_base64: String,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let actor_id = actor.user.id;
    let body = decode_avatar(&image_base64)?;

    let transaction = state.database().begin().await?;
    match user_avatar::Entity::find_by_id(actor.user.id)
        .one(&transaction)
        .await?
    {
        Some(avatar) => {
            let mut active: user_avatar::ActiveModel = avatar.into();
            active.content = Set(body);
            active.update(&transaction).await?;
        }
        None => {
            user_avatar::ActiveModel {
                user_id: Set(actor.user.id),
                content: Set(body),
            }
            .insert(&transaction)
            .await?;
        }
    }

    let now = Utc::now();
    let mut account: user::ActiveModel = actor.user.into();
    account.avatar_updated_at = Set(Some(now));
    account.updated_at = Set(now);
    account.update(&transaction).await?;
    state
        .audit_on(&transaction, Some(actor_id), "account.avatar.update", None)
        .await?;
    transaction.commit().await?;
    Ok(())
}
pub async fn update_organization_avatar(
    state: &AccountService<'_>,
    actor: &Principal,
    slug: String,
    image_base64: String,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let organization =
        crate::permissions::owned_organization(state.database(), &slug, actor.user.id).await?;
    let body = decode_avatar(&image_base64)?;
    let transaction = state.database().begin().await?;
    match organization_avatar::Entity::find_by_id(organization.id)
        .one(&transaction)
        .await?
    {
        Some(avatar) => {
            let mut active: organization_avatar::ActiveModel = avatar.into();
            active.content = Set(body);
            active.update(&transaction).await?;
        }
        None => {
            organization_avatar::ActiveModel {
                organization_id: Set(organization.id),
                content: Set(body),
            }
            .insert(&transaction)
            .await?;
        }
    }
    let now = Utc::now();
    let mut active: organization::ActiveModel = organization.into();
    active.avatar_updated_at = Set(Some(now));
    active.updated_at = Set(now);
    active.update(&transaction).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "organization.avatar.update",
            Some(slug),
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}
pub async fn delete_avatar(
    state: &AccountService<'_>,
    actor: &Principal,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let actor_id = actor.user.id;
    let transaction = state.database().begin().await?;
    user_avatar::Entity::delete_by_id(actor.user.id)
        .exec(&transaction)
        .await?;

    let now = Utc::now();
    let mut account: user::ActiveModel = actor.user.into();
    account.avatar_updated_at = Set(None);
    account.updated_at = Set(now);
    account.update(&transaction).await?;
    state
        .audit_on(&transaction, Some(actor_id), "account.avatar.delete", None)
        .await?;
    transaction.commit().await?;
    Ok(())
}
pub async fn delete_organization_avatar(
    state: &AccountService<'_>,
    actor: &Principal,
    slug: String,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let organization =
        crate::permissions::owned_organization(state.database(), &slug, actor.user.id).await?;
    let transaction = state.database().begin().await?;
    organization_avatar::Entity::delete_by_id(organization.id)
        .exec(&transaction)
        .await?;
    let now = Utc::now();
    let mut active: organization::ActiveModel = organization.into();
    active.avatar_updated_at = Set(None);
    active.updated_at = Set(now);
    active.update(&transaction).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "organization.avatar.delete",
            Some(slug),
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}
pub fn validate_avatar_png(bytes: &[u8]) -> Result<(), IdentityError> {
    if bytes.len() > MAX_AVATAR_BYTES {
        return Err(IdentityError::bad_request(
            "Profile pictures cannot exceed 4 MiB.",
        ));
    }
    if bytes.len() < 24
        || !bytes.starts_with(&PNG_SIGNATURE)
        || bytes.get(12..16) != Some(b"IHDR".as_slice())
    {
        return Err(invalid_png());
    }

    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width != AVATAR_EDGE || height != AVATAR_EDGE {
        return Err(IdentityError::bad_request(
            "Profile pictures must be 512 by 512 pixels.",
        ));
    }

    if !crate::images::valid_png(bytes) {
        return Err(invalid_png());
    }
    Ok(())
}
pub fn invalid_png() -> IdentityError {
    IdentityError::bad_request("The uploaded profile picture is not a valid PNG.")
}

fn decode_avatar(encoded: &str) -> Result<Vec<u8>, IdentityError> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let body = STANDARD.decode(encoded).map_err(|_| invalid_png())?;
    validate_avatar_png(&body)?;
    Ok(body)
}
