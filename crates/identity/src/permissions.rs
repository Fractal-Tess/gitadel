//! Namespace and organization management authorization.
use crate::{AuthenticatedUser, IdentityError};
use gitadel_db::entity::{namespace, organization, organization_member};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;
pub async fn authorize_namespace<C: ConnectionTrait>(
    connection: &C,
    actor: &AuthenticatedUser,
    slug: &str,
) -> Result<(), IdentityError> {
    let owner = namespace::Entity::find_by_id(slug)
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    let permitted = match owner.kind.as_str() {
        "user" => owner.user_id == Some(actor.user.id),
        "organization" => match owner.organization_id {
            Some(organization_id) => {
                organization_member::Entity::find_by_id((organization_id, actor.user.id))
                    .filter(organization_member::Column::Role.eq("owner"))
                    .one(connection)
                    .await?
                    .is_some()
            }
            None => false,
        },
        _ => false,
    };
    if permitted {
        Ok(())
    } else {
        Err(IdentityError::not_found())
    }
}

pub async fn accessible_organization<C: ConnectionTrait>(
    connection: &C,
    slug: &str,
    user_id: Uuid,
) -> Result<organization::Model, IdentityError> {
    let organization = organization::Entity::find()
        .filter(organization::Column::Slug.eq(slug))
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    organization_member::Entity::find_by_id((organization.id, user_id))
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    Ok(organization)
}

pub async fn owned_organization<C: ConnectionTrait>(
    connection: &C,
    slug: &str,
    user_id: Uuid,
) -> Result<organization::Model, IdentityError> {
    let organization = accessible_organization(connection, slug, user_id).await?;
    let membership = organization_member::Entity::find_by_id((organization.id, user_id))
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    if membership.role != "owner" {
        return Err(IdentityError::forbidden(
            "Only an organization owner can manage this organization.",
        ));
    }
    Ok(organization)
}
