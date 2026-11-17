use std::path::Path;

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use sley::{ObjectId, Repository as GitRepository};
use utoipa::ToSchema;

use super::{
    Permission, RepositoryState,
    protection::{RefCheck, load_guard},
};
use crate::identity::{ApiError, SCOPE_WRITE, commit_emails::commit_author};

const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_FILE_REQUEST_BYTES: usize = MAX_FILE_BYTES * 6 + 16 * 1024;

#[derive(Deserialize, ToSchema)]
pub struct CreateFileRequest {
    pub branch: String,
    pub expected_commit: Option<String>,
    pub path: String,
    pub content: String,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct CreateFileResponse {
    pub path: String,
    pub branch: String,
    pub commit: String,
}

pub(super) use gitadel_repositories::files::FileWrite;

#[utoipa::path(
    post, path = "/repositories/{namespace}/{name}/files",
    operation_id = "post_repositories_namespace_name_files",
    summary = "Commit a new file",
    tag = "Repository content",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = CreateFileResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_file(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateFileRequest>,
) -> Result<Json<CreateFileResponse>, ApiError> {
    write_file_request(
        state,
        namespace,
        name,
        headers,
        jar,
        request,
        FileWrite::Create,
    )
    .await
}

/// Replaces an existing file's contents on a branch with a new commit, as an
/// edit made in the browser.
#[utoipa::path(
    put, path = "/repositories/{namespace}/{name}/files",
    operation_id = "put_repositories_namespace_name_files",
    summary = "Commit a change to an existing file",
    tag = "Repository content",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = CreateFileResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_file(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateFileRequest>,
) -> Result<Json<CreateFileResponse>, ApiError> {
    if request.expected_commit.is_none() {
        return Err(ApiError::bad_request(
            "Editing a file needs the commit it was opened at.",
        ));
    }
    write_file_request(
        state,
        namespace,
        name,
        headers,
        jar,
        request,
        FileWrite::Update,
    )
    .await
}

async fn write_file_request(
    state: RepositoryState,
    namespace: String,
    name: String,
    headers: HeaderMap,
    jar: CookieJar,
    request: CreateFileRequest,
    write: FileWrite,
) -> Result<Json<CreateFileResponse>, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Write,
            SCOPE_WRITE,
        )
        .await?;
    validate_request(&request)?;
    let branch = request
        .branch
        .trim()
        .strip_prefix("refs/heads/")
        .unwrap_or(request.branch.trim())
        .to_owned();
    // commit_file only writes when the branch still matches expected_commit,
    // and its commit always has that tip as parent, so the update is a
    // fast-forward and only creation, push restrictions, and deletion matter.
    let guard = load_guard(&state, &repository, Some(actor.user.id)).await?;
    if let RefCheck::Rejected(message) = guard.check(
        &format!("refs/heads/{branch}"),
        request.expected_commit.is_none(),
        false,
    ) {
        return Err(ApiError::forbidden(message));
    }
    let path = state.repository_path(&repository);
    let actor_user_id = actor.user.id;
    let (author_name, author_email) =
        commit_author(state.identity().database(), &actor.user).await?;
    let author = format!("{author_name} <{author_email}>");
    let result = tokio::task::spawn_blocking(move || {
        let commit = write_file(
            &path,
            &branch,
            request.expected_commit.as_deref(),
            &request.path,
            request.content.into_bytes(),
            &request.message,
            &author,
            write,
        )?;
        Ok::<_, ApiError>(CreateFileResponse {
            path: request.path,
            branch,
            commit: commit.to_hex(),
        })
    })
    .await
    .map_err(ApiError::internal)??;
    super::resources::record_push(
        &state,
        repository.id,
        actor_user_id,
        format!("{namespace}/{name}"),
    )
    .await?;
    Ok(Json(result))
}

fn validate_request(request: &CreateFileRequest) -> Result<(), ApiError> {
    gitadel_repositories::files::validate_write(
        &request.branch,
        &request.path,
        &request.content,
        &request.message,
    )
    .map_err(super::service_error)
}

/// Creates a file; the shorthand tests use for building up a branch.
#[cfg(test)]
pub(super) fn commit_file(
    path: &Path,
    branch: &str,
    expected_commit: Option<&str>,
    file_path: &str,
    content: Vec<u8>,
    message: &str,
    username: &str,
) -> Result<ObjectId, ApiError> {
    write_file(
        path,
        branch,
        expected_commit,
        file_path,
        content,
        message,
        &format!("{username} <{username}@gitadel.local>"),
        FileWrite::Create,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn write_file(
    path: &Path,
    branch: &str,
    expected_commit: Option<&str>,
    file_path: &str,
    content: Vec<u8>,
    message: &str,
    author: &str,
    write: FileWrite,
) -> Result<sley::ObjectId, ApiError> {
    gitadel_repositories::files::write_file(
        path,
        branch,
        expected_commit,
        file_path,
        content,
        message,
        author,
        write,
    )
    .map_err(super::service_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, response::IntoResponse};

    struct TestDirectory(std::path::PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn nested_file_creation_preserves_content_and_rejects_stale_writes() {
        let directory = TestDirectory(
            std::env::temp_dir().join(format!("gitadel-file-test-{}", uuid::Uuid::new_v4())),
        );
        let git = GitRepository::init_bare(&directory.0).unwrap();
        let first = commit_file(
            &directory.0,
            "main",
            None,
            "notes/first.txt",
            b"first".to_vec(),
            "First",
            "test",
        )
        .unwrap();
        let first_hex = first.to_hex();
        let second = commit_file(
            &directory.0,
            "main",
            Some(&first_hex),
            "notes/second.txt",
            b"second".to_vec(),
            "Second",
            "test",
        )
        .unwrap();
        for expected in [None, Some(first_hex.as_str())] {
            let error = commit_file(
                &directory.0,
                "main",
                expected,
                "stale.txt",
                b"stale".to_vec(),
                "Stale",
                "test",
            )
            .unwrap_err();
            assert_eq!(error.into_response().status(), StatusCode::CONFLICT);
        }
        assert_eq!(git.rev_parse("refs/heads/main").unwrap(), second);
        let first_file = git
            .resolve_path(&second.to_hex(), "notes/first.txt")
            .unwrap();
        let second_file = git
            .resolve_path(&second.to_hex(), "notes/second.txt")
            .unwrap();
        assert_eq!(git.blobs().read(first_file.oid).unwrap(), b"first");
        assert_eq!(git.blobs().read(second_file.oid).unwrap(), b"second");
    }

    #[test]
    fn updating_replaces_an_existing_file_and_refuses_everything_else() {
        let directory = TestDirectory(
            std::env::temp_dir().join(format!("gitadel-file-test-{}", uuid::Uuid::new_v4())),
        );
        let git = GitRepository::init_bare(&directory.0).unwrap();
        let first = commit_file(
            &directory.0,
            "main",
            None,
            "notes/first.txt",
            b"first".to_vec(),
            "First",
            "test",
        )
        .unwrap()
        .to_hex();
        let update = |expected: &str, path: &str, content: &[u8]| {
            write_file(
                &directory.0,
                "main",
                Some(expected),
                path,
                content.to_vec(),
                "Edit",
                "test <test@example.com>",
                FileWrite::Update,
            )
        };

        let edited = update(&first, "notes/first.txt", b"edited").unwrap();
        let file = git
            .resolve_path(&edited.to_hex(), "notes/first.txt")
            .unwrap();
        assert_eq!(git.blobs().read(file.oid).unwrap(), b"edited");
        assert_eq!(
            git.read_commit(&edited)
                .unwrap()
                .parents
                .iter()
                .map(ObjectId::to_hex)
                .collect::<Vec<_>>(),
            vec![first.clone()]
        );

        let tip = edited.to_hex();
        let status =
            |result: Result<ObjectId, ApiError>| result.unwrap_err().into_response().status();
        // Nothing changed, the file is missing, a directory, or the tip moved.
        assert_eq!(
            status(update(&tip, "notes/first.txt", b"edited")),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status(update(&tip, "notes/missing.txt", b"x")),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status(update(&tip, "absent/file.txt", b"x")),
            StatusCode::CONFLICT
        );
        assert_eq!(status(update(&tip, "notes", b"x")), StatusCode::CONFLICT);
        assert_eq!(
            status(update(&first, "notes/first.txt", b"late")),
            StatusCode::CONFLICT
        );
        assert_eq!(git.rev_parse("refs/heads/main").unwrap(), edited);
    }
}
