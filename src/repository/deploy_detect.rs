//! Suggest how a repository deploys, from the files on its default branch.
//!
//! Only Dockerfiles and Compose files are recognised; they are what Dokploy
//! can build without further configuration. Suggestions are a starting point
//! the user confirms, never a decision Gitadel makes on its own.

use serde::Serialize;
use sley::{GitObjectType, Repository as GitRepository};

/// Directories that hold dependencies or build output rather than a service.
const SKIPPED_DIRECTORIES: &[&str] = &[
    "node_modules",
    "vendor",
    "dist",
    "build",
    "target",
    "out",
    ".git",
    ".next",
];
const MAX_DEPTH: usize = 3;
const MAX_ENTRIES: usize = 5_000;
const MAX_SUGGESTIONS: usize = 8;
const MAX_DOCKERFILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct BuildSuggestion {
    /// `application` for a Dockerfile, `compose` for a Compose file.
    pub kind: &'static str,
    /// Directory Dokploy builds from, such as `/` or `/web`.
    pub build_path: String,
    /// Dockerfile name relative to `build_path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dockerfile: Option<String>,
    /// Compose file relative to the repository root, such as `./compose.yml`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose_path: Option<String>,
    /// The port the Dockerfile exposes, if it declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

pub(crate) fn suggest(git: &GitRepository, revision: &str) -> sley::Result<Vec<BuildSuggestion>> {
    let commit_oid = git.peel_to_commit_oid(git.rev_parse(revision)?)?;
    let tree_oid = git.read_commit(&commit_oid)?.tree;
    let mut found = Vec::new();
    let mut visited = 0;
    walk(git, tree_oid, "", 0, &mut visited, &mut found)?;
    // Shallow files first: the root service is the likeliest deployment.
    found.sort_by_key(|(depth, suggestion): &(usize, BuildSuggestion)| {
        (*depth, suggestion.kind == "compose")
    });
    Ok(found
        .into_iter()
        .map(|(_, suggestion)| suggestion)
        .take(MAX_SUGGESTIONS)
        .collect())
}

fn walk(
    git: &GitRepository,
    tree_oid: sley::ObjectId,
    prefix: &str,
    depth: usize,
    visited: &mut usize,
    found: &mut Vec<(usize, BuildSuggestion)>,
) -> sley::Result<()> {
    for entry in git.read_tree(&tree_oid)?.entries {
        *visited += 1;
        if *visited > MAX_ENTRIES {
            return Ok(());
        }
        let name = String::from_utf8_lossy(entry.name.as_bytes()).into_owned();
        if entry.is_gitlink() || entry.mode == 0o120000 {
            continue;
        }
        if entry.is_tree() {
            if depth < MAX_DEPTH && !SKIPPED_DIRECTORIES.contains(&name.as_str()) {
                let path = join(prefix, &name);
                walk(git, entry.oid, &path, depth + 1, visited, found)?;
            }
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if lower == "dockerfile" {
            let port = match git.read_object_header(&entry.oid)? {
                Some((GitObjectType::Blob, size)) if size <= MAX_DOCKERFILE_BYTES => git
                    .blobs()
                    .read(entry.oid)
                    .ok()
                    .and_then(|content| exposed_port(&String::from_utf8_lossy(&content))),
                _ => None,
            };
            found.push((
                depth,
                BuildSuggestion {
                    kind: "application",
                    build_path: format!("/{prefix}"),
                    dockerfile: Some(name),
                    compose_path: None,
                    port,
                },
            ));
        } else if matches!(
            lower.as_str(),
            "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml"
        ) {
            found.push((
                depth,
                BuildSuggestion {
                    kind: "compose",
                    build_path: "/".to_owned(),
                    dockerfile: None,
                    compose_path: Some(format!("./{}", join(prefix, &name))),
                    port: None,
                },
            ));
        }
    }
    Ok(())
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

/// The last port an `EXPOSE` instruction declares, which in a multi-stage
/// build belongs to the final image.
fn exposed_port(dockerfile: &str) -> Option<u16> {
    dockerfile
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| {
            line.get(..7)
                .is_some_and(|keyword| keyword.eq_ignore_ascii_case("expose "))
        })
        .filter_map(|line| {
            line[7..]
                .split_whitespace()
                .next()
                .and_then(|port| port.split('/').next())
                .and_then(|port| port.parse().ok())
        })
        .next()
}

#[cfg(test)]
mod tests {
    use super::exposed_port;

    #[test]
    fn exposed_port_reads_the_final_stage() {
        let dockerfile = "FROM node AS build\nEXPOSE 3000\nFROM nginx\n  expose 80/tcp\n";
        assert_eq!(exposed_port(dockerfile), Some(80));
        assert_eq!(exposed_port("FROM scratch\n"), None);
        assert_eq!(exposed_port("EXPOSE $PORT\n"), None);
    }
}
