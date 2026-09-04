use axum::http::{HeaderMap, header};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::entity::{
    action_artifact_grant, action_job, action_job_token, action_run,
    action_runner_registration_token, repository,
};

const REGISTRATION_PREFIX: &str = "gta_reg_";
const RUNNER_PREFIX: &str = "gta_runner_";
const JOB_SCOPE_PREFIX: &str = "Actions.Results";

pub(crate) fn registration_token() -> String {
    token(REGISTRATION_PREFIX)
}

pub(crate) fn runner_token() -> String {
    token(RUNNER_PREFIX)
}

#[derive(Deserialize, Serialize)]
struct JobTokenHeader {
    alg: String,
    typ: String,
}

#[derive(Deserialize, Serialize)]
struct JobTokenClaims {
    exp: i64,
    jti: Uuid,
    scp: String,
}

fn token(prefix: &str) -> String {
    let secret: [u8; 32] = rand::random();
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(secret))
}

fn job_token(
    run_id: Uuid,
    job_id: i64,
    token_id: Uuid,
    expires_at: chrono::DateTime<Utc>,
) -> Result<String, serde_json::Error> {
    let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&JobTokenHeader {
        alg: "HS256".to_owned(),
        typ: "JWT".to_owned(),
    })?);
    let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&JobTokenClaims {
        exp: expires_at.timestamp(),
        jti: token_id,
        scp: format!("{JOB_SCOPE_PREFIX}:{run_id}:{job_id}"),
    })?);
    let signature: [u8; 32] = rand::random();
    Ok(format!(
        "{header}.{claims}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

fn decode_job_token(raw: &str) -> Option<(JobTokenHeader, JobTokenClaims)> {
    let mut segments = raw.split('.');
    let header = segments.next()?;
    let claims = segments.next()?;
    let signature = segments.next()?;
    if signature.is_empty() || segments.next().is_some() {
        return None;
    }
    let header = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
    let claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).ok()?).ok()?;
    Some((header, claims))
}

#[derive(Clone)]
pub(crate) struct AuthenticatedJob {
    pub(crate) token: action_job_token::Model,
    pub(crate) job: action_job::Model,
    pub(crate) run: action_run::Model,
    pub(crate) repository: repository::Model,
}

pub(crate) fn digest(value: &str) -> String {
    let bytes = Sha256::digest(value.as_bytes());
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn digest_matches(value: &str, stored: &str) -> bool {
    let candidate = digest(value);
    candidate.as_bytes().ct_eq(stored.as_bytes()).into()
}

pub(crate) async fn issue_registration<C: ConnectionTrait>(
    database: &C,
    namespace: Option<String>,
    created_by: Option<Uuid>,
    runner_name: String,
    labels: Vec<String>,
) -> Result<String, sea_orm::DbErr> {
    let raw = registration_token();
    let now = Utc::now();
    action_runner_registration_token::ActiveModel {
        id: Set(Uuid::new_v4()),
        token_hash: Set(digest(&raw)),
        namespace: Set(namespace),
        runner_name: Set(runner_name),
        approved_labels: Set(serde_json::to_string(&labels).expect("labels serialize")),
        expires_at: Set(now + Duration::minutes(10)),
        used_at: Set(None),
        created_by: Set(created_by),
        created_at: Set(now),
    }
    .insert(database)
    .await?;
    Ok(raw)
}

pub(crate) async fn consume_registration<C: ConnectionTrait>(
    database: &C,
    raw: &str,
) -> Result<Option<action_runner_registration_token::Model>, sea_orm::DbErr> {
    if !raw.starts_with(REGISTRATION_PREFIX) {
        return Ok(None);
    }
    let stored = action_runner_registration_token::Entity::find()
        .filter(action_runner_registration_token::Column::TokenHash.eq(digest(raw)))
        .filter(action_runner_registration_token::Column::UsedAt.is_null())
        .filter(action_runner_registration_token::Column::ExpiresAt.gt(Utc::now()))
        .one(database)
        .await?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    if !digest_matches(raw, &stored.token_hash) {
        return Ok(None);
    }
    let mut active: action_runner_registration_token::ActiveModel = stored.clone().into();
    active.used_at = Set(Some(Utc::now()));
    active.update(database).await?;
    Ok(Some(stored))
}

pub(crate) async fn issue_job<C: ConnectionTrait>(
    database: &C,
    job_id: i64,
    repository_id: Uuid,
    lease_generation: i64,
) -> Result<String, sea_orm::DbErr> {
    action_job_token::Entity::update_many()
        .col_expr(action_job_token::Column::RevokedAt, Utc::now().into())
        .filter(action_job_token::Column::JobId.eq(job_id))
        .filter(action_job_token::Column::LeaseGeneration.ne(lease_generation))
        .filter(action_job_token::Column::RevokedAt.is_null())
        .exec(database)
        .await?;
    let run_id = action_job::Entity::find_by_id(job_id)
        .one(database)
        .await?
        .ok_or_else(|| sea_orm::DbErr::RecordNotFound("action job".to_owned()))?
        .run_id;
    let now = Utc::now();
    let expires_at = now + Duration::hours(6);
    let token_id = Uuid::new_v4();
    let raw = job_token(run_id, job_id, token_id, expires_at)
        .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
    action_job_token::ActiveModel {
        id: Set(token_id),
        job_id: Set(job_id),
        repository_id: Set(repository_id),
        lease_generation: Set(lease_generation),
        token_hash: Set(digest(&raw)),
        expires_at: Set(expires_at),
        revoked_at: Set(None),
        created_at: Set(now),
    }
    .insert(database)
    .await?;
    Ok(raw)
}

pub(crate) async fn authenticate_job<C: ConnectionTrait>(
    database: &C,
    raw: &str,
) -> Result<Option<AuthenticatedJob>, sea_orm::DbErr> {
    let Some((header, claims)) = decode_job_token(raw) else {
        return Ok(None);
    };
    if header.alg != "HS256" || header.typ != "JWT" {
        return Ok(None);
    }
    let token = action_job_token::Entity::find()
        .filter(action_job_token::Column::TokenHash.eq(digest(raw)))
        .filter(action_job_token::Column::RevokedAt.is_null())
        .filter(action_job_token::Column::ExpiresAt.gt(Utc::now()))
        .one(database)
        .await?;
    let Some(token) = token.filter(|row| digest_matches(raw, &row.token_hash)) else {
        return Ok(None);
    };
    if claims.jti != token.id
        || claims.exp != token.expires_at.timestamp()
        || claims.exp <= Utc::now().timestamp()
    {
        return Ok(None);
    }
    let Some(job) = action_job::Entity::find_by_id(token.job_id)
        .one(database)
        .await?
    else {
        return Ok(None);
    };
    let now = Utc::now();
    if job.lease_generation != token.lease_generation
        || !matches!(job.status.as_str(), "leased" | "running")
        || !job.lease_deadline.is_some_and(|deadline| deadline > now)
    {
        return Ok(None);
    }
    let Some(run) = action_run::Entity::find_by_id(job.run_id)
        .filter(action_run::Column::RepositoryId.eq(token.repository_id))
        .one(database)
        .await?
    else {
        return Ok(None);
    };
    if claims.scp != format!("{JOB_SCOPE_PREFIX}:{}:{}", run.id, job.id) {
        return Ok(None);
    }
    let Some(repository) = repository::Entity::find_by_id(run.repository_id)
        .one(database)
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(AuthenticatedJob {
        token,
        job,
        run,
        repository,
    }))
}

pub(crate) async fn authenticate_job_bearer<C: ConnectionTrait>(
    database: &C,
    headers: &HeaderMap,
) -> Result<Option<AuthenticatedJob>, sea_orm::DbErr> {
    let Some(raw) = bearer_token(headers) else {
        return Ok(None);
    };
    authenticate_job(database, raw).await
}

pub(crate) async fn authenticate_job_api_token<C: ConnectionTrait>(
    database: &C,
    headers: &HeaderMap,
) -> Result<Option<AuthenticatedJob>, sea_orm::DbErr> {
    let Some(raw) = api_token(headers) else {
        return Ok(None);
    };
    authenticate_job(database, raw).await
}

pub(crate) async fn authorize_job<C: ConnectionTrait>(
    database: &C,
    raw: &str,
    repository_id: Uuid,
) -> Result<Option<action_job_token::Model>, sea_orm::DbErr> {
    Ok(authenticate_job(database, raw)
        .await?
        .filter(|authenticated| authenticated.repository.id == repository_id)
        .map(|authenticated| authenticated.token))
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

fn api_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    ((scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("token"))
        && !token.is_empty())
    .then_some(token)
}

pub(crate) async fn revoke_job<C: ConnectionTrait>(
    database: &C,
    job_id: i64,
) -> Result<(), sea_orm::DbErr> {
    action_job_token::Entity::update_many()
        .col_expr(action_job_token::Column::RevokedAt, Utc::now().into())
        .filter(action_job_token::Column::JobId.eq(job_id))
        .filter(action_job_token::Column::RevokedAt.is_null())
        .exec(database)
        .await?;
    action_artifact_grant::Entity::update_many()
        .col_expr(action_artifact_grant::Column::RevokedAt, Utc::now().into())
        .filter(action_artifact_grant::Column::IssuedToJobId.eq(job_id))
        .filter(action_artifact_grant::Column::RevokedAt.is_null())
        .exec(database)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_have_distinct_shapes_and_256_bits() {
        assert_eq!(registration_token().len(), REGISTRATION_PREFIX.len() + 43);
        assert_eq!(runner_token().len(), RUNNER_PREFIX.len() + 43);
        let raw = job_token(
            Uuid::nil(),
            42,
            Uuid::new_v4(),
            Utc::now() + Duration::minutes(5),
        )
        .unwrap();
        let (_, claims) = decode_job_token(&raw).unwrap();
        assert_eq!(claims.scp, format!("{JOB_SCOPE_PREFIX}:{}:42", Uuid::nil()));
    }

    #[test]
    fn digest_comparison_rejects_different_values() {
        let raw = runner_token();
        assert!(digest_matches(&raw, &digest(&raw)));
        assert!(!digest_matches(&runner_token(), &digest(&raw)));
    }

    #[test]
    fn release_api_accepts_bearer_and_forgejo_token_schemes() {
        for value in ["Bearer secret", "token secret", "TOKEN secret"] {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, value.parse().unwrap());
            assert_eq!(api_token(&headers), Some("secret"));
        }
    }
}
