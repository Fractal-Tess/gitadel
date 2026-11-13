//! Hosted websites associated with a repository, in display order.
use super::{Permission, RepositoryState};
use crate::{
    entity::repository_website,
    identity::{ApiError, SCOPE_WRITE},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use sea_orm::{EntityTrait, Set, TransactionTrait, sea_query::OnConflict};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct Website {
    pub label: String,
    pub url: String,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateWebsites {
    pub websites: Vec<Website>,
}

pub async fn list(
    State(state): State<RepositoryState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<Website>>, ApiError> {
    let repository =
        super::browser::readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let row = repository_website::Entity::find_by_id(repository.id)
        .one(state.identity().database())
        .await?;
    Ok(Json(
        row.map(|row| serde_json::from_str(&row.links))
            .transpose()
            .map_err(ApiError::internal)?
            .unwrap_or_default(),
    ))
}

pub async fn update(
    State(state): State<RepositoryState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateWebsites>,
) -> Result<Json<Vec<Website>>, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let repository = state.find(&namespace, &name).await?;
    state
        .authorize(&repository, Some(actor.user.id), Permission::Manage)
        .await?;
    let websites = normalize(request.websites)?;
    let links = serde_json::to_string(&websites).map_err(ApiError::internal)?;
    let transaction = state.identity().database().begin().await?;
    repository_website::Entity::insert(repository_website::ActiveModel {
        repository_id: Set(repository.id),
        links: Set(links),
    })
    .on_conflict(
        OnConflict::column(repository_website::Column::RepositoryId)
            .update_column(repository_website::Column::Links)
            .to_owned(),
    )
    .exec(&transaction)
    .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.websites.update",
            Some(format!("{namespace}/{name}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(websites))
}

fn normalize(websites: Vec<Website>) -> Result<Vec<Website>, ApiError> {
    if websites.len() > 10 {
        return Err(ApiError::bad_request("Add at most 10 websites."));
    }
    let mut seen = HashSet::new();
    websites
        .into_iter()
        .map(|website| {
            let raw = website.url.trim();
            let url = url::Url::parse(raw).map_err(|_| {
                ApiError::bad_request("Enter an absolute HTTP or HTTPS website URL.")
            })?;
            if raw.len() > 2048
                || !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || raw.chars().any(char::is_control)
            {
                return Err(ApiError::bad_request(
                    "Website URLs must use HTTP or HTTPS without credentials.",
                ));
            }
            let label = website.label.trim();
            if label.chars().count() > 80 || label.chars().any(char::is_control) {
                return Err(ApiError::bad_request(
                    "Website labels must be at most 80 characters.",
                ));
            }
            let label = if label.is_empty() {
                url.host_str().unwrap_or_default()
            } else {
                label
            }
            .to_owned();
            let url = url.to_string();
            if !seen.insert(url.clone()) {
                return Err(ApiError::bad_request("Each website URL must be unique."));
            }
            Ok(Website { label, url })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_links_and_normalizes_empty_labels() {
        let links = normalize(vec![Website {
            label: " ".into(),
            url: " https://example.com ".into(),
        }])
        .unwrap();
        assert_eq!(links[0].label, "example.com");
        assert_eq!(links[0].url, "https://example.com/");
        for url in [
            "javascript:alert(1)",
            "//example.com",
            "https://user:pass@example.com",
            "file:///tmp/file",
        ] {
            assert!(
                normalize(vec![Website {
                    label: "Site".into(),
                    url: url.into()
                }])
                .is_err()
            );
        }
        assert!(normalize(vec![links[0].clone(), links[0].clone()]).is_err());
    }
}
