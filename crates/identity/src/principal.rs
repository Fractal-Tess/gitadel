//! Credential authority retained across domain calls.

use chrono::Utc;
use gitadel_db::entity::{api_token, oauth_access_token, session};
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::{AuthenticatedUser, IdentityError, IdentityService, SCOPE_READ, SCOPE_REPOSITORY_READ};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    Browser,
    PersonalToken,
    OAuthToken,
}

#[derive(Clone)]
pub(crate) enum Credential {
    Browser { hash: String },
    Personal { id: Uuid, scopes: i32 },
    OAuth { hash: String, scopes: i32 },
}

/// Only credential verification can construct a principal. Legacy actors remain
/// available for compatibility but cannot grant authority to a domain service.
#[derive(Clone)]
pub struct Principal {
    pub(crate) actor: AuthenticatedUser,
    pub(crate) credential: Credential,
}

impl std::ops::Deref for Principal {
    type Target = AuthenticatedUser;
    fn deref(&self) -> &AuthenticatedUser {
        &self.actor
    }
}

impl Principal {
    pub fn source(&self) -> CredentialSource {
        match self.credential {
            Credential::Browser { .. } => CredentialSource::Browser,
            Credential::Personal { .. } => CredentialSource::PersonalToken,
            Credential::OAuth { .. } => CredentialSource::OAuthToken,
        }
    }

    pub fn granted_scopes(&self) -> i32 {
        match self.credential {
            Credential::Browser { .. } => i32::MAX,
            Credential::Personal { scopes, .. } | Credential::OAuth { scopes, .. } => scopes,
        }
    }

    /// Recheck revocation, expiration, user binding and current scopes on the writer.
    pub async fn authorize(
        &self,
        database: &DatabaseConnection,
        scope: i32,
    ) -> Result<AuthenticatedUser, IdentityError> {
        let now = Utc::now();
        let (user_id, scopes, required, message) = match &self.credential {
            Credential::Browser { hash } => {
                let row = session::Entity::find_by_id(hash)
                    .one(database)
                    .await?
                    .filter(|row| row.expires_at > now)
                    .ok_or_else(IdentityError::unauthorized)?;
                (
                    row.user_id,
                    i32::MAX,
                    scope,
                    "The session does not have the required scope.",
                )
            }
            Credential::Personal { id, .. } => {
                let row = api_token::Entity::find_by_id(*id)
                    .one(database)
                    .await?
                    .filter(|row| {
                        row.revoked_at.is_none() && row.expires_at.is_none_or(|at| at > now)
                    })
                    .ok_or_else(IdentityError::unauthorized)?;
                let required = if scope & SCOPE_REPOSITORY_READ != 0 {
                    (scope & !SCOPE_REPOSITORY_READ) | SCOPE_READ
                } else {
                    scope
                };
                (
                    row.user_id,
                    row.scopes,
                    required,
                    "The API token does not have the required scope.",
                )
            }
            Credential::OAuth { hash, .. } => {
                let row = oauth_access_token::Entity::find_by_id(hash)
                    .one(database)
                    .await?
                    .filter(|row| row.revoked_at.is_none() && row.expires_at > now)
                    .ok_or_else(IdentityError::unauthorized)?;
                (
                    row.user_id,
                    row.scopes,
                    scope,
                    "The OAuth token does not have the required scope.",
                )
            }
        };
        if user_id != self.user.id {
            return Err(IdentityError::unauthorized());
        }
        if scopes & required != required {
            return Err(IdentityError::forbidden(message));
        }
        let user = IdentityService::new(database)
            .enabled_user(user_id)
            .await?
            .ok_or_else(IdentityError::unauthorized)?;
        Ok(AuthenticatedUser {
            user,
            via_api_token: self.via_api_token,
        })
    }
}
