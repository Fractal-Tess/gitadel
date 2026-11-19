//! Default branch selection from valid commit-bearing refs.
use sley::{FullName, Repository as SleyRepository};

/// Whether a short name can identify a branch managed by Gitadel.
pub fn valid_branch_name(branch: &str) -> bool {
    !branch.is_empty()
        && branch != "HEAD"
        && !branch.starts_with('-')
        && FullName::new(format!("refs/heads/{branch}")).is_ok()
}

/// Prefer a valid configured branch, then conventional names, then newest tip.
pub fn select_default_branch(
    repository: &SleyRepository,
    configured: Option<&str>,
) -> sley::Result<Option<String>> {
    let mut branches = Vec::new();
    for reference in repository
        .references()
        .list_refs_with_prefix("refs/heads/")?
    {
        let Some(name) = reference.name.strip_prefix("refs/heads/") else {
            continue;
        };
        if !valid_branch_name(name) {
            continue;
        }
        let Ok(oid) = repository.rev_parse(&reference.name) else {
            continue;
        };
        let Ok(oid) = repository.peel_to_commit_oid(oid) else {
            continue;
        };
        let Ok(commit) = repository.read_commit(&oid) else {
            continue;
        };
        let commit_time = commit
            .committer_signature()
            .map_or(i64::MIN, |signature| signature.time.seconds);
        branches.push((name.to_owned(), commit_time));
    }

    if let Some(configured) = configured
        .filter(|configured| valid_branch_name(configured))
        .filter(|configured| branches.iter().any(|(name, _)| name == configured))
    {
        return Ok(Some(configured.to_owned()));
    }
    for preferred in ["main", "master", "prod", "staging"] {
        if branches.iter().any(|(name, _)| name == preferred) {
            return Ok(Some(preferred.to_owned()));
        }
    }
    Ok(branches
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
        .map(|(name, _)| name))
}
