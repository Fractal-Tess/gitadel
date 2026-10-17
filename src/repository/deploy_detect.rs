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
const MAX_COMPOSE_BYTES: u64 = 256 * 1024;
/// Service names that usually carry the public web interface.
const WEB_SERVICE_NAMES: &[&str] = &["web", "app", "frontend", "server", "nginx", "site", "api"];

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
    /// The port the Dockerfile exposes, or the chosen Compose service's port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The Compose service a domain should route to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Every service in a Compose file, with the port it listens on if known.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ComposeService>,
    /// Things in the file that commonly break a Dokploy deployment.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ComposeService {
    pub name: String,
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
            let port = read_small(git, entry.oid, MAX_DOCKERFILE_BYTES)?
                .and_then(|content| exposed_port(&content));
            let warnings = if port.is_none() {
                vec![
                    "The Dockerfile has no EXPOSE instruction; give the port the app listens on."
                        .to_owned(),
                ]
            } else {
                Vec::new()
            };
            found.push((
                depth,
                BuildSuggestion {
                    kind: "application",
                    build_path: format!("/{prefix}"),
                    dockerfile: Some(name),
                    compose_path: None,
                    port,
                    service: None,
                    services: Vec::new(),
                    warnings,
                },
            ));
        } else if matches!(
            lower.as_str(),
            "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml"
        ) {
            let compose = read_small(git, entry.oid, MAX_COMPOSE_BYTES)?
                .map(|content| inspect_compose(&content))
                .unwrap_or_default();
            let chosen = compose.default_service();
            found.push((
                depth,
                BuildSuggestion {
                    kind: "compose",
                    build_path: "/".to_owned(),
                    dockerfile: None,
                    compose_path: Some(format!("./{}", join(prefix, &name))),
                    port: chosen.and_then(|service| service.port),
                    service: chosen.map(|service| service.name.clone()),
                    services: compose.services,
                    warnings: compose.warnings,
                },
            ));
        }
    }
    Ok(())
}

fn read_small(
    git: &GitRepository,
    oid: sley::ObjectId,
    limit: u64,
) -> sley::Result<Option<String>> {
    Ok(match git.read_object_header(&oid)? {
        Some((GitObjectType::Blob, size)) if size <= limit => git
            .blobs()
            .read(oid)
            .ok()
            .map(|content| String::from_utf8_lossy(&content).into_owned()),
        _ => None,
    })
}

#[derive(Default)]
struct ComposeFile {
    services: Vec<ComposeService>,
    warnings: Vec<String>,
}

impl ComposeFile {
    /// The service a domain most likely belongs to: a conventionally named
    /// one with a port, else the first with a port, else the first.
    fn default_service(&self) -> Option<&ComposeService> {
        let with_port = || {
            self.services
                .iter()
                .filter(|service| service.port.is_some())
        };
        with_port()
            .find(|service| WEB_SERVICE_NAMES.contains(&service.name.to_ascii_lowercase().as_str()))
            .or_else(|| with_port().next())
            .or_else(|| self.services.first())
    }
}

fn inspect_compose(content: &str) -> ComposeFile {
    use yaml_serde::Value;

    let mut file = ComposeFile::default();
    let Ok(document) = yaml_serde::from_str::<Value>(content) else {
        file.warnings
            .push("The Compose file is not valid YAML.".to_owned());
        return file;
    };
    let Some(services) = document.get("services").and_then(Value::as_mapping) else {
        return file;
    };
    for (name, service) in services {
        let Some(name) = name.as_str() else { continue };
        let expose = service
            .get("expose")
            .and_then(Value::as_sequence)
            .and_then(|ports| ports.iter().find_map(port_number));
        let published = service
            .get("ports")
            .and_then(Value::as_sequence)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let target = published.iter().find_map(container_port);
        if !published.is_empty() {
            file.warnings.push(format!(
                "Service {name} publishes host ports. Dokploy's proxy already routes the \
                 domain, and published ports clash with other apps on the server (80 and \
                 443 are the proxy's own). Replace `ports` with `expose`."
            ));
        }
        if service.get("container_name").is_some() {
            file.warnings.push(format!(
                "Service {name} sets container_name, which collides if the file is deployed \
                 twice. Remove it."
            ));
        }
        let binds_checkout = service
            .get("volumes")
            .and_then(Value::as_sequence)
            .is_some_and(|volumes| {
                volumes.iter().any(|volume| {
                    volume
                        .as_str()
                        .or_else(|| volume.get("source").and_then(Value::as_str))
                        .is_some_and(|source| source.starts_with("./") || source == ".")
                })
            });
        if binds_checkout {
            file.warnings.push(format!(
                "Service {name} mounts a path from the repository checkout, which Dokploy \
                 replaces on every deployment. Keep data in a named volume or under \
                 ../files/."
            ));
        }
        file.services.push(ComposeService {
            name: name.to_owned(),
            port: expose.or(target),
        });
    }
    file
}

fn port_number(value: &yaml_serde::Value) -> Option<u16> {
    match value {
        yaml_serde::Value::Number(number) => number.as_u64().and_then(|port| port.try_into().ok()),
        yaml_serde::Value::String(text) => text.split('/').next()?.trim().parse().ok(),
        _ => None,
    }
}

/// The container side of a `ports` entry: `"8080:80"`, `"80"`, `80`, or the
/// long form with `target`.
fn container_port(value: &yaml_serde::Value) -> Option<u16> {
    if let Some(target) = value.get("target") {
        return port_number(target);
    }
    match value {
        yaml_serde::Value::String(text) => text
            .split('/')
            .next()?
            .rsplit(':')
            .next()?
            .trim()
            .parse()
            .ok(),
        other => port_number(other),
    }
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
    use super::{exposed_port, inspect_compose};

    #[test]
    fn compose_inspection_finds_the_web_service_and_risky_settings() {
        let compose = inspect_compose(
            "services:\n  db:\n    image: postgres\n    expose: [\"5432\"]\n  web:\n    build: .\n    ports:\n      - \"8080:3000\"\n    container_name: web\n    volumes:\n      - ./data:/data\n  worker:\n    build: .\n",
        );
        let names: Vec<_> = compose
            .services
            .iter()
            .map(|service| (service.name.as_str(), service.port))
            .collect();
        assert_eq!(
            names,
            [("db", Some(5432)), ("web", Some(3000)), ("worker", None)]
        );
        let chosen = compose.default_service().unwrap();
        assert_eq!((chosen.name.as_str(), chosen.port), ("web", Some(3000)));
        assert_eq!(compose.warnings.len(), 3, "{:?}", compose.warnings);

        let clean = inspect_compose("services:\n  app:\n    build: .\n    expose:\n      - 8000\n");
        assert!(clean.warnings.is_empty());
        assert_eq!(clean.default_service().unwrap().port, Some(8000));
    }

    #[test]
    fn exposed_port_reads_the_final_stage() {
        let dockerfile = "FROM node AS build\nEXPOSE 3000\nFROM nginx\n  expose 80/tcp\n";
        assert_eq!(exposed_port(dockerfile), Some(80));
        assert_eq!(exposed_port("FROM scratch\n"), None);
        assert_eq!(exposed_port("EXPOSE $PORT\n"), None);
    }
}
