//! Validated file writes and optimistic branch transactions.
use crate::Error;
use chrono::Utc;
use sley::{
    BString, CommitObject, FullName, GitObjectType, ObjectId, RefPrecondition, ReferenceTarget,
    Repository as GitRepository, TreeEditor,
};
use std::path::Path;
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 1024;
const MAX_MESSAGE_BYTES: usize = 255;
/// Whether a write adds a new file or replaces the contents of an existing one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileWrite {
    Create,
    Update,
}

/// Validate a browser file write before the adapter checks its ref policy.
pub fn validate_write(branch: &str, path: &str, content: &str, message: &str) -> Result<(), Error> {
    validate_input(branch, path, content.len(), message)
}

fn validate_input(
    branch: &str,
    path: &str,
    content_bytes: usize,
    message: &str,
) -> Result<(), Error> {
    let branch = branch.trim();
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
        return Err(Error::invalid_input("The selected branch is invalid."));
    }
    if FullName::new(format!("refs/heads/{branch_name}")).is_err() {
        return Err(Error::invalid_input("The selected branch is invalid."));
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(Error::invalid_input("The file path is too long."));
    }
    if content_bytes > MAX_FILE_BYTES {
        return Err(Error::invalid_input("The file is too large."));
    }
    if message.trim().is_empty() || message.len() > MAX_MESSAGE_BYTES {
        return Err(Error::invalid_input(
            "A commit message up to 255 characters is required.",
        ));
    }
    if path.split('/').enumerate().any(|(depth, component)| {
        depth >= 64
            || component.is_empty()
            || component == "."
            || component == ".."
            || component.eq_ignore_ascii_case(".git")
            || component.contains('\\')
            || component.chars().any(char::is_control)
    }) {
        return Err(Error::invalid_input(
            "The path must contain only normal file names.",
        ));
    }
    Ok(())
}

/// Commits `content` at `file_path` on top of the branch tip, which must still
/// be `expected_commit`. Creating refuses an occupied path; updating needs a
/// regular file there, keeps its mode, and refuses a write that changes
/// nothing.
#[allow(clippy::too_many_arguments)]
pub fn write_file(
    path: &Path,
    branch: &str,
    expected_commit: Option<&str>,
    file_path: &str,
    content: Vec<u8>,
    message: &str,
    author: &str,
    write: FileWrite,
) -> Result<ObjectId, Error> {
    validate_input(branch, file_path, content.len(), message)?;
    let repository = GitRepository::open_exact_bare(path).map_err(Error::internal)?;
    let reference = format!("refs/heads/{branch}");
    let current = repository
        .references()
        .read_ref(&reference)
        .map_err(Error::internal)?;
    let current_oid = match current {
        Some(ReferenceTarget::Direct(oid)) => Some(oid),
        Some(ReferenceTarget::Symbolic(_)) => {
            return Err(Error::conflict("The selected branch is symbolic."));
        }
        None => None,
    };
    let matches = match (current_oid, expected_commit) {
        (None, None) => true,
        (Some(oid), Some(expected)) => oid.to_hex() == expected,
        _ => false,
    };
    if !matches {
        return Err(Error::conflict(
            "The branch changed while this editor was open.",
        ));
    }

    let root = current_oid
        .map(|oid| repository.read_commit(&oid).map(|commit| commit.tree))
        .transpose()
        .map_err(Error::internal)?;
    let blob = repository.write_blob(content).map_err(Error::internal)?;
    let components = file_path.split('/').collect::<Vec<_>>();
    let tree = upsert_file(&repository, root, &components, blob, write)?;
    if write == FileWrite::Update && root == Some(tree) {
        return Err(Error::invalid_input(
            "The file is unchanged, so there is nothing to commit.",
        ));
    }
    let now = Utc::now().timestamp();
    // `author` is "Name <address>", already checked to hold no line breaks.
    let identity = format!("{author} {now} +0000");
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
        .map_err(Error::internal)?;
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
        .map_err(|error| Error::conflict(error.to_string()))?;
    Ok(commit_oid)
}

fn upsert_file(
    repository: &GitRepository,
    root: Option<ObjectId>,
    components: &[&str],
    blob: ObjectId,
    write: FileWrite,
) -> Result<ObjectId, Error> {
    let existing_tree = root
        .map(|oid| repository.read_tree(&oid).map_err(Error::internal))
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
        let mode = match (write, existing) {
            (FileWrite::Create, None) => 0o100644,
            (FileWrite::Create, Some(_)) => {
                return Err(Error::conflict(
                    "A file or directory already exists at that path.",
                ));
            }
            // Keep the executable bit; symlinks, submodules, and directories
            // are not text files to edit.
            (FileWrite::Update, Some((mode @ (0o100644 | 0o100755), _))) => mode,
            (FileWrite::Update, Some(_)) => {
                return Err(Error::conflict("Only regular files can be edited."));
            }
            (FileWrite::Update, None) => {
                return Err(Error::conflict("The file no longer exists on this branch."));
            }
        };
        builder.upsert_raw(BString::from(name.as_bytes()), mode, blob);
    } else {
        let child = match existing {
            Some((0o040000, oid)) => Some(oid),
            Some(_) => return Err(Error::conflict("A path component is already a file.")),
            None if write == FileWrite::Update => {
                return Err(Error::conflict("The file no longer exists on this branch."));
            }
            None => None,
        };
        let child_tree = upsert_file(repository, child, &components[1..], blob, write)?;
        builder.upsert_raw(BString::from(name.as_bytes()), 0o040000, child_tree);
    }
    repository.write_tree(builder).map_err(Error::internal)
}
