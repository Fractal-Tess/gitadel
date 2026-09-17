use std::path::Path;

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sley::{
    BString, CommitObject, FullName, GitObjectType, ObjectId, RefPrecondition, ReferenceTarget,
    Repository as GitRepository, TreeEditor,
};

use super::{Permission, RepositoryState};
use crate::identity::{ApiError, SCOPE_WRITE};

const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_FILE_REQUEST_BYTES: usize = MAX_FILE_BYTES * 6 + 16 * 1024;
const MAX_PATH_BYTES: usize = 1024;
const MAX_MESSAGE_BYTES: usize = 255;

#[derive(Deserialize)]
pub struct CreateFileRequest {
    pub branch: String,
    pub expected_commit: Option<String>,
    pub path: String,
    pub content: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct CreateFileResponse {
    pub path: String,
    pub branch: String,
    pub commit: String,
}

pub async fn create_file(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateFileRequest>,
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
    let path = state.repository_path(&repository);
    let actor_user_id = actor.user.id;
    let result = tokio::task::spawn_blocking(move || {
        let commit = commit_file(
            &path,
            &branch,
            request.expected_commit.as_deref(),
            &request.path,
            request.content.into_bytes(),
            &request.message,
            &actor.user.username,
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
    let branch = request.branch.trim();
    let branch_name = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    if branch.is_empty()
        || branch_name == "HEAD"
        || branch_name.starts_with('-')
        || branch.len() > MAX_PATH_BYTES
        || (branch.starts_with("refs/") && !branch.starts_with("refs/heads/"))
        || branch_name.split('/').any(|component| {
            component.is_empty()
                || component == "."
                || component == ".."
                || component.contains('\\')
                || component.chars().any(char::is_control)
        })
    {
        return Err(ApiError::bad_request("The selected branch is invalid."));
    }
    if FullName::new(format!("refs/heads/{branch_name}")).is_err() {
        return Err(ApiError::bad_request("The selected branch is invalid."));
    }
    if request.path.len() > MAX_PATH_BYTES {
        return Err(ApiError::bad_request("The file path is too long."));
    }
    if request.content.len() > MAX_FILE_BYTES {
        return Err(ApiError::bad_request("The file is too large."));
    }
    if request.message.trim().is_empty() || request.message.len() > MAX_MESSAGE_BYTES {
        return Err(ApiError::bad_request(
            "A commit message up to 255 characters is required.",
        ));
    }
    if request
        .path
        .split('/')
        .enumerate()
        .any(|(depth, component)| {
            depth >= 64
                || component.is_empty()
                || component == "."
                || component == ".."
                || component.eq_ignore_ascii_case(".git")
                || component.contains('\\')
                || component.chars().any(char::is_control)
        })
    {
        return Err(ApiError::bad_request(
            "The path must contain only normal file names.",
        ));
    }
    Ok(())
}

fn commit_file(
    path: &Path,
    branch: &str,
    expected_commit: Option<&str>,
    file_path: &str,
    content: Vec<u8>,
    message: &str,
    username: &str,
) -> Result<ObjectId, ApiError> {
    let repository = GitRepository::open_exact_bare(path).map_err(ApiError::internal)?;
    let reference = format!("refs/heads/{branch}");
    let current = repository
        .references()
        .read_ref(&reference)
        .map_err(ApiError::internal)?;
    let current_oid = match current {
        Some(ReferenceTarget::Direct(oid)) => Some(oid),
        Some(ReferenceTarget::Symbolic(_)) => {
            return Err(ApiError::conflict("The selected branch is symbolic."));
        }
        None => None,
    };
    let matches = match (current_oid, expected_commit) {
        (None, None) => true,
        (Some(oid), Some(expected)) => oid.to_hex() == expected,
        _ => false,
    };
    if !matches {
        return Err(ApiError::conflict(
            "The branch changed while this editor was open.",
        ));
    }

    let root = current_oid
        .map(|oid| repository.read_commit(&oid).map(|commit| commit.tree))
        .transpose()
        .map_err(ApiError::internal)?;
    let blob = repository.write_blob(content).map_err(ApiError::internal)?;
    let components = file_path.split('/').collect::<Vec<_>>();
    let tree = upsert_file(&repository, root, &components, blob)?;
    let now = Utc::now().timestamp();
    let identity = format!("{username} <{username}@gitadel.local> {now} +0000");
    let commit = CommitObject {
        tree,
        parents: current_oid.into_iter().collect(),
        author: identity.as_bytes().to_vec(),
        committer: identity.into_bytes(),
        encoding: None,
        message: format!("{message}\n").into_bytes(),
    };
    let commit_oid = repository
        .write_raw_object(GitObjectType::Commit, commit.write())
        .map_err(ApiError::internal)?;
    let references = repository.references();
    let mut transaction = references.transaction();
    transaction.update_to(
        reference,
        ReferenceTarget::Direct(commit_oid),
        current_oid.map_or(RefPrecondition::MustNotExist, |oid| {
            RefPrecondition::MustExistAndMatch(ReferenceTarget::Direct(oid))
        }),
        None,
    );
    transaction
        .commit()
        .map_err(|error| ApiError::conflict(error.to_string()))?;
    Ok(commit_oid)
}

fn upsert_file(
    repository: &GitRepository,
    root: Option<ObjectId>,
    components: &[&str],
    blob: ObjectId,
) -> Result<ObjectId, ApiError> {
    let existing_tree = root
        .map(|oid| repository.read_tree(&oid).map_err(ApiError::internal))
        .transpose()?;
    let name = components[0];
    let existing = existing_tree.as_ref().and_then(|tree| {
        tree.entries
            .iter()
            .find(|entry| entry.name.as_bytes() == name.as_bytes())
            .map(|entry| (entry.mode, entry.oid))
    });
    let mut builder = existing_tree.map(TreeEditor::from_tree).unwrap_or_default();
    if components.len() == 1 {
        if existing.is_some() {
            return Err(ApiError::conflict(
                "A file or directory already exists at that path.",
            ));
        }
        builder.upsert_raw(BString::from(name.as_bytes()), 0o100644, blob);
    } else {
        let child = match existing {
            Some((mode, oid)) if mode == 0o040000 => Some(oid),
            Some(_) => return Err(ApiError::conflict("A path component is already a file.")),
            None => None,
        };
        let child_tree = upsert_file(repository, child, &components[1..], blob)?;
        builder.upsert_raw(BString::from(name.as_bytes()), 0o040000, child_tree);
    }
    repository.write_tree(builder).map_err(ApiError::internal)
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
}
