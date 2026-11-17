use axum::http::{HeaderMap, header};
pub(crate) use gitadel_actions::tokens::*;
use sea_orm::ConnectionTrait;

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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_api_accepts_bearer_and_forgejo_token_schemes() {
        for value in ["Bearer secret", "token secret", "TOKEN secret"] {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, value.parse().unwrap());
            assert_eq!(api_token(&headers), Some("secret"));
        }
    }
}
