//! Verified address lookup and one-time email credential persistence.
use crate::{IdentityError, hash_secret, random_secret};
use chrono::{Duration, Utc};
use gitadel_db::entity::{email_token, user_email};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, sea_query::Expr,
};
use uuid::Uuid;

pub async fn verified_email<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<Option<String>, IdentityError> {
    Ok(user_email::Entity::find_by_id(user_id)
        .filter(user_email::Column::VerifiedAt.is_not_null())
        .one(connection)
        .await?
        .map(|row| row.email))
}

pub async fn issue_token<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
    purpose: &str,
    email: Option<&str>,
    lifetime: Duration,
) -> Result<String, IdentityError> {
    let now = Utc::now();
    email_token::Entity::delete_many()
        .filter(email_token::Column::UserId.eq(user_id))
        .filter(email_token::Column::Purpose.eq(purpose))
        .filter(email_token::Column::ExpiresAt.lte(now))
        .exec(connection)
        .await?;
    let token = random_secret(32);
    email_token::ActiveModel {
        token_hash: Set(hash_secret(&token)),
        user_id: Set(user_id),
        purpose: Set(purpose.to_owned()),
        email: Set(email.map(str::to_owned)),
        expires_at: Set(now + lifetime),
        used_at: Set(None),
        created_at: Set(now),
    }
    .insert(connection)
    .await?;
    Ok(token)
}

/// Atomically mark an unexpired, unused token as used and return it.
pub async fn consume_token<C: ConnectionTrait>(
    connection: &C,
    token_hash: &str,
    purpose: &str,
) -> Result<Option<email_token::Model>, IdentityError> {
    let now = Utc::now();
    let consumed = email_token::Entity::update_many()
        .col_expr(email_token::Column::UsedAt, Expr::value(Some(now)))
        .filter(email_token::Column::TokenHash.eq(token_hash))
        .filter(email_token::Column::Purpose.eq(purpose))
        .filter(email_token::Column::UsedAt.is_null())
        .filter(email_token::Column::ExpiresAt.gt(now))
        .exec(connection)
        .await?;
    if consumed.rows_affected != 1 {
        return Ok(None);
    }
    Ok(email_token::Entity::find_by_id(token_hash)
        .one(connection)
        .await?)
}
