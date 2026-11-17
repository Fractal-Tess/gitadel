//! Commit-pinned snippets, declaration outlines, and native line attribution.
use super::{
    RepositoryState,
    browser::{normalize_browse_path, read_git, readable_repository, signature_response},
};
use crate::identity::ApiError;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use sley::{GitError, GitObjectType, ObjectId, Repository as GitRepository};
use sley_rev::blame::{BlameRequest, PassThroughConverter, compute_blame};
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};
use utoipa::ToSchema;

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CodeQuery {
    pub rev: Option<String>,
    pub path: String,
    pub start: Option<usize>,
    pub end: Option<usize>,
}
struct TextFile {
    commit: ObjectId,
    path: String,
    content: String,
}

async fn text_file(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
    query: &CodeQuery,
) -> Result<(std::path::PathBuf, TextFile), ApiError> {
    let repository = readable_repository(state, headers, jar, namespace, name).await?;
    let revision = query
        .rev
        .as_deref()
        .filter(|rev| !rev.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| repository.default_branch.clone())
        .ok_or_else(ApiError::not_found)?;
    let path = normalize_browse_path(&query.path)?;
    if path.is_empty() {
        return Err(ApiError::bad_request("A file path is required."));
    }
    let root = state.repository_path(&repository);
    let result = read_git(root.clone(), move |git| {
        let commit = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let entry = git.resolve_path(&commit.to_hex(), &path)?;
        if entry.object_type != GitObjectType::Blob {
            return Err(GitError::InvalidPath(path));
        }
        let size = git
            .read_object_header(&entry.oid)?
            .map_or(u64::MAX, |(_, size)| size);
        if size > 2 * 1024 * 1024 {
            return Ok(None);
        }
        let bytes = git.blobs().read(entry.oid)?;
        let content = String::from_utf8(bytes)
            .ok()
            .filter(|text| !text.contains('\0'));
        Ok(content.map(|content| TextFile {
            commit,
            path,
            content,
        }))
    })
    .await?
    .ok_or_else(|| ApiError::bad_request("This feature supports UTF-8 text files up to 2 MiB."))?;
    Ok((root, result))
}

#[derive(Serialize, ToSchema)]
pub struct SnippetResponse {
    pub commit_oid: String,
    pub path: String,
    pub line_count: usize,
    pub start: usize,
    pub end: usize,
    pub content: String,
    pub permalink: String,
}
fn line_range(query: &CodeQuery, count: usize) -> Result<(usize, usize), ApiError> {
    let start = query.start.unwrap_or(1);
    let end = query
        .end
        .unwrap_or(if query.start.is_some() { start } else { count });
    if count == 0 && query.start.is_none() && query.end.is_none() {
        return Ok((0, 0));
    }
    if start == 0 || end < start || end > count {
        return Err(ApiError::bad_request("The line range is outside the file."));
    }
    Ok((start, end))
}

#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/snippet",
    operation_id = "get_repositories_namespace_name_snippet",
    summary = "Read a commit-pinned line range and its permanent link",
    tag = "Repository content",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        CodeQuery,
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = SnippetResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn snippet(
    State(state): State<RepositoryState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<CodeQuery>,
) -> Result<Json<SnippetResponse>, ApiError> {
    let (_, file) = text_file(&state, &headers, &jar, &namespace, &name, &query).await?;
    let lines = file.content.lines().collect::<Vec<_>>();
    let (start, end) = line_range(&query, lines.len())?;
    let mut url = state.public_url().clone();
    url.set_path(&format!("/{namespace}/{name}"));
    url.query_pairs_mut()
        .append_pair("rev", &file.commit.to_hex())
        .append_pair("path", &file.path)
        .append_pair("plain", "1");
    if start > 0 {
        url.set_fragment(Some(&if start == end {
            format!("L{start}")
        } else {
            format!("L{start}-L{end}")
        }));
    }
    Ok(Json(SnippetResponse {
        commit_oid: file.commit.to_hex(),
        path: file.path,
        line_count: lines.len(),
        start,
        end,
        content: if start == 0 {
            String::new()
        } else {
            lines[start - 1..end].join("\n")
        },
        permalink: url.to_string(),
    }))
}

#[derive(Serialize, ToSchema)]
pub struct CodeSymbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
}
#[derive(Serialize, ToSchema)]
pub struct SymbolsResponse {
    pub commit_oid: String,
    pub path: String,
    pub symbols: Vec<CodeSymbol>,
}
// Declaration matching intentionally avoids guessing references or definitions of calls.
static DECLARATIONS: LazyLock<Vec<(&str, regex::Regex)>> = LazyLock::new(|| {
    [
    ("function",r"^\s*(?:(?:pub(?:\([^)]*\))?|async|export|default|static|unsafe|extern|private|public|protected)\s+)*(?:fn|function\*?|def|func)\s+(?:\([^)]*\)\s*)?([\p{L}_$][\p{L}\p{N}_$]*)"),
    ("type",r"^\s*(?:(?:pub(?:\([^)]*\))?|export|default|public|private|abstract|sealed)\s+)*(?:class|struct|enum|trait|interface|type|mod|namespace)\s+([\p{L}_$][\p{L}\p{N}_$]*)"),
    ("function",r"^\s*(?:export\s+)?(?:const|let|var)\s+([\p{L}_$][\p{L}\p{N}_$]*)\s*(?::[^=]+)?=\s*(?:async\s+)?(?:\([^)]*\)|[\w$]+)\s*(?::[^=]+)?=>"),
].into_iter().filter_map(|(kind,pattern)|regex::Regex::new(pattern).ok().map(|regex|(kind,regex))).collect()
});

fn outline(text: &str) -> Vec<CodeSymbol> {
    let mut in_comment = false;
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim_start();
            if in_comment {
                if trimmed.contains("*/") {
                    in_comment = false;
                }
                return None;
            }
            if trimmed.starts_with("/*") {
                in_comment = !trimmed.contains("*/");
                return None;
            }
            if ["//", "#", "*"]
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
            {
                return None;
            }
            DECLARATIONS.iter().find_map(|(kind, regex)| {
                regex
                    .captures(line)
                    .and_then(|captures| captures.get(1))
                    .map(|name| CodeSymbol {
                        name: name.as_str().to_owned(),
                        kind: (*kind).to_owned(),
                        line: index + 1,
                    })
            })
        })
        .take(2000)
        .collect()
}
#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/symbols",
    operation_id = "get_repositories_namespace_name_symbols",
    summary = "List declarations in a text file",
    tag = "Repository content",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        CodeQuery,
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = SymbolsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn symbols(
    State(state): State<RepositoryState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<CodeQuery>,
) -> Result<Json<SymbolsResponse>, ApiError> {
    let (_, file) = text_file(&state, &headers, &jar, &namespace, &name, &query).await?;
    Ok(Json(SymbolsResponse {
        commit_oid: file.commit.to_hex(),
        path: file.path,
        symbols: outline(&file.content),
    }))
}

#[derive(Serialize, ToSchema)]
pub struct BlameCommit {
    pub author: String,
    pub timestamp: i64,
    pub title: String,
}
#[derive(Serialize, ToSchema)]
pub struct BlameLine {
    pub line: usize,
    pub commit_oid: String,
    pub original_path: String,
    pub original_line: usize,
}
#[derive(Serialize, ToSchema)]
pub struct BlameResponse {
    pub commit_oid: String,
    pub path: String,
    pub lines: Vec<BlameLine>,
    pub commits: HashMap<String, BlameCommit>,
}
#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/blame",
    operation_id = "get_repositories_namespace_name_blame",
    summary = "Attribute file lines to commits, following renames",
    tag = "Repository content",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        CodeQuery,
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = BlameResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn blame(
    State(state): State<RepositoryState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<CodeQuery>,
) -> Result<Json<BlameResponse>, ApiError> {
    let (root, file) = text_file(&state, &headers, &jar, &namespace, &name, &query).await?;
    let count = file.content.lines().count();
    if count > 20_000 {
        return Err(ApiError::bad_request(
            "Blame supports files up to 20,000 lines.",
        ));
    }
    let (start, end) = line_range(&query, count)?;
    // Keep the permit in the blocking operation even if its client disconnects.
    let slot = super::browser::history_slot().await?;
    let response = read_git(root, move |git| {
        let _slot = slot;
        let db = git.objects();
        let (attribution, _) = compute_blame(
            &BlameRequest {
                db: &db,
                reader: db.as_ref(),
                format: git.object_format(),
                start_commit: file.commit,
                repo_path: &file.path,
                final_blob: file.content.as_bytes(),
                first_parent: false,
                boundary_tip: None,
                copy_level: 0,
                copy_score: 40,
                virtual_final: false,
                ignore_set: &HashSet::new(),
                diff_algorithm: sley_diff_merge::DiffAlgorithm::Myers,
                grafts: &HashMap::new(),
            },
            &mut PassThroughConverter,
        )?;
        let mut commits = HashMap::new();
        let mut lines = Vec::new();
        for (index, row) in attribution
            .into_iter()
            .enumerate()
            .skip(start.saturating_sub(1))
            .take(end.saturating_sub(start) + usize::from(start > 0))
        {
            let oid = row.commit.to_hex();
            if !commits.contains_key(&oid) {
                let commit = git.read_commit(&row.commit)?;
                let author = signature_response(commit.author_signature(), &commit.author);
                commits.insert(
                    oid.clone(),
                    BlameCommit {
                        author: author.name,
                        timestamp: author.timestamp,
                        title: String::from_utf8_lossy(&commit.message)
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .to_owned(),
                    },
                );
            }
            lines.push(BlameLine {
                line: index + 1,
                commit_oid: oid,
                original_path: row.origin_path,
                original_line: row.origin_lineno,
            });
        }
        Ok(BlameResponse {
            commit_oid: file.commit.to_hex(),
            path: file.path,
            lines,
            commits,
        })
    })
    .await?;
    Ok(Json(response))
}

fn path_oid(git: &GitRepository, commit: ObjectId, path: &str) -> sley::Result<Option<ObjectId>> {
    match git.resolve_path(&commit.to_hex(), path) {
        Ok(entry) => Ok(Some(entry.oid)),
        Err(GitError::NotFound(_) | GitError::InvalidPath(_)) => Ok(None),
        Err(error) => Err(error),
    }
}
pub(super) fn touches_path(git: &GitRepository, oid: ObjectId, path: &str) -> sley::Result<bool> {
    let commit = git.read_commit(&oid)?;
    let current = path_oid(git, oid, path)?;
    if commit.parents.is_empty() {
        return Ok(current.is_some());
    }
    for parent in commit.parents {
        if path_oid(git, parent, path)? == current {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outline_recognizes_declarations_and_skips_comments() {
        let symbols = outline(
            "// fn ignored() {}\npub async fn serve() {}\nexport class Example {}\nconst run = () => {};\n",
        );
        assert_eq!(
            symbols
                .iter()
                .map(|symbol| (symbol.name.as_str(), symbol.line))
                .collect::<Vec<_>>(),
            vec![("serve", 2), ("Example", 3), ("run", 4)]
        );
    }
    #[test]
    fn ranges_reject_zero_reversed_and_past_eof() {
        for (start, end) in [(0, 1), (3, 2), (1, 4)] {
            assert!(
                line_range(
                    &CodeQuery {
                        rev: None,
                        path: "file".into(),
                        start: Some(start),
                        end: Some(end)
                    },
                    3
                )
                .is_err()
            );
        }
    }
}
