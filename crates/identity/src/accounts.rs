//! Account insertion and namespace changes shared by every sign-up source.
use crate::{AccountService, IdentityError};
use chrono::Utc;
use gitadel_db::entity::{
    action_runner, action_runner_registration_token, action_secret, action_variable, namespace,
    namespace_integration, namespace_mirror_identity, namespace_pin, repository, repository_alias,
    repository_import, repository_import_item, user,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, QueryFilter,
    Set, TransactionTrait, sea_query::Expr,
};
use uuid::Uuid;
pub async fn move_namespace_resources<C: ConnectionTrait>(
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
pub async fn rename_account(
    state: &AccountService<'_>,
    account: user::Model,
    username: String,
) -> Result<user::Model, IdentityError> {
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
pub async fn rename_user_namespace(
    transaction: &DatabaseTransaction,
    account: user::Model,
    username: &str,
) -> Result<user::Model, IdentityError> {
    if namespace::Entity::find_by_id(username)
        .one(transaction)
        .await?
        .is_some()
    {
        return Err(IdentityError::conflict("That username is already in use."));
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
        return Err(IdentityError::internal("the user namespace is missing"));
    }

    let mut active: user::ActiveModel = account.into();
    active.username = Set(username.to_owned());
    active.updated_at = Set(Utc::now());
    Ok(active.update(transaction).await?)
}

/// Insert an account with its personal namespace on the caller's transaction.
pub async fn insert_account_on<C: ConnectionTrait>(
    connection: &C,
    username: &str,
    password_hash: String,
    is_admin: bool,
    now: chrono::DateTime<Utc>,
) -> Result<user::Model, IdentityError> {
    let account = user::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set(username.to_owned()),
        password_hash: Set(password_hash),
        is_admin: Set(is_admin),
        default_repository_visibility: Set("private".to_owned()),
        theme_preference: Set("system".to_owned()),
        motion_preference: Set("system".to_owned()),
        disabled_at: Set(None),
        avatar_updated_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(connection)
    .await?;
    namespace::ActiveModel {
        slug: Set(username.to_owned()),
        kind: Set("user".to_owned()),
        user_id: Set(Some(account.id)),
        organization_id: Set(None),
        created_at: Set(now),
    }
    .insert(connection)
    .await?;
    Ok(account)
}
