//! Verified commit addresses and browser commit identity policy.
use crate::{
    IdentityError, Principal,
    email::{PURPOSE_COMMIT_VERIFY, VERIFY_LIFETIME, consume_token, issue_token, link},
    runtime::AuthService,
};
use chrono::Utc;
use gitadel_db::entity::{
    email_token, ssh_key, user, user_commit_email, user_commit_profile, user_email,
};
use gitadel_mail::{Email, parse_address};
use lettre::message::Mailbox;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait, sea_query::Expr,
};
use std::collections::HashSet;
use uuid::Uuid;
const MAX_EMAIL_BYTES: usize = 254;
const MAX_NAME_CHARS: usize = 100;
const MAX_COMMIT_EMAILS: usize = 20;
#[derive(Debug, Default)]
pub struct CommitIdentities {
    /// Lowercased author addresses.
    pub emails: HashSet<String>,
    /// SHA256 fingerprints of the user's SSH keys, as signatures report them.
    pub fingerprints: HashSet<String>,
}

pub struct CommitEmailResponse {
    pub email: String,
    pub verified_at: Option<chrono::DateTime<Utc>>,
    pub verification: Verification,
}

pub struct CommitIdentityResponse {
    /// The chosen name, if any.
    pub name: Option<String>,
    /// The chosen address for browser commits, if any.
    pub primary_email: Option<String>,
    /// The name and address browser commits actually use.
    pub author_name: String,
    pub author_email: String,
    pub emails: Vec<CommitEmailResponse>,
    /// The account address, which counts without being listed.
    pub account_email: Option<String>,
    /// Whether new addresses must be confirmed by email before they count.
    /// False when the instance cannot send mail.
    pub verification_required: bool,
}

pub struct UpdateIdentityRequest {
    pub name: Option<String>,
    pub primary_email: Option<String>,
}

pub struct AddEmailRequest {
    pub email: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
    /// Proved by opening the emailed link.
    Verified,
    /// Waiting for the emailed link; attributes nothing yet.
    Pending,
    /// Accepted as entered because this instance cannot send mail.
    NotRequired,
}
impl CommitIdentities {
    pub fn matches(&self, email: &str, signer: Option<&str>) -> bool {
        self.emails.contains(email) || signer.is_some_and(|key| self.fingerprints.contains(key))
    }
}
impl CommitEmailResponse {
    fn new(row: user_commit_email::Model, mail_enabled: bool) -> Self {
        let verification = match (row.verified_at, mail_enabled) {
            (None, _) => Verification::Pending,
            (Some(_), true) => Verification::Verified,
            (Some(_), false) => Verification::NotRequired,
        };
        Self {
            email: row.email,
            verified_at: row.verified_at,
            verification,
        }
    }
}
pub(crate) fn placeholder_email(username: &str) -> String {
    format!("{}@gitadel.local", username.to_ascii_lowercase())
}
pub async fn commit_identities<C: ConnectionTrait>(
    database: &C,
    account: &user::Model,
) -> Result<CommitIdentities, IdentityError> {
    let mut emails = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::UserId.eq(account.id))
        .filter(user_commit_email::Column::VerifiedAt.is_not_null())
        .all(database)
        .await?
        .into_iter()
        .map(|row| row.email)
        .collect::<HashSet<_>>();
    if let Some(row) = user_email::Entity::find_by_id(account.id)
        .one(database)
        .await?
    {
        emails.insert(row.email.to_ascii_lowercase());
    }
    emails.insert(placeholder_email(&account.username));
    let fingerprints = ssh_key::Entity::find()
        .filter(ssh_key::Column::UserId.eq(account.id))
        .all(database)
        .await?
        .into_iter()
        .map(|key| key.fingerprint)
        .collect();
    Ok(CommitIdentities {
        emails,
        fingerprints,
    })
}
pub async fn commit_author<C: ConnectionTrait>(
    database: &C,
    account: &user::Model,
) -> Result<(String, String), IdentityError> {
    let profile = user_commit_profile::Entity::find_by_id(account.id)
        .one(database)
        .await?;
    let name = profile
        .as_ref()
        .and_then(|profile| profile.name.clone())
        .unwrap_or_else(|| account.username.clone());
    let chosen = match profile.and_then(|profile| profile.primary_email) {
        Some(email)
            if commit_identities(database, account)
                .await?
                .emails
                .contains(&email) =>
        {
            Some(email)
        }
        _ => None,
    };
    let email = match chosen {
        Some(email) => email,
        None => user_email::Entity::find_by_id(account.id)
            .one(database)
            .await?
            .map(|row| row.email)
            .unwrap_or_else(|| placeholder_email(&account.username)),
    };
    Ok((name, email))
}
pub fn normalize_email(raw: &str) -> Result<String, IdentityError> {
    let email = raw.trim().to_ascii_lowercase();
    let valid = email.len() <= MAX_EMAIL_BYTES
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && !domain.is_empty() && !domain.contains('@')
        })
        && !email.chars().any(|character| {
            character.is_whitespace() || character.is_control() || "<>,".contains(character)
        });
    if valid {
        Ok(email)
    } else {
        Err(IdentityError::bad_request("Enter a valid email address."))
    }
}
pub fn normalize_name(raw: &str) -> Result<Option<String>, IdentityError> {
    let name = raw.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > MAX_NAME_CHARS
        || name
            .chars()
            .any(|character| character.is_control() || "<>".contains(character))
    {
        return Err(IdentityError::bad_request(
            "The commit name must be at most 100 characters, without < or >.",
        ));
    }
    Ok(Some(name.to_owned()))
}
pub(crate) async fn identity_response<C: ConnectionTrait>(
    state: &AuthService<'_>,
    database: &C,
    account: &user::Model,
) -> Result<CommitIdentityResponse, IdentityError> {
    let mail_enabled = state.mailer().is_enabled();
    let emails = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::UserId.eq(account.id))
        .order_by_asc(user_commit_email::Column::CreatedAt)
        .all(database)
        .await?
        .into_iter()
        .map(|row| CommitEmailResponse::new(row, mail_enabled))
        .collect();
    let profile = user_commit_profile::Entity::find_by_id(account.id)
        .one(database)
        .await?;
    let account_email = user_email::Entity::find_by_id(account.id)
        .one(database)
        .await?
        .map(|row| row.email);
    let (author_name, author_email) = commit_author(database, account).await?;
    Ok(CommitIdentityResponse {
        name: profile.as_ref().and_then(|profile| profile.name.clone()),
        primary_email: profile.and_then(|profile| profile.primary_email),
        author_name,
        author_email,
        emails,
        account_email,
        verification_required: mail_enabled,
    })
}
pub(crate) async fn verified_by_another<C: ConnectionTrait>(
    database: &C,
    user_id: Uuid,
    email: &str,
) -> Result<bool, IdentityError> {
    let commit_claim = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::Email.eq(email))
        .filter(user_commit_email::Column::UserId.ne(user_id))
        .filter(user_commit_email::Column::VerifiedAt.is_not_null())
        .one(database)
        .await?
        .is_some();
    if commit_claim {
        return Ok(true);
    }
    Ok(user_email::Entity::find()
        .filter(user_email::Column::Email.eq(email))
        .filter(user_email::Column::UserId.ne(user_id))
        .filter(user_email::Column::VerifiedAt.is_not_null())
        .one(database)
        .await?
        .is_some())
}
pub async fn verify_with_token<C: ConnectionTrait>(
    state: &AuthService<'_>,
    connection: &C,
    token_hash: &str,
) -> Result<bool, IdentityError> {
    let Some(token) = consume_token(connection, token_hash, PURPOSE_COMMIT_VERIFY).await? else {
        return Ok(false);
    };
    let Some(email) = token.email else {
        return Ok(false);
    };
    let Some(row) = user_commit_email::Entity::find_by_id((token.user_id, email.clone()))
        .one(connection)
        .await?
    else {
        return Ok(false);
    };
    if row.verified_at.is_some() {
        return Ok(true);
    }
    if verified_by_another(connection, token.user_id, &email).await? {
        return Err(IdentityError::conflict(
            "That address was verified by another account in the meantime.",
        ));
    }
    let now = Utc::now();
    let losing_claims = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::Email.eq(&email))
        .filter(user_commit_email::Column::UserId.ne(token.user_id))
        .all(connection)
        .await?;
    for claim in losing_claims {
        remove_claim(connection, &claim).await?;
    }
    user_email::Entity::delete_many()
        .filter(user_email::Column::Email.eq(&email))
        .filter(user_email::Column::UserId.ne(token.user_id))
        .filter(user_email::Column::VerifiedAt.is_null())
        .exec(connection)
        .await?;
    user_commit_email::Entity::update_many()
        .col_expr(
            user_commit_email::Column::VerifiedAt,
            Expr::value(Some(now)),
        )
        .filter(user_commit_email::Column::UserId.eq(token.user_id))
        .filter(user_commit_email::Column::Email.eq(&email))
        .exec(connection)
        .await?;
    state
        .audit_on(
            connection,
            Some(token.user_id),
            "account.commit_email.verify",
            Some(email),
        )
        .await?;
    Ok(true)
}
pub(crate) async fn remove_claim<C: ConnectionTrait>(
    connection: &C,
    claim: &user_commit_email::Model,
) -> Result<(), IdentityError> {
    user_commit_email::Entity::delete_by_id((claim.user_id, claim.email.clone()))
        .exec(connection)
        .await?;
    email_token::Entity::delete_many()
        .filter(email_token::Column::UserId.eq(claim.user_id))
        .filter(email_token::Column::Purpose.eq(PURPOSE_COMMIT_VERIFY))
        .filter(email_token::Column::Email.eq(&claim.email))
        .exec(connection)
        .await?;
    user_commit_profile::Entity::update_many()
        .col_expr(
            user_commit_profile::Column::PrimaryEmail,
            Expr::value(Option::<String>::None),
        )
        .col_expr(
            user_commit_profile::Column::UpdatedAt,
            Expr::value(Utc::now()),
        )
        .filter(user_commit_profile::Column::UserId.eq(claim.user_id))
        .filter(user_commit_profile::Column::PrimaryEmail.eq(&claim.email))
        .exec(connection)
        .await?;
    Ok(())
}
pub(crate) fn send_verification(
    state: &AuthService<'_>,
    account: &user::Model,
    address: &str,
    token: &str,
) {
    let Some(mailbox) = parse_address(address) else {
        return;
    };
    let link = link(state, "/verify-email", token);
    state.mailer().enqueue(Email {
        to: Mailbox::new(Some(account.username.clone()), mailbox),
        subject: "Verify your Gitadel commit email".to_owned(),
        body: format!(
            "Confirm that commits written as {address} belong to the Gitadel \
             account \"{}\" at {} by opening this link within 24 hours:\n\n{link}\n\n\
             Until then the address counts for nothing. If you did not add it, \
             ignore this email.\n",
            account.username,
            state.public_url(),
        ),
    });
}
pub async fn get_identity(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<CommitIdentityResponse, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    identity_response(state, state.database(), &actor.user).await
}
pub async fn update_identity(
    state: &AuthService<'_>,
    actor: &Principal,
    request: UpdateIdentityRequest,
) -> Result<CommitIdentityResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let name = request
        .name
        .as_deref()
        .map(normalize_name)
        .transpose()?
        .flatten();
    let primary_email = request
        .primary_email
        .as_deref()
        .filter(|email| !email.trim().is_empty())
        .map(normalize_email)
        .transpose()?;
    if let Some(email) = &primary_email {
        let identities = commit_identities(state.database(), &actor.user).await?;
        if *email == placeholder_email(&actor.user.username) || !identities.emails.contains(email) {
            return Err(IdentityError::bad_request(
                "Add and verify the address in your commit emails before choosing it.",
            ));
        }
    }
    let transaction = state.database().begin().await?;
    user_commit_profile::Entity::delete_by_id(actor.user.id)
        .exec(&transaction)
        .await?;
    user_commit_profile::ActiveModel {
        user_id: Set(actor.user.id),
        name: Set(name),
        primary_email: Set(primary_email),
        updated_at: Set(Utc::now()),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.commit_identity.update",
            None,
        )
        .await?;
    transaction.commit().await?;
    identity_response(state, state.database(), &actor.user).await
}
pub async fn add_email(
    state: &AuthService<'_>,
    actor: &Principal,
    request: AddEmailRequest,
) -> Result<CommitIdentityResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let email = normalize_email(&request.email)?;
    let database = state.database();
    if email.ends_with("@gitadel.local") {
        return Err(IdentityError::bad_request(
            "Addresses at gitadel.local are reserved for this instance.",
        ));
    }
    if let Some(existing) = user_commit_email::Entity::find_by_id((actor.user.id, email.clone()))
        .one(database)
        .await?
    {
        return Err(IdentityError::conflict(if existing.verified_at.is_some() {
            "That address is already on your list."
        } else {
            "That address is already waiting for verification. Resend the email if it did not arrive."
        }));
    }
    if user_email::Entity::find_by_id(actor.user.id)
        .one(database)
        .await?
        .is_some_and(|row| row.email.eq_ignore_ascii_case(&email))
    {
        return Err(IdentityError::conflict(
            "Your account email already counts; there is no need to add it.",
        ));
    }
    if verified_by_another(database, actor.user.id, &email).await? {
        return Err(IdentityError::conflict(
            "That address belongs to another account.",
        ));
    }
    let count = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::UserId.eq(actor.user.id))
        .all(database)
        .await?
        .len();
    if count >= MAX_COMMIT_EMAILS {
        return Err(IdentityError::bad_request(format!(
            "Add at most {MAX_COMMIT_EMAILS} commit emails."
        )));
    }
    let mail_enabled = state.mailer().is_enabled();
    if mail_enabled {
        state
            .enforce_rate_limit(format!("commit-email-verify:{}", actor.user.id), 5)
            .await?;
    }
    let now = Utc::now();
    let transaction = database.begin().await?;
    user_commit_email::ActiveModel {
        user_id: Set(actor.user.id),
        email: Set(email.clone()),
        verified_at: Set((!mail_enabled).then_some(now)),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    let token = if mail_enabled {
        Some(
            issue_token(
                &transaction,
                actor.user.id,
                PURPOSE_COMMIT_VERIFY,
                Some(&email),
                VERIFY_LIFETIME,
            )
            .await?,
        )
    } else {
        None
    };
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.commit_email.add",
            Some(email.clone()),
        )
        .await?;
    transaction.commit().await?;
    if let Some(token) = token {
        send_verification(state, &actor.user, &email, &token);
    }
    identity_response(state, database, &actor.user).await
}
pub async fn resend_verification(
    state: &AuthService<'_>,
    actor: &Principal,
    email: String,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let email = normalize_email(&email)?;
    if !state.mailer().is_enabled() {
        return Err(IdentityError::conflict(
            "Email is not configured on this Gitadel instance.",
        ));
    }
    let row = user_commit_email::Entity::find_by_id((actor.user.id, email.clone()))
        .one(state.database())
        .await?
        .ok_or_else(IdentityError::not_found)?;
    if row.verified_at.is_some() {
        return Err(IdentityError::conflict("This address is already verified."));
    }
    state
        .enforce_rate_limit(format!("commit-email-verify:{}", actor.user.id), 5)
        .await?;
    let transaction = state.database().begin().await?;
    let token = issue_token(
        &transaction,
        actor.user.id,
        PURPOSE_COMMIT_VERIFY,
        Some(&email),
        VERIFY_LIFETIME,
    )
    .await?;
    transaction.commit().await?;
    send_verification(state, &actor.user, &email, &token);
    Ok(())
}
pub async fn remove_email(
    state: &AuthService<'_>,
    actor: &Principal,
    email: String,
) -> Result<CommitIdentityResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let email = normalize_email(&email)?;
    let database = state.database();
    let transaction = database.begin().await?;
    let claim = user_commit_email::Entity::find_by_id((actor.user.id, email.clone()))
        .one(&transaction)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    remove_claim(&transaction, &claim).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.commit_email.remove",
            Some(email),
        )
        .await?;
    transaction.commit().await?;
    identity_response(state, database, &actor.user).await
}
