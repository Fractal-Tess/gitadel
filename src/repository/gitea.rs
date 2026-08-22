use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use sea_orm::{EntityTrait, QueryOrder};
use serde::{Deserialize, Serialize};
use sley::ReferenceTarget;

use super::{Permission, RepositoryState, browser};
use crate::{entity::repository, identity::ApiError, identity::SCOPE_READ};

#[derive(Default, Deserialize)]
pub struct GiteaPagination {
    page: Option<usize>,
    limit: Option<usize>,
}

impl GiteaPagination {
    fn bounds(&self, total: usize) -> (usize, usize) {
        let page = self.page.unwrap_or(1).max(1);
        let limit = self.limit.unwrap_or(30).clamp(1, 50);
        let start = page.saturating_sub(1).saturating_mul(limit).min(total);
        (start, start.saturating_add(limit).min(total))
    }
}

#[derive(Serialize)]
pub struct GiteaOwnerResponse {
    login: String,
    username: String,
}

#[derive(Serialize)]
pub struct GiteaRepositoryResponse {
    id: String,
    owner: GiteaOwnerResponse,
    name: String,
    full_name: String,
    description: Option<String>,
    private: bool,
    html_url: String,
    clone_url: String,
    ssh_url: String,
    default_branch: String,
    archived: bool,
}

pub async fn list_user_repositories(
    State(state): State<RepositoryState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(pagination): Query<GiteaPagination>,
) -> Result<Json<Vec<GiteaRepositoryResponse>>, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_READ)
        .await?;
    let repositories = repository::Entity::find()
        .order_by_desc(repository::Column::UpdatedAt)
        .all(state.identity().database())
        .await?;
    let mut accessible = Vec::with_capacity(repositories.len());
    for repository in repositories {
        if state
            .can_access(&repository, Some(actor.user.id), Permission::Read)
            .await?
        {
            accessible.push(repository);
        }
    }
    let (start, end) = pagination.bounds(accessible.len());
    Ok(Json(
        accessible[start..end]
            .iter()
            .cloned()
            .map(|repository| repository_response(repository, &state))
            .collect(),
    ))
}

#[derive(Serialize)]
pub struct GiteaBranchResponse {
    name: String,
    commit: GiteaBranchCommitResponse,
    protected: bool,
}

#[derive(Serialize)]
pub struct GiteaBranchCommitResponse {
    id: String,
}

pub async fn list_branches(
    State(state): State<RepositoryState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(pagination): Query<GiteaPagination>,
) -> Result<Json<Vec<GiteaBranchResponse>>, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_READ)
        .await?;
    let repository = state.find(&namespace, &name).await?;
    state
        .authorize(&repository, Some(actor.user.id), Permission::Read)
        .await?;
    let path = state.repository_path(&repository);
    let branches = browser::read_git(path, |git| {
        let mut branches = git
            .references()
            .list_refs_with_prefix("refs/heads/")?
            .into_iter()
            .filter_map(|reference| {
                let ReferenceTarget::Direct(oid) = reference.target else {
                    return None;
                };
                Some(GiteaBranchResponse {
                    name: reference.name.strip_prefix("refs/heads/")?.to_owned(),
                    commit: GiteaBranchCommitResponse { id: oid.to_hex() },
                    protected: false,
                })
            })
            .collect::<Vec<_>>();
        branches.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        Ok(branches)
    })
    .await?;
    let (start, end) = pagination.bounds(branches.len());
    Ok(Json(
        branches.into_iter().skip(start).take(end - start).collect(),
    ))
}

fn repository_response(
    repository: repository::Model,
    state: &RepositoryState,
) -> GiteaRepositoryResponse {
    let full_name = format!("{}/{}", repository.namespace, repository.name);
    let clone_url = state.http_clone_url(&repository);
    let html_url = clone_url
        .strip_suffix(".git")
        .unwrap_or(&clone_url)
        .to_owned();
    let owner = GiteaOwnerResponse {
        login: repository.namespace.clone(),
        username: repository.namespace.clone(),
    };
    let ssh_url = state.ssh_clone_url(&repository);
    GiteaRepositoryResponse {
        id: repository.id.to_string(),
        owner,
        name: repository.name,
        full_name,
        description: repository.description,
        private: repository.visibility == "private",
        html_url,
        clone_url,
        ssh_url,
        default_branch: repository.default_branch,
        archived: repository.archived_at.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_should_clamp_to_gitea_page_size() {
        let pagination = GiteaPagination {
            page: Some(2),
            limit: Some(100),
        };
        assert_eq!(pagination.bounds(120), (50, 100));
    }
}
