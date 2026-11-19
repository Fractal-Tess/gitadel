//! Content language analysis and verified commit authorship.
use crate::signature::{SshCommitSignature, verify_ssh_commit_signature};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use sley::{ObjectId, ReachableCommitOptions, Repository as GitRepository, StreamControl};
use std::collections::BTreeMap;
use tokei::{Config as TokeiConfig, LanguageType};

/// Lines of one language in a repository tree.
pub struct LanguageStat {
    pub language: String,
    pub files: usize,
    pub code: usize,
    pub comments: usize,
    pub blanks: usize,
}

/// Verified contributions grouped by the author's local calendar day.
pub struct AuthorActivity {
    pub date: NaiveDate,
    pub email: String,
    pub signer: Option<String>,
    pub count: usize,
}

/// History shared by all branches, counted once even across merge paths.
pub struct HistoryOverview {
    pub head: Option<String>,
    pub authors: Vec<AuthorActivity>,
}

/// Walk the supplied branch tips without reading application settings or caches.
pub fn walk_overview(
    git: &GitRepository,
    tips: &[(String, ObjectId)],
    default_reference: Option<&str>,
    since: NaiveDate,
) -> sley::Result<HistoryOverview> {
    let mut roots = Vec::with_capacity(tips.len());
    let mut default_index = None;
    for (name, target) in tips {
        let commit_oid = git.peel_to_commit_oid(*target)?;
        if default_reference == Some(name.as_str()) {
            default_index = Some(roots.len());
        }
        roots.push(commit_oid);
    }
    let head = default_index.map(|index: usize| roots[index].to_hex());
    let mut authors = BTreeMap::<(NaiveDate, String, Option<String>), usize>::new();
    if !roots.is_empty() {
        git.rev_graph().stream_reachable_commits(
            roots,
            ReachableCommitOptions::new(),
            |metadata| {
                let commit = git.read_commit(&metadata.oid)?;
                let signature = commit.author_signature();
                if let Some(date) = signature.as_ref().and_then(authored_on)
                    && date >= since
                {
                    let (email, signer) =
                        commit_authorship(git, &metadata.oid, signature.as_ref())?;
                    *authors.entry((date, email, signer)).or_default() += 1;
                }
                Ok(StreamControl::Continue)
            },
        )?;
    }
    Ok(HistoryOverview {
        head,
        authors: authors
            .into_iter()
            .map(|((date, email, signer), count)| AuthorActivity {
                date,
                email,
                signer,
                count,
            })
            .collect(),
    })
}

pub fn compute_stats(
    repository: &GitRepository,
    root: ObjectId,
) -> sley::Result<Vec<LanguageStat>> {
    let mut totals = BTreeMap::<String, LanguageStat>::new();
    let config = TokeiConfig::default();
    collect_tree_stats(repository, root, "", &config, &mut totals)?;
    let mut response = totals.into_values().collect::<Vec<_>>();
    response.sort_unstable_by(|left, right| {
        right
            .code
            .cmp(&left.code)
            .then_with(|| left.language.cmp(&right.language))
    });
    Ok(response)
}

fn collect_tree_stats(
    repository: &GitRepository,
    tree_oid: ObjectId,
    prefix: &str,
    config: &TokeiConfig,
    totals: &mut BTreeMap<String, LanguageStat>,
) -> sley::Result<()> {
    for entry in repository.read_tree(&tree_oid)?.entries {
        let name = String::from_utf8_lossy(entry.name.as_bytes());
        let path = if prefix.is_empty() {
            name.into_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if entry.is_tree() {
            collect_tree_stats(repository, entry.oid, &path, config, totals)?;
            continue;
        }
        if entry.is_gitlink() {
            continue;
        }
        let Some(language) = LanguageType::from_path(&path, config) else {
            continue;
        };
        let content = repository.blobs().read(entry.oid)?;
        let stats = language.parse_from_slice(&content, config).summarise();
        let language_name = language.to_string();
        let total = totals
            .entry(language_name.clone())
            .or_insert_with(|| LanguageStat {
                language: language_name,
                files: 0,
                code: 0,
                comments: 0,
                blanks: 0,
            });
        total.files += 1;
        total.code += stats.code;
        total.comments += stats.comments;
        total.blanks += stats.blanks;
    }
    Ok(())
}

/// The lowercased author address of a commit, and the fingerprint of the SSH
/// key that validly signed it.
pub fn commit_authorship(
    git: &GitRepository,
    oid: &ObjectId,
    signature: Option<&sley::Signature>,
) -> sley::Result<(String, Option<String>)> {
    let email = signature
        .map(|signature| String::from_utf8_lossy(signature.email.as_bytes()).to_ascii_lowercase())
        .unwrap_or_default();
    let signer = match verify_ssh_commit_signature(&git.read_object(oid)?.body) {
        SshCommitSignature::Valid(fingerprint) => Some(fingerprint),
        SshCommitSignature::Unsigned | SshCommitSignature::Invalid => None,
    };
    Ok((email, signer))
}

/// The calendar day a commit was authored on, in the author's own time zone,
/// which is the day the activity graph counts it under.
pub fn authored_on(signature: &sley::Signature) -> Option<NaiveDate> {
    let timestamp = DateTime::<Utc>::from_timestamp(signature.time.seconds, 0)?;
    Some(
        (timestamp + Duration::minutes(i64::from(signature.time.timezone_offset_minutes)))
            .date_naive(),
    )
}
