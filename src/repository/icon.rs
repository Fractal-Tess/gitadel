use std::{collections::HashSet, path::Path};

use axum::{
    Json,
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use axum_extra::extract::cookie::CookieJar;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sley::{GitObjectType, Repository as GitRepository};

use super::{
    Permission, RepositoryState,
    browser::read_git,
    image::{render_thumbnail, resolve_image_content},
};
use crate::{
    entity::{repository, repository_icon, repository_icon_candidate, repository_icon_scan},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

pub const MAX_ICON_REQUEST_BYTES: usize = 6 * 1024 * 1024;
const MAX_ICON_BYTES: usize = 4 * 1024 * 1024;
const MAX_SCAN_ENTRIES: usize = 20_000;
const MAX_SCAN_DEPTH: usize = 64;
const MAX_SCAN_READMES: usize = 32;
const MAX_SCAN_README_BYTES: usize = 256 * 1024;
const MAX_CANDIDATES: usize = 64;
const MAX_SCAN_IMAGES: usize = 256;
const DETECTOR_VERSION: i64 = 1;
pub const SOURCE_AUTOMATIC: &str = "automatic";
pub const SOURCE_SELECTED: &str = "selected";
pub const SOURCE_UPLOADED: &str = "uploaded";
pub const SOURCE_NONE: &str = "none";

#[derive(Clone, Debug, Serialize)]
pub struct IconCandidateResponse {
    path: String,
    oid: String,
    width: i32,
    height: i32,
    mime_type: String,
    reasons: Vec<String>,
    recommended: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct IconCandidatesResponse {
    mode: String,
    selected_path: Option<String>,
    selected_missing: bool,
    scan_status: String,
    candidates: Vec<IconCandidateResponse>,
    commit_oid: Option<String>,
    icon_updated_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Deserialize)]
pub struct IconSelectionRequest {
    mode: String,
    path: Option<String>,
    commit_oid: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateIconRequest {
    image_base64: String,
}

#[derive(Clone, Debug)]
struct DiscoveredCandidate {
    path: String,
    oid: String,
    bytes: Vec<u8>,
    width: i32,
    height: i32,
    mime_type: String,
    reasons: Vec<String>,
    score: i32,
}
#[derive(Debug)]
struct ScanFields {
    commit_oid: ActiveValue<Option<String>>,
    selected_path: ActiveValue<Option<String>>,
    selected_missing: ActiveValue<bool>,
    status: ActiveValue<String>,
    version: ActiveValue<i64>,
    detector_version: ActiveValue<i64>,
    error: ActiveValue<Option<String>>,
    updated_at: ActiveValue<chrono::DateTime<Utc>>,
}

#[derive(Debug)]
struct Discovery {
    commit_oid: String,
    candidates: Vec<DiscoveredCandidate>,
    partial: bool,
}

pub async fn public_icon(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, ApiError> {
    let repository = state.find(&namespace, &name).await?;
    let user_id = state
        .identity()
        .optional_user(&headers, &jar, SCOPE_READ)
        .await?
        .map(|account| account.id);
    state
        .authorize(&repository, user_id, Permission::Read)
        .await?;

    let icon = repository_icon::Entity::find_by_id(repository.id)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    image_response(icon.content)
}

pub async fn update_icon(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateIconRequest>,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let body = STANDARD
        .decode(request.image_base64)
        .map_err(|_| invalid_image())?;
    if body.len() > MAX_ICON_BYTES {
        return Err(ApiError::bad_request(
            "Repository icons cannot exceed 4 MiB.",
        ));
    }
    let rendered = thumbnail(body, "upload.png").await?;

    let transaction = state.identity().database().begin().await?;
    store_icon(&transaction, repository.id, rendered.content).await?;
    let now = Utc::now();
    let mut active = repository.into_active_model();
    active.icon_updated_at = Set(Some(now));
    active.icon_source = Set(Some(SOURCE_UPLOADED.to_owned()));
    active.updated_at = Set(now);
    active.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.icon.update",
            Some(format!("{namespace}/{name}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_icon(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let transaction = state.identity().database().begin().await?;
    repository_icon::Entity::delete_by_id(repository.id)
        .exec(&transaction)
        .await?;
    let now = Utc::now();
    let mut active = repository.into_active_model();
    active.icon_updated_at = Set(None);
    active.icon_source = Set(Some(SOURCE_NONE.to_owned()));
    active.updated_at = Set(now);
    active.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.icon.delete",
            Some(format!("{namespace}/{name}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Returns the current candidate set. Repository read permission is required,
/// including for private repositories; candidate blobs are never public.
pub async fn icon_candidates(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<IconCandidatesResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    Ok(Json(load_candidates(&state, &repository).await?))
}

pub async fn select_icon(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<IconSelectionRequest>,
) -> Result<Json<IconCandidatesResponse>, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let mode = normalize_mode(&request.mode)?;
    let database = state.identity().database();
    let candidates = repository_icon_candidate::Entity::find()
        .filter(repository_icon_candidate::Column::RepositoryId.eq(repository.id))
        .all(database)
        .await?;
    let candidate = match mode {
        SOURCE_SELECTED => {
            let path = normalize_candidate_path(request.path.as_deref().ok_or_else(|| {
                ApiError::bad_request("A path is required when selecting a repository icon.")
            })?)?;
            Some(
                candidates
                    .iter()
                    .find(|candidate| candidate.path == path)
                    .cloned()
                    .ok_or_else(|| {
                        ApiError::bad_request("That icon is not a repository candidate.")
                    })?,
            )
        }
        SOURCE_AUTOMATIC => candidates
            .iter()
            .find(|candidate| candidate.recommended)
            .or_else(|| candidates.first())
            .cloned(),
        _ => None,
    };
    let selected_path = (mode == SOURCE_SELECTED)
        .then(|| candidate.as_ref().map(|candidate| candidate.path.clone()))
        .flatten();
    let image = if let Some(candidate) = candidate.as_ref() {
        if request
            .commit_oid
            .as_deref()
            .is_some_and(|commit| commit != candidate.commit_oid)
        {
            return Err(ApiError::bad_request("Icon candidates are out of date."));
        }
        let path = state.repository_path(&repository);
        let revision = candidate.commit_oid.clone();
        let requested_path = candidate.path.clone();
        let bytes = read_git(path, move |git| {
            let resolved = git.resolve_path(&revision, &requested_path)?;
            if resolved.object_type != GitObjectType::Blob {
                return Err(sley::GitError::InvalidPath(requested_path));
            }
            git.blobs().read(resolved.oid)
        })
        .await?;
        let bytes = resolve_image_content(&state, &repository, bytes).await?;
        Some(thumbnail(bytes, &candidate.path).await?)
    } else {
        None
    };

    let transaction = database.begin().await?;
    let now = Utc::now();
    let scan = repository_icon_scan::Entity::find_by_id(repository.id)
        .one(&transaction)
        .await?;
    if let Some(candidate) = candidate.as_ref() {
        let current =
            repository_icon_candidate::Entity::find_by_id((repository.id, candidate.path.clone()))
                .one(&transaction)
                .await?;
        if !current.is_some_and(|current| {
            current.commit_oid == candidate.commit_oid && current.oid == candidate.oid
        }) {
            transaction.rollback().await?;
            return Err(ApiError::conflict(
                "Icon candidates changed. Refresh and select again.",
            ));
        }
    }
    let version = scan
        .as_ref()
        .map_or(1, |scan| scan.version.saturating_add(1));
    upsert_scan(
        &transaction,
        repository.id,
        ScanFields {
            commit_oid: Set(scan.as_ref().and_then(|scan| scan.commit_oid.clone())),
            selected_path: Set(selected_path),
            selected_missing: Set(false),
            status: Set(scan.as_ref().map_or_else(
                || {
                    if repository.default_branch.is_none() {
                        "complete"
                    } else {
                        "pending"
                    }
                    .to_owned()
                },
                |scan| scan.status.clone(),
            )),
            version: Set(version),
            detector_version: Set(scan
                .as_ref()
                .map_or(DETECTOR_VERSION, |scan| scan.detector_version)),
            error: Set(None),
            updated_at: Set(now),
        },
    )
    .await?;
    let mut repository_active = repository.clone().into_active_model();
    repository_active.icon_source = Set(Some(mode.to_owned()));
    repository_active.updated_at = Set(now);
    repository_active.update(&transaction).await?;
    if let Some(image) = image {
        store_icon(&transaction, repository.id, image.content).await?;
        let mut active = repository.clone().into_active_model();
        active.icon_updated_at = Set(Some(now));
        active.update(&transaction).await?;
    } else if mode == SOURCE_NONE || mode == SOURCE_AUTOMATIC {
        repository_icon::Entity::delete_by_id(repository.id)
            .exec(&transaction)
            .await?;
        let mut active = repository.clone().into_active_model();
        active.icon_updated_at = Set(None);
        active.update(&transaction).await?;
    }
    transaction.commit().await?;
    state.queue_repository_analysis(repository.id).await;
    Ok(Json(load_candidates(&state, &repository).await?))
}

/// Scans the complete default-branch tree. This is intentionally called by the
/// analysis worker rather than by an HTTP request, so a new push cannot make an
/// unauthenticated repository blob observable.
pub(super) async fn detect_icon(
    state: &RepositoryState,
    repository: &repository::Model,
) -> Result<(), ApiError> {
    let Some(revision) = repository.default_branch.as_deref() else {
        return Ok(());
    };
    let revision = revision.to_owned();
    let repository_path = state.repository_path(repository);
    let tip = match read_git(repository_path.clone(), {
        let revision = revision.clone();
        move |git| {
            git.peel_to_commit_oid(git.rev_parse(&revision)?)
                .map(|oid| oid.to_hex())
        }
    })
    .await
    {
        Ok(tip) => tip,
        Err(error) => {
            let version = begin_scan(state, repository.id).await?;
            finish_scan(
                state,
                repository,
                version,
                None,
                Vec::new(),
                Some(error.to_string()),
            )
            .await?;
            return Err(error);
        }
    };
    let previous_scan = repository_icon_scan::Entity::find_by_id(repository.id)
        .one(state.identity().database())
        .await?;
    if previous_scan.as_ref().is_some_and(|scan| {
        scan.commit_oid.as_deref() == Some(tip.as_str())
            && scan.status == "complete"
            && scan.detector_version == DETECTOR_VERSION
    }) {
        return Ok(());
    }
    let selected_path = if source_mode(repository.icon_source.as_deref()) == SOURCE_SELECTED {
        previous_scan.and_then(|scan| scan.selected_path)
    } else {
        None
    };
    let version = begin_scan(state, repository.id).await?;
    let repository_name = repository.name.clone();
    let selected_for_discovery = selected_path.clone();
    let discovery = read_git(repository_path, move |git| {
        discover(
            git,
            &revision,
            &repository_name,
            selected_for_discovery.as_deref(),
        )
    })
    .await;
    let mut discovery = match discovery {
        Ok(discovery) => discovery,
        Err(error) => {
            finish_scan(
                state,
                repository,
                version,
                None,
                Vec::new(),
                Some(error.to_string()),
            )
            .await?;
            return Err(error);
        }
    };
    // A push may have advanced the branch while the tree was being opened.
    if discovery.commit_oid != tip {
        return Ok(());
    }
    let mut resolve_error = discovery
        .partial
        .then(|| "icon scan was bounded".to_owned());
    for candidate in &mut discovery.candidates {
        let candidate_path = candidate.path.clone();
        let revision = discovery.commit_oid.clone();
        let repository_path = state.repository_path(repository);
        let raw = read_git(repository_path, move |git| {
            let resolved = git.resolve_path(&revision, &candidate_path)?;
            if resolved.object_type != GitObjectType::Blob {
                return Err(sley::GitError::InvalidPath(candidate_path));
            }
            let Some((object_type, size)) = git.read_object_header(&resolved.oid)? else {
                return Err(sley::GitError::InvalidObject(format!(
                    "missing icon object {}",
                    resolved.oid
                )));
            };
            if object_type != GitObjectType::Blob || size > MAX_ICON_BYTES as u64 {
                return Err(sley::GitError::InvalidObject(format!(
                    "invalid icon object {}",
                    resolved.oid
                )));
            }
            git.blobs().read(resolved.oid)
        })
        .await;
        let original = match raw {
            Ok(bytes) => bytes,
            Err(error) => {
                resolve_error.get_or_insert(error.to_string());
                continue;
            }
        };
        match resolve_image_content(state, repository, original).await {
            Ok(bytes) => match thumbnail(bytes, &candidate.path).await {
                Ok(rendered) => {
                    candidate.bytes = rendered.content;
                    candidate.width = i32::try_from(rendered.width).unwrap_or(i32::MAX);
                    candidate.height = i32::try_from(rendered.height).unwrap_or(i32::MAX);
                    let (score, reasons) = score_path(
                        &candidate.path,
                        &repository.name,
                        rendered.width,
                        rendered.height,
                        candidate
                            .reasons
                            .iter()
                            .any(|reason| reason == "README reference"),
                    );
                    candidate.score = score;
                    candidate.reasons = reasons;
                    candidate.mime_type = rendered.mime_type;
                }
                Err(error) => {
                    resolve_error.get_or_insert(error.to_string());
                }
            },
            Err(error) => {
                resolve_error.get_or_insert(error.to_string());
            }
        }
    }
    discovery.candidates.retain(|candidate| {
        !candidate.bytes.is_empty()
            && (candidate.score >= 0 || selected_path.as_deref() == Some(candidate.path.as_str()))
    });
    discovery.candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    if discovery.candidates.len() > MAX_CANDIDATES {
        if let Some(index) = discovery
            .candidates
            .iter()
            .position(|candidate| selected_path.as_deref() == Some(candidate.path.as_str()))
            .filter(|index| *index >= MAX_CANDIDATES)
        {
            let selected = discovery.candidates.remove(index);
            discovery.candidates.truncate(MAX_CANDIDATES - 1);
            discovery.candidates.push(selected);
        } else {
            discovery.candidates.truncate(MAX_CANDIDATES);
        }
    }
    let latest_revision = repository.default_branch.clone().unwrap_or_default();
    let latest_tip = read_git(state.repository_path(repository), move |git| {
        git.peel_to_commit_oid(git.rev_parse(&latest_revision)?)
            .map(|oid| oid.to_hex())
    })
    .await?;
    if latest_tip != discovery.commit_oid {
        return Ok(());
    }
    let scan_error = resolve_error;
    finish_scan(
        state,
        repository,
        version,
        Some(discovery.commit_oid),
        discovery.candidates,
        scan_error,
    )
    .await
}

async fn begin_scan(state: &RepositoryState, repository_id: uuid::Uuid) -> Result<i64, ApiError> {
    let transaction = state.identity().database().begin().await?;
    let existing = repository_icon_scan::Entity::find_by_id(repository_id)
        .one(&transaction)
        .await?;
    let version = existing
        .as_ref()
        .map_or(1, |scan| scan.version.saturating_add(1));
    let now = Utc::now();
    upsert_scan(
        &transaction,
        repository_id,
        ScanFields {
            commit_oid: Set(existing.as_ref().and_then(|scan| scan.commit_oid.clone())),
            selected_path: Set(existing
                .as_ref()
                .and_then(|scan| scan.selected_path.clone())),
            selected_missing: Set(existing.as_ref().is_some_and(|scan| scan.selected_missing)),
            status: Set("pending".to_owned()),
            version: Set(version),
            detector_version: Set(DETECTOR_VERSION),
            error: Set(None),
            updated_at: Set(now),
        },
    )
    .await?;
    transaction.commit().await?;
    Ok(version)
}

async fn finish_scan(
    state: &RepositoryState,
    repository: &repository::Model,
    version: i64,
    commit_oid: Option<String>,
    mut candidates: Vec<DiscoveredCandidate>,
    error: Option<String>,
) -> Result<(), ApiError> {
    let transaction = state.identity().database().begin().await?;
    let Some(scan) = repository_icon_scan::Entity::find_by_id(repository.id)
        .one(&transaction)
        .await?
    else {
        transaction.rollback().await?;
        return Ok(());
    };
    let Some(current_repository) = repository::Entity::find_by_id(repository.id)
        .one(&transaction)
        .await?
    else {
        transaction.rollback().await?;
        return Ok(());
    };
    // A selection, newer scan, or default-branch change won the race.
    if scan.version != version || current_repository.default_branch != repository.default_branch {
        transaction.rollback().await?;
        return Ok(());
    }

    let now = Utc::now();
    if commit_oid.is_none() && error.is_some() {
        let mut active: repository_icon_scan::ActiveModel = scan.into();
        active.status = Set("failed".to_owned());
        active.error = Set(error);
        active.updated_at = Set(now);
        active.update(&transaction).await?;
        transaction.commit().await?;
        return Ok(());
    }
    repository_icon_candidate::Entity::delete_many()
        .filter(repository_icon_candidate::Column::RepositoryId.eq(repository.id))
        .exec(&transaction)
        .await?;
    let mut selected_missing = false;
    let mode = source_mode(current_repository.icon_source.as_deref());
    let selected_path = if mode == SOURCE_SELECTED {
        scan.selected_path.clone()
    } else {
        None
    };
    let selected = selected_path.as_deref().and_then(|path| {
        candidates
            .iter()
            .position(|candidate| candidate.path == path)
    });
    let effective = match mode {
        SOURCE_UPLOADED | SOURCE_NONE => None,
        SOURCE_SELECTED => {
            selected_missing = selected.is_none()
                && selected_path.is_some()
                && (error.is_none() || scan.selected_missing);
            selected
        }
        _ if error.is_none() || current_repository.icon_updated_at.is_none() => {
            (!candidates.is_empty()).then_some(0)
        }
        _ => None,
    };
    let image = effective.map(|index| std::mem::take(&mut candidates[index].bytes));
    for (index, candidate) in candidates.into_iter().enumerate() {
        repository_icon_candidate::ActiveModel {
            repository_id: Set(repository.id),
            path: Set(candidate.path),
            commit_oid: Set(commit_oid.clone().unwrap_or_default()),
            oid: Set(candidate.oid),
            width: Set(candidate.width),
            height: Set(candidate.height),
            mime_type: Set(candidate.mime_type),
            reasons_json: Set(
                serde_json::to_string(&candidate.reasons).map_err(ApiError::internal)?
            ),
            recommended: Set(index == 0),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&transaction)
        .await?;
    }

    if let Some(content) = image {
        store_icon(&transaction, repository.id, content).await?;
        let mut active = current_repository.clone().into_active_model();
        active.icon_updated_at = Set(Some(now));
        active.icon_source = Set(Some(if mode == SOURCE_SELECTED {
            SOURCE_SELECTED.to_owned()
        } else {
            SOURCE_AUTOMATIC.to_owned()
        }));
        active.updated_at = Set(now);
        active.update(&transaction).await?;
    } else if mode == SOURCE_AUTOMATIC && error.is_none() {
        repository_icon::Entity::delete_by_id(repository.id)
            .exec(&transaction)
            .await?;
        let mut active = current_repository.clone().into_active_model();
        active.icon_updated_at = Set(None);
        active.updated_at = Set(now);
        active.update(&transaction).await?;
    }

    let status = if error.is_some() {
        "partial"
    } else {
        "complete"
    };
    let mut scan_active: repository_icon_scan::ActiveModel = scan.into();
    scan_active.commit_oid = Set(commit_oid);
    scan_active.selected_path = Set(selected_path);
    scan_active.selected_missing = Set(selected_missing);
    scan_active.status = Set(status.to_owned());
    scan_active.error = Set(error);
    scan_active.updated_at = Set(now);
    scan_active.update(&transaction).await?;
    transaction.commit().await?;
    Ok(())
}

async fn load_candidates(
    state: &RepositoryState,
    repository: &repository::Model,
) -> Result<IconCandidatesResponse, ApiError> {
    let transaction = state.identity().database().begin().await?;
    let repository = repository::Entity::find_by_id(repository.id)
        .one(&transaction)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let scan = repository_icon_scan::Entity::find_by_id(repository.id)
        .one(&transaction)
        .await?;
    let rows = repository_icon_candidate::Entity::find()
        .filter(repository_icon_candidate::Column::RepositoryId.eq(repository.id))
        .all(&transaction)
        .await?;
    transaction.commit().await?;
    let mut candidates = Vec::with_capacity(rows.len());
    for row in rows {
        let reasons = serde_json::from_str(&row.reasons_json).unwrap_or_default();
        candidates.push(IconCandidateResponse {
            path: row.path,
            oid: row.oid,
            width: row.width,
            height: row.height,
            mime_type: row.mime_type,
            reasons,
            recommended: row.recommended,
        });
    }
    candidates.sort_by(|left, right| {
        right
            .recommended
            .cmp(&left.recommended)
            .then_with(|| left.path.cmp(&right.path))
    });
    let (selected_path, selected_missing, mut scan_status, commit_oid) = scan.map_or_else(
        || {
            (
                None,
                false,
                if repository.default_branch.is_none() {
                    "complete"
                } else {
                    "pending"
                }
                .to_owned(),
                None,
            )
        },
        |scan| {
            (
                scan.selected_path,
                scan.selected_missing,
                scan.status,
                scan.commit_oid,
            )
        },
    );
    if let Some(revision) = repository.default_branch.clone() {
        let current_tip = read_git(state.repository_path(&repository), move |git| {
            git.peel_to_commit_oid(git.rev_parse(&revision)?)
                .map(|oid| oid.to_hex())
        })
        .await;
        match current_tip {
            Ok(tip) if scan_status != "failed" && commit_oid.as_deref() != Some(tip.as_str()) => {
                scan_status = "pending".to_owned();
                state.queue_repository_analysis(repository.id).await;
            }
            Ok(_) => {}
            Err(_) => scan_status = "failed".to_owned(),
        }
    } else {
        scan_status = "complete".to_owned();
    }
    let mode = source_mode(repository.icon_source.as_deref());
    Ok(IconCandidatesResponse {
        mode: mode.to_owned(),
        selected_path: if mode == SOURCE_SELECTED {
            selected_path
        } else {
            None
        },
        selected_missing: mode == SOURCE_SELECTED && selected_missing,
        scan_status,
        candidates,
        commit_oid,
        icon_updated_at: repository.icon_updated_at,
    })
}

fn discover(
    git: &GitRepository,
    revision: &str,
    repository_name: &str,
    selected_path: Option<&str>,
) -> sley::Result<Discovery> {
    let commit_oid = git.peel_to_commit_oid(git.rev_parse(revision)?)?;
    let tree_oid = git.read_commit(&commit_oid)?.tree;
    let mut files = Vec::new();
    let mut readmes = Vec::new();
    let mut visited = 0;
    let mut partial = false;
    collect_files(
        git,
        tree_oid,
        "",
        &mut files,
        &mut readmes,
        0,
        &mut visited,
        &mut partial,
    )?;
    let commit_hex = commit_oid.to_hex();
    if let Some(path) = selected_path
        && !files.iter().any(|(candidate, _)| candidate == path)
    {
        match git.resolve_path(&commit_hex, path) {
            Ok(resolved) if matches!(resolved.mode, Some(0o100644 | 0o100755)) => {
                match git.read_object_header(&resolved.oid)? {
                    Some((GitObjectType::Blob, size)) if size <= MAX_ICON_BYTES as u64 => {
                        files.push((path.to_owned(), resolved.oid));
                    }
                    _ => partial = true,
                }
            }
            Ok(_) | Err(sley::GitError::NotFound(_)) => {}
            Err(_) => partial = true,
        }
    }
    let readme_refs = readmes
        .iter()
        .flat_map(|(path, content)| extract_readme_refs(path, content))
        .collect::<HashSet<_>>();
    let mut candidates = Vec::new();
    for (path, oid) in files {
        let Some((mime_type, _)) = image_mime(&path) else {
            continue;
        };
        let readme = readme_refs.contains(&path);
        let (score, reasons) = score_path(&path, repository_name, 512, 512, readme);
        if score < 0 && selected_path != Some(path.as_str()) {
            continue;
        }
        candidates.push(DiscoveredCandidate {
            path,
            oid: oid.to_hex(),
            bytes: Vec::new(),
            width: 0,
            height: 0,
            mime_type,
            reasons,
            score,
        });
    }
    candidates.sort_by(|left, right| {
        (selected_path == Some(right.path.as_str()))
            .cmp(&(selected_path == Some(left.path.as_str())))
            .then_with(|| right.score.cmp(&left.score))
            .then_with(|| {
                left.path
                    .to_ascii_lowercase()
                    .cmp(&right.path.to_ascii_lowercase())
            })
            .then_with(|| left.path.cmp(&right.path))
    });
    // Keep aliases only when they have independent branding/README evidence;
    // this removes copied framework assets while retaining useful path choices.
    let mut seen_oids = HashSet::new();
    candidates.retain(|candidate| {
        if seen_oids.insert(candidate.oid.clone()) || selected_path == Some(candidate.path.as_str())
        {
            return true;
        }
        candidate
            .reasons
            .iter()
            .any(|reason| reason == "README reference" || reason == "explicit branding")
    });
    let partial = partial || candidates.len() > MAX_SCAN_IMAGES;
    candidates.truncate(MAX_SCAN_IMAGES);
    Ok(Discovery {
        commit_oid: commit_hex,
        candidates,
        partial,
    })
}

fn collect_files(
    git: &GitRepository,
    tree_oid: sley::ObjectId,
    prefix: &str,
    files: &mut Vec<(String, sley::ObjectId)>,
    readmes: &mut Vec<(String, Vec<u8>)>,
    depth: usize,
    visited: &mut usize,
    partial: &mut bool,
) -> sley::Result<()> {
    if depth > MAX_SCAN_DEPTH || *visited >= MAX_SCAN_ENTRIES {
        *partial = true;
        return Ok(());
    }
    for entry in git.read_tree(&tree_oid)?.entries {
        *visited += 1;
        if *visited > MAX_SCAN_ENTRIES {
            *partial = true;
            break;
        }
        let name = String::from_utf8_lossy(entry.name.as_bytes()).into_owned();
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if entry.is_gitlink() || entry.mode == 0o120000 {
            continue;
        }
        if entry.is_tree() {
            collect_files(
                git,
                entry.oid,
                &path,
                files,
                readmes,
                depth + 1,
                visited,
                partial,
            )?;
            continue;
        }
        let lower = path.to_ascii_lowercase();
        let header = git.read_object_header(&entry.oid).ok().flatten();
        let Some((object_type, size)) = header else {
            *partial = true;
            continue;
        };
        if object_type != GitObjectType::Blob {
            continue;
        }
        if is_readme(&lower) {
            if readmes.len() >= MAX_SCAN_READMES {
                *partial = true;
            } else if size <= MAX_SCAN_README_BYTES as u64
                && let Ok(content) = git.blobs().read(entry.oid)
            {
                readmes.push((path.clone(), content));
            }
        }
        if image_mime(&path).is_some() {
            if size <= MAX_ICON_BYTES as u64 {
                files.push((path, entry.oid));
            } else {
                *partial = true;
            }
        }
    }
    Ok(())
}

fn score_path(
    path: &str,
    repository_name: &str,
    width: u32,
    height: u32,
    readme: bool,
) -> (i32, Vec<String>) {
    let lower = path.to_ascii_lowercase();
    let basename = Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut score = 0;
    let mut reasons = Vec::new();
    let repo = normalize_token(repository_name);
    let base = normalize_token(&basename);
    let explicit = ["logo", "logomark", "brand", "favicon", "appicon", "icon"]
        .iter()
        .any(|name| base.contains(name));
    if explicit || (!repo.is_empty() && base.contains(&repo)) {
        score += 100;
        reasons.push("explicit branding".to_owned());
    }
    if readme {
        score += 90;
        reasons.push("README reference".to_owned());
    }
    if path.rsplit('/').count() == 1 {
        score += 25;
        reasons.push("repository root".to_owned());
    } else if ["static", "public", "assets", "frontend", "web", "docs"]
        .iter()
        .any(|directory| lower.split('/').any(|part| part == *directory))
    {
        score += 15;
        reasons.push("web asset path".to_owned());
    }
    let edge = width.max(height);
    if (16..=2048).contains(&edge) {
        score += 15;
        reasons.push("suitable dimensions".to_owned());
    }
    let ratio = width.max(height) as f32 / width.min(height).max(1) as f32;
    if ratio <= 1.35 {
        score += 15;
        reasons.push("near-square".to_owned());
    } else if ratio > 4.0 {
        score -= 35;
        reasons.push("wide banner".to_owned());
    }
    for demotion in [
        "screenshot",
        "screenshots",
        "badge",
        "badges",
        "vendor",
        "node_modules",
        "test",
        "tests",
        "fixtures",
        "storybook",
        "ui",
        "icons",
    ] {
        if lower.split('/').any(|part| part == demotion) || basename.contains(demotion) {
            score -= 80;
            reasons.push(format!("demoted {demotion}"));
        }
    }
    (score, reasons)
}

fn extract_readme_refs(readme_path: &str, content: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(content);
    let base = Path::new(readme_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let mut refs = Vec::new();
    let mut rest = text.as_ref();
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else { break };
        let raw = after[..end].split_whitespace().next().unwrap_or_default();
        if let Some(path) = normalize_local_ref(base, raw) {
            refs.push(path);
        }
        rest = &after[end + 1..];
    }
    let mut rest = text.as_ref();
    while let Some(start) = rest.to_ascii_lowercase().find("src=") {
        let after = &rest[start + 4..];
        let raw = after.trim_start_matches(&[' ', '\t', '"', '\''][..]);
        let raw = raw
            .split(&['"', '\'', ' ', '>'][..])
            .next()
            .unwrap_or_default();
        if let Some(path) = normalize_local_ref(base, raw) {
            refs.push(path);
        }
        rest = &after[1.min(after.len())..];
    }
    refs
}

fn normalize_local_ref(base: &Path, raw: &str) -> Option<String> {
    let raw = raw.split(&['#', '?'][..]).next()?.trim();
    if raw.is_empty() || raw.starts_with("/") || raw.contains("://") {
        return None;
    }
    let path = base.join(raw.trim_start_matches("./"));
    let mut clean = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(value) => clean.push(value.to_string_lossy().into_owned()),
            std::path::Component::ParentDir => {
                clean.pop();
            }
            _ => {}
        }
    }
    let path = clean.join("/");
    image_mime(&path).is_some().then_some(path)
}
fn image_mime(path: &str) -> Option<(String, &'static str)> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => return None,
    };
    Some((mime.to_owned(), mime))
}

fn normalize_token(value: &str) -> String {
    value
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}
fn is_readme(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    filename == "readme" || filename.starts_with("readme.")
}

fn source_mode(source: Option<&str>) -> &'static str {
    match source {
        Some(SOURCE_UPLOADED) => SOURCE_UPLOADED,
        Some(SOURCE_SELECTED) => SOURCE_SELECTED,
        Some(SOURCE_NONE) => SOURCE_NONE,
        _ => SOURCE_AUTOMATIC,
    }
}

fn normalize_mode(mode: &str) -> Result<&'static str, ApiError> {
    match mode {
        SOURCE_AUTOMATIC => Ok(SOURCE_AUTOMATIC),
        SOURCE_SELECTED => Ok(SOURCE_SELECTED),
        SOURCE_NONE => Ok(SOURCE_NONE),
        _ => Err(ApiError::bad_request("Invalid repository icon mode.")),
    }
}

fn normalize_candidate_path(path: &str) -> Result<String, ApiError> {
    let path = path.trim();
    if path.is_empty() || path.starts_with('/') || path.contains("..") || path.len() > 1024 {
        return Err(ApiError::bad_request("Invalid repository icon path."));
    }
    Ok(path.to_owned())
}

async fn readable_repository(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
) -> Result<repository::Model, ApiError> {
    let repository = state.find(namespace, name).await?;
    let user_id = state
        .identity()
        .optional_user(headers, jar, SCOPE_READ)
        .await?
        .map(|account| account.id);
    state
        .authorize(&repository, user_id, Permission::Read)
        .await?;
    Ok(repository)
}

async fn upsert_scan(
    connection: &impl sea_orm::ConnectionTrait,
    repository_id: uuid::Uuid,
    fields: ScanFields,
) -> Result<(), ApiError> {
    match repository_icon_scan::Entity::find_by_id(repository_id)
        .one(connection)
        .await?
    {
        Some(scan) => {
            let mut active: repository_icon_scan::ActiveModel = scan.into();
            active.commit_oid = fields.commit_oid;
            active.selected_path = fields.selected_path;
            active.selected_missing = fields.selected_missing;
            active.status = fields.status;
            active.version = fields.version;
            active.detector_version = fields.detector_version;
            active.error = fields.error;
            active.updated_at = fields.updated_at;
            active.update(connection).await?;
        }
        None => {
            repository_icon_scan::ActiveModel {
                repository_id: Set(repository_id),
                commit_oid: fields.commit_oid,
                selected_path: fields.selected_path,
                selected_missing: fields.selected_missing,
                status: fields.status,
                version: fields.version,
                detector_version: fields.detector_version,
                error: fields.error,
                updated_at: fields.updated_at,
            }
            .insert(connection)
            .await?;
        }
    }
    Ok(())
}
async fn thumbnail(bytes: Vec<u8>, path: &str) -> Result<super::image::RenderedImage, ApiError> {
    render_thumbnail(bytes, path.to_owned()).await
}

fn image_response(content: Vec<u8>) -> Result<Response, ApiError> {
    let mut response = Response::new(Body::from(content));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

async fn store_icon<C>(
    connection: &C,
    repository_id: uuid::Uuid,
    content: Vec<u8>,
) -> Result<(), ApiError>
where
    C: sea_orm::ConnectionTrait,
{
    match repository_icon::Entity::find_by_id(repository_id)
        .one(connection)
        .await?
    {
        Some(icon) => {
            let mut active: repository_icon::ActiveModel = icon.into();
            active.content = Set(content);
            active.update(connection).await?;
        }
        None => {
            repository_icon::ActiveModel {
                repository_id: Set(repository_id),
                content: Set(content),
            }
            .insert(connection)
            .await?;
        }
    }
    Ok(())
}

fn invalid_image() -> ApiError {
    ApiError::bad_request("The repository icon is not a supported image.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branding_and_readme_outscore_screenshots() {
        let (logo, _) = score_path("assets/gitadel-favicon.svg", "gitadel", 128, 128, true);
        let (shot, _) = score_path("docs/screenshots/home.png", "gitadel", 128, 128, false);
        assert!(logo > shot);
    }

    #[test]
    fn readme_refs_are_local_and_normalized() {
        let refs = extract_readme_refs(
            "docs/README.md",
            br#"![logo](../frontend/static/favicon.png) <img src="../assets/icon.svg"> [x](https://example.com/a.png)"#,
        );
        assert_eq!(refs, vec!["frontend/static/favicon.png", "assets/icon.svg"]);
    }
}
