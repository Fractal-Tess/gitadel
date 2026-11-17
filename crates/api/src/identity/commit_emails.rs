//! Commit identities: the addresses a user writes commits under, which
//! attribute those commits to them on their profile, and the name and address
//! commits made in the browser are authored with.
//!
//! A commit counts as a user's when its author address is one of their
//! verified commit addresses or their account address, or when it carries a
//! valid SSH signature from one of their keys.
//!
//! An address is verified by opening an emailed link. Until then it is a
//! claim: it attributes nothing, cannot author browser commits, and does not
//! stop the real owner from verifying the same address, which removes the
//! unverified claim. When the instance cannot send mail, addresses are
//! verified on creation, as they were before verification existed.

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE};
use crate::entity::user;
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::ConnectionTrait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Everything that marks a commit as one user's.
pub use gitadel_identity::commit_emails::CommitIdentities;

/// The user's verified identities. Unverified claims are left out, so a
/// claimed address attributes nothing until its owner proves it.
pub async fn commit_identities<C: ConnectionTrait>(
    database: &C,
    account: &user::Model,
) -> Result<CommitIdentities, ApiError> {
    Ok(domain::commit_identities(database, account).await?)
}

/// The name and address a commit made in the browser is authored with: the
/// chosen ones, else the account address, else the instance placeholder.
///
/// A chosen address that is no longer verified, because its real owner
/// verified it elsewhere, falls back rather than impersonating them.
pub async fn commit_author<C: ConnectionTrait>(
    database: &C,
    account: &user::Model,
) -> Result<(String, String), ApiError> {
    Ok(domain::commit_author(database, account).await?)
}

#[cfg(test)]
fn normalize_email(raw: &str) -> Result<String, ApiError> {
    Ok(domain::normalize_email(raw)?)
}

#[cfg(test)]
fn normalize_name(raw: &str) -> Result<Option<String>, ApiError> {
    Ok(domain::normalize_name(raw)?)
}

/// How an address came to count, or why it does not yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = CommitEmailVerification)]
pub enum Verification {
    /// Proved by opening the emailed link.
    Verified,
    /// Waiting for the emailed link; attributes nothing yet.
    Pending,
    /// Accepted as entered because this instance cannot send mail.
    NotRequired,
}

#[derive(Serialize, ToSchema)]
pub struct CommitEmailResponse {
    email: String,
    verified_at: Option<chrono::DateTime<Utc>>,
    verification: Verification,
}

#[derive(Serialize, ToSchema)]
pub struct CommitIdentityResponse {
    /// The chosen name, if any.
    name: Option<String>,
    /// The chosen address for browser commits, if any.
    primary_email: Option<String>,
    /// The name and address browser commits actually use.
    author_name: String,
    author_email: String,
    emails: Vec<CommitEmailResponse>,
    /// The account address, which counts without being listed.
    account_email: Option<String>,
    /// Whether new addresses must be confirmed by email before they count.
    /// False when the instance cannot send mail.
    verification_required: bool,
}

#[utoipa::path(
    get, path = "/me/commit-identity",
    operation_id = "get_me_commit_identity",
    summary = "Get the commit identity",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = CommitIdentityResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_identity(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<CommitIdentityResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::get_identity(&state.auth_service(), &actor)
            .await?
            .into(),
    ))
}

#[derive(Deserialize, ToSchema)]
#[schema(as = UpdateCommitIdentityRequest)]
pub struct UpdateIdentityRequest {
    name: Option<String>,
    primary_email: Option<String>,
}

/// Sets the name and address browser commits are authored with. The address
/// must be one of the user's verified commit addresses or their account
/// address.
#[utoipa::path(
    put, path = "/me/commit-identity",
    operation_id = "put_me_commit_identity",
    summary = "Update the commit identity",
    tag = "Account",

    responses(
        (status = 200, description = "Success", body = CommitIdentityResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_identity(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateIdentityRequest>,
) -> Result<Json<CommitIdentityResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok(Json(
        domain::update_identity(
            &state.auth_service(),
            &actor,
            domain::UpdateIdentityRequest {
                name: request.name,
                primary_email: request.primary_email,
            },
        )
        .await?
        .into(),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct AddEmailRequest {
    email: String,
}

/// Claims an address for the user's commits. When the instance sends mail the
/// claim stays pending until the emailed link is opened; otherwise it counts
/// at once. An address another account has verified is refused, so nobody
/// can count someone else's commits as their own.
#[utoipa::path(
    post, path = "/me/commit-emails",
    operation_id = "post_me_commit_emails",
    summary = "Add a commit email address",
    tag = "Account",

    responses(
        (status = 201, description = "Success", body = CommitIdentityResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn add_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<AddEmailRequest>,
) -> Result<(StatusCode, Json<CommitIdentityResponse>), ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(
            domain::add_email(
                &state.auth_service(),
                &actor,
                domain::AddEmailRequest {
                    email: request.email,
                },
            )
            .await?
            .into(),
        ),
    ))
}

/// Sends a fresh verification link for a pending address.
#[utoipa::path(
    post, path = "/me/commit-emails/{email}/verification",
    operation_id = "post_me_commit_emails_email_verification",
    summary = "Resend a commit email verification",
    tag = "Account",
    params(
        ("email" = String, Path),
    ),
    responses(
        (status = 202, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn resend_verification(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(email): Path<String>,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::resend_verification(&state.auth_service(), &actor, email).await?;
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(
    delete, path = "/me/commit-emails/{email}",
    operation_id = "delete_me_commit_emails_email",
    summary = "Remove a commit email address",
    tag = "Account",
    params(
        ("email" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = CommitIdentityResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn remove_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(email): Path<String>,
) -> Result<Json<CommitIdentityResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok(Json(
        domain::remove_email(&state.auth_service(), &actor, email)
            .await?
            .into(),
    ))
}

#[cfg(test)]
use super::email::{PURPOSE_COMMIT_VERIFY, VERIFY_LIFETIME, issue_token};
use gitadel_identity::commit_emails as domain;

impl From<domain::CommitEmailResponse> for CommitEmailResponse {
    fn from(value: domain::CommitEmailResponse) -> Self {
        Self {
            email: value.email,
            verified_at: value.verified_at,
            verification: value.verification.into(),
        }
    }
}

#[cfg(test)]
use crate::entity::email_token;
#[cfg(test)]
use crate::entity::user_commit_email;
#[cfg(test)]
use crate::entity::user_commit_profile;
#[cfg(test)]
use std::collections::HashSet;
impl From<domain::CommitIdentityResponse> for CommitIdentityResponse {
    fn from(value: domain::CommitIdentityResponse) -> Self {
        Self {
            name: value.name,
            primary_email: value.primary_email,
            author_name: value.author_name,
            author_email: value.author_email,
            emails: value.emails.into_iter().map(Into::into).collect(),
            account_email: value.account_email,
            verification_required: value.verification_required,
        }
    }
}
impl From<domain::Verification> for Verification {
    fn from(value: domain::Verification) -> Self {
        match value {
            domain::Verification::Verified => Self::Verified,
            domain::Verification::Pending => Self::Pending,
            domain::Verification::NotRequired => Self::NotRequired,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalized_and_validated() {
        assert_eq!(
            normalize_email("  Me@Example.COM ").unwrap(),
            "me@example.com"
        );
        for invalid in [
            "",
            "no-at",
            "@example.com",
            "me@",
            "a@b@c",
            "me @x.com",
            "<me@x.com>",
        ] {
            assert!(normalize_email(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn names_are_optional_and_cannot_break_the_author_line() {
        assert_eq!(normalize_name("  ").unwrap(), None);
        assert_eq!(
            normalize_name(" Fractal Tess ").unwrap().as_deref(),
            Some("Fractal Tess")
        );
        assert!(normalize_name("Evil <x@y>").is_err());
        assert!(normalize_name("line\nbreak").is_err());
    }

    #[test]
    fn a_commit_matches_by_address_or_signing_key() {
        let identities = CommitIdentities {
            emails: HashSet::from(["me@example.com".to_owned()]),
            fingerprints: HashSet::from(["SHA256:key".to_owned()]),
        };
        assert!(identities.matches("me@example.com", None));
        assert!(identities.matches("other@example.com", Some("SHA256:key")));
        assert!(!identities.matches("other@example.com", Some("SHA256:else")));
        assert!(!identities.matches("other@example.com", None));
    }

    mod handlers {
        use axum::http::{Method, StatusCode};
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};
        use serde_json::{Value, json};

        use super::super::*;
        use crate::{
            identity::test_support::{Harness, mailed_token},
            mail::{fake_smtp_server, test_mailer},
        };

        fn emails(identity: &Value) -> Vec<(String, String)> {
            identity["emails"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| {
                    (
                        entry["email"].as_str().unwrap().to_owned(),
                        entry["verification"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        }

        async fn verified_identities(harness: &Harness, account: &user::Model) -> HashSet<String> {
            commit_identities(&harness.database, account)
                .await
                .unwrap()
                .emails
        }

        #[tokio::test]
        async fn without_mail_addresses_count_at_once_and_say_so() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;

            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&cookie),
                    Some(json!({ "email": "Alice@Example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            assert_eq!(body["verification_required"], Value::Bool(false));
            assert_eq!(
                emails(&body),
                vec![("alice@example.com".to_owned(), "not_required".to_owned())]
            );
            assert!(body["emails"][0]["verified_at"].is_string());
            assert!(
                verified_identities(&harness, &alice)
                    .await
                    .contains("alice@example.com")
            );
        }

        #[tokio::test]
        async fn with_mail_addresses_are_pending_until_the_link_is_opened() {
            let (port, server) = fake_smtp_server().await;
            let harness = Harness::new().await.with_mailer(test_mailer(port));
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;

            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&cookie),
                    Some(json!({ "email": "alice@example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            assert_eq!(body["verification_required"], Value::Bool(true));
            assert_eq!(
                emails(&body),
                vec![("alice@example.com".to_owned(), "pending".to_owned())]
            );
            assert!(body["emails"][0]["verified_at"].is_null());
            assert!(
                !verified_identities(&harness, &alice)
                    .await
                    .contains("alice@example.com"),
                "a pending address attributes nothing"
            );

            // A pending address cannot author browser commits either.
            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/commit-identity",
                    Some(&cookie),
                    Some(json!({ "primary_email": "alice@example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

            let mail = tokio::time::timeout(std::time::Duration::from_secs(10), server)
                .await
                .unwrap()
                .unwrap();
            assert!(mail.contains("Subject: Verify your Gitadel commit email"));
            let token = mailed_token(&mail);

            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/auth/email/verify",
                    None,
                    Some(json!({ "token": token })),
                )
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
            assert!(
                verified_identities(&harness, &alice)
                    .await
                    .contains("alice@example.com")
            );
            let (_, body) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/commit-identity",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(
                emails(&body),
                vec![("alice@example.com".to_owned(), "verified".to_owned())]
            );
            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/commit-identity",
                    Some(&cookie),
                    Some(json!({ "primary_email": "alice@example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["author_email"], "alice@example.com");

            // The link is single use.
            let (status, _) = harness
                .request(
                    Method::POST,
                    "/api/v1/auth/email/verify",
                    None,
                    Some(json!({ "token": token })),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }

        #[tokio::test]
        async fn an_unverified_claim_cannot_block_the_owner_and_is_removed_when_they_verify() {
            let harness = Harness::new().await.with_mailer(test_mailer(1));
            let squatter = harness.user("squatter").await;
            let owner = harness.user("owner").await;
            let now = Utc::now();
            user_commit_email::ActiveModel {
                user_id: Set(squatter.id),
                email: Set("owner@example.com".to_owned()),
                verified_at: Set(None),
                created_at: Set(now),
            }
            .insert(&harness.database)
            .await
            .unwrap();
            user_commit_profile::ActiveModel {
                user_id: Set(squatter.id),
                name: Set(None),
                primary_email: Set(Some("owner@example.com".to_owned())),
                updated_at: Set(now),
            }
            .insert(&harness.database)
            .await
            .unwrap();

            // The owner can still claim the address.
            let owner_cookie = harness.login(&owner).await;
            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&owner_cookie),
                    Some(json!({ "email": "owner@example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");

            // ... and verifying it removes the squatter's claim.
            let token = issue_token(
                &harness.database,
                owner.id,
                PURPOSE_COMMIT_VERIFY,
                Some("owner@example.com"),
                VERIFY_LIFETIME,
            )
            .await
            .unwrap();
            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/auth/email/verify",
                    None,
                    Some(json!({ "token": token })),
                )
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
            assert!(
                verified_identities(&harness, &owner)
                    .await
                    .contains("owner@example.com")
            );
            assert!(
                user_commit_email::Entity::find_by_id((
                    squatter.id,
                    "owner@example.com".to_owned()
                ))
                .one(&harness.database)
                .await
                .unwrap()
                .is_none()
            );
            let squatter_profile = user_commit_profile::Entity::find_by_id(squatter.id)
                .one(&harness.database)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(squatter_profile.primary_email, None);

            // Once verified, the address is taken.
            let squatter_cookie = harness.login(&squatter).await;
            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&squatter_cookie),
                    Some(json!({ "email": "owner@example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
        }

        #[tokio::test]
        async fn resending_needs_a_pending_address_and_mail() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;
            harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&cookie),
                    Some(json!({ "email": "alice@example.com" })),
                )
                .await;
            let (status, _) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails/alice%40example.com/verification",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::CONFLICT, "mail is not configured");

            let (port, server) = fake_smtp_server().await;
            let harness = Harness::new().await.with_mailer(test_mailer(port));
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;
            user_commit_email::ActiveModel {
                user_id: Set(alice.id),
                email: Set("alice@example.com".to_owned()),
                verified_at: Set(None),
                created_at: Set(Utc::now()),
            }
            .insert(&harness.database)
            .await
            .unwrap();
            let (status, _) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails/alice%40example.com/verification",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::ACCEPTED);
            let mail = tokio::time::timeout(std::time::Duration::from_secs(10), server)
                .await
                .unwrap()
                .unwrap();
            assert!(mail.contains("/verify-email?token="));
            let (status, _) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails/nobody%40example.com/verification",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn removing_a_pending_address_discards_its_tokens() {
            let harness = Harness::new().await.with_mailer(test_mailer(1));
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;
            harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&cookie),
                    Some(json!({ "email": "alice@example.com" })),
                )
                .await;
            assert_eq!(
                email_token::Entity::find()
                    .all(&harness.database)
                    .await
                    .unwrap()
                    .len(),
                1
            );
            let (status, body) = harness
                .request(
                    Method::DELETE,
                    "/api/v1/me/commit-emails/alice%40example.com",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(emails(&body).is_empty());
            assert!(
                email_token::Entity::find()
                    .all(&harness.database)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
