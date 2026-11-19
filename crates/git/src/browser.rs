//! Tree browsing and object metadata without HTTP response types.
use crate::Error;
use sley::{
    GitError, GitObjectType, ObjectId, ReachableCommitOptions, Repository as GitRepository,
    StreamControl,
};
use std::collections::HashMap;
const MAX_LFS_POINTER_BYTES: u64 = 1024;
const MAX_GITMODULES_BYTES: usize = 128 * 1024;

/// Committed submodule configuration; includes are deliberately never followed.
pub struct Submodule {
    pub name: String,
    pub url: Option<String>,
    pub branch: Option<String>,
}
/// One directory entry and its optional LFS or submodule metadata.
pub struct TreeEntry {
    pub name: String,
    pub path: String,
    pub oid: String,
    pub kind: &'static str,
    pub mode: u32,
    pub size: Option<u64>,
    pub lfs_size: Option<u64>,
    pub submodule: Option<Submodule>,
}
/// A directory listing pinned to a resolved commit.
pub struct TreeSnapshot {
    pub revision: String,
    pub commit_oid: String,
    pub commit_timestamp: i64,
    pub path: String,
    pub entries: Vec<TreeEntry>,
}

/// Read and sort directory entries, reading only small blobs for LFS metadata.
pub fn read_tree(
    git: &GitRepository,
    revision: String,
    requested_path: String,
) -> sley::Result<(TreeSnapshot, ObjectId)> {
    let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
    let commit = git.read_commit(&commit_oid)?;
    let commit_timestamp = commit
        .committer_signature()
        .map_or(0, |signature| signature.time.seconds);
    let commit_revision = commit_oid.to_hex();
    let resolved = git.resolve_path(&commit_revision, &requested_path)?;
    if resolved.object_type != GitObjectType::Tree {
        return Err(GitError::InvalidPath(requested_path));
    }
    let tree = git.read_tree(&resolved.oid)?;
    let mut entries = Vec::with_capacity(tree.entries.len());
    for entry in tree.entries {
        let name = String::from_utf8_lossy(entry.name.as_bytes()).into_owned();
        let entry_path = if requested_path.is_empty() {
            name.clone()
        } else {
            format!("{requested_path}/{name}")
        };
        let kind = if entry.is_tree() {
            "tree"
        } else if entry.is_gitlink() {
            "submodule"
        } else if entry.is_symlink() {
            "symlink"
        } else {
            "blob"
        };
        let size = if kind == "blob" || kind == "symlink" {
            git.read_object_header(&entry.oid)
                .ok()
                .flatten()
                .map(|(_, size)| size)
        } else {
            None
        };
        // Only small regular blobs can be pointers; never load large files or symlinks.
        let lfs_size = if kind == "blob" && size.is_some_and(|size| size < MAX_LFS_POINTER_BYTES) {
            lfs_pointer_metadata(&git.blobs().read(entry.oid)?).map(|pointer| pointer.size)
        } else {
            None
        };
        entries.push(TreeEntry {
            name,
            path: entry_path,
            oid: entry.oid.to_hex(),
            kind,
            mode: entry.mode,
            size,
            lfs_size,
            submodule: None,
        });
    }
    if entries.iter().any(|entry| entry.kind == "submodule") {
        let mut submodules = read_submodule_metadata(git, commit.tree)?;
        for entry in &mut entries {
            if entry.kind == "submodule" {
                entry.submodule = submodules.remove(&entry.path);
            }
        }
    }
    entries.sort_unstable_by(|left, right| {
        let left_file = left.kind != "tree";
        let right_file = right.kind != "tree";
        left_file
            .cmp(&right_file)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok((
        TreeSnapshot {
            revision,
            commit_oid: commit_oid.to_hex(),
            commit_timestamp,
            path: requested_path,
            entries,
        },
        commit_oid,
    ))
}

pub fn read_submodule_metadata(
    git: &GitRepository,
    commit_tree_oid: ObjectId,
) -> Result<HashMap<String, Submodule>, GitError> {
    let root_tree = git.read_tree(&commit_tree_oid)?;
    let Some(config_entry) = root_tree.entries.into_iter().find(|entry| {
        entry.name.as_bytes() == b".gitmodules" && matches!(entry.mode, 0o100644 | 0o100755)
    }) else {
        return Ok(HashMap::new());
    };
    let Some((object_type, size)) = git.read_object_header(&config_entry.oid)? else {
        return Err(GitError::InvalidObject(format!(
            "missing .gitmodules object {}",
            config_entry.oid
        )));
    };
    if object_type != GitObjectType::Blob {
        return Err(GitError::InvalidObject(format!(
            ".gitmodules object {} is not a blob",
            config_entry.oid
        )));
    }
    if size > MAX_GITMODULES_BYTES as u64 {
        return Ok(HashMap::new());
    }
    let bytes = git.blobs().read(config_entry.oid)?;
    Ok(parse_submodule_metadata(&bytes))
}

pub fn parse_submodule_metadata(bytes: &[u8]) -> HashMap<String, Submodule> {
    let Ok(config) = sley_config::GitConfig::parse(bytes) else {
        return HashMap::new();
    };

    let mut submodules = HashMap::new();
    for section in &config.sections {
        if !section.name.eq_ignore_ascii_case("submodule") {
            continue;
        }
        let Some(name) = section.subsection.as_ref().filter(|name| !name.is_empty()) else {
            continue;
        };
        let path = config
            .get("submodule", Some(name.as_str()), "path")
            .map(str::to_owned)
            .filter(|path| !path.is_empty());
        let Some(path) = path else {
            continue;
        };
        submodules.insert(
            path,
            Submodule {
                name: name.clone(),
                url: config
                    .get("submodule", Some(name.as_str()), "url")
                    .map(str::to_owned)
                    .filter(|value| !value.is_empty()),
                branch: config
                    .get("submodule", Some(name.as_str()), "branch")
                    .map(str::to_owned)
                    .filter(|value| !value.is_empty()),
            },
        );
    }
    submodules
}

pub fn count_reachable_commits(
    repository: &GitRepository,
    tip: ObjectId,
) -> Result<usize, GitError> {
    let mut count = 0;
    repository.rev_graph().stream_reachable_commits(
        [tip],
        ReachableCommitOptions::new(),
        |_| {
            count += 1;
            Ok(StreamControl::Continue)
        },
    )?;
    Ok(count)
}

pub fn normalize_browse_path(path: &str) -> Result<String, Error> {
    let normalized = path.trim_matches('/');
    if normalized.is_empty() {
        return Ok(String::new());
    }
    if normalized
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(Error::invalid_input("The repository path is invalid."));
    }
    Ok(normalized.to_owned())
}

/// Resolve a blob without reading its contents, for conditional downloads.
pub fn resolve_blob(
    git: &GitRepository,
    revision: &str,
    path: &str,
) -> sley::Result<(ObjectId, u64)> {
    let resolved = git.resolve_path(revision, path)?;
    if resolved.object_type != GitObjectType::Blob {
        return Err(GitError::InvalidPath(path.to_owned()));
    }
    let size = git
        .read_object_header(&resolved.oid)?
        .map_or(u64::MAX, |(_, size)| size);
    Ok((resolved.oid, size))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LfsPointerMetadata<'a> {
    pub oid: &'a str,
    pub size: u64,
}

pub fn lfs_pointer_metadata(content: &[u8]) -> Option<LfsPointerMetadata<'_>> {
    let text = std::str::from_utf8(content).ok()?.strip_suffix('\n')?;
    let mut lines = text.split('\n');
    match lines.next()? {
        "version https://git-lfs.github.com/spec/v1"
        | "version https://hawser.github.com/spec/v1" => {}
        _ => return None,
    }
    let mut previous_key = "";
    let mut oid = None;
    let mut size = None;
    for line in lines {
        let (key, value) = line.split_once(' ')?;
        if key.is_empty()
            || key <= previous_key
            || !key.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
            })
            || value.is_empty()
            || value.contains('\r')
        {
            return None;
        }
        previous_key = key;
        match key {
            "oid" => {
                let hash = value.strip_prefix("sha256:")?;
                if hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return None;
                }
                oid = Some(hash);
            }
            "size" => {
                if !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                size = Some(value.parse().ok()?);
            }
            "version" => return None,
            _ => {}
        }
    }
    Some(LfsPointerMetadata {
        oid: oid?,
        size: size?,
    })
}
