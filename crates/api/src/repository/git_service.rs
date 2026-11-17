pub(crate) use gitadel_repositories::git_service::*;

#[cfg(test)]
mod tests {
    use std::{
        io,
        path::{Path, PathBuf},
    };

    use sley::{
        ObjectFormat, ObjectId, RefPrecondition, ReferenceTarget, Repository as GitRepository,
    };

    use super::*;
    use crate::repository::{
        files::commit_file,
        protection::{RefGuard, test_rule},
    };

    const ZERO: &str = "0000000000000000000000000000000000000000";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("gitadel-receive-{label}-{}", uuid::Uuid::new_v4())),
            )
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn commit(path: &Path, branch: &str, parent: Option<ObjectId>, file: &str) -> ObjectId {
        let parent = parent.map(|oid| oid.to_hex());
        commit_file(
            path,
            branch,
            parent.as_deref(),
            file,
            file.as_bytes().to_vec(),
            file,
            "test",
        )
        .unwrap()
    }

    fn pack(path: &Path, root: ObjectId) -> Vec<u8> {
        let repository = GitRepository::open_exact_bare(path).unwrap();
        repository
            .reachable_pack_plan()
            .root(root)
            .build()
            .unwrap()
            .unwrap()
            .prepare_to_memory()
            .unwrap()
            .pack
    }

    fn pkt(line: &str) -> Vec<u8> {
        format!("{:04x}{line}", line.len() + 4).into_bytes()
    }

    /// Build a receive-pack request whose first command carries capabilities.
    fn request(commands: &[(String, String, &str)], pack: Option<Vec<u8>>) -> Vec<u8> {
        let mut body = Vec::new();
        for (index, (old, new, name)) in commands.iter().enumerate() {
            let capabilities = if index == 0 { "\0report-status" } else { "" };
            body.extend(pkt(&format!("{old} {new} {name}{capabilities}\n")));
        }
        body.extend_from_slice(b"0000");
        body.extend(pack.unwrap_or_default());
        body
    }

    fn push(path: &Path, guard: &RefGuard, body: Vec<u8>) -> (ReceiveOutcome, String) {
        let mut output = Vec::new();
        let outcome = serve_receive_pack(
            path,
            ObjectFormat::Sha1,
            guard,
            &mut io::Cursor::new(body),
            &mut output,
        )
        .unwrap();
        (outcome, String::from_utf8_lossy(&output).into_owned())
    }

    fn tip(path: &Path, branch: &str) -> Option<ObjectId> {
        let repository = GitRepository::open_exact_bare(path).unwrap();
        match repository
            .references()
            .read_ref(&format!("refs/heads/{branch}"))
            .unwrap()
        {
            Some(ReferenceTarget::Direct(oid)) => Some(oid),
            _ => None,
        }
    }

    fn main_guard() -> RefGuard {
        RefGuard::for_tests(vec![test_rule("branch", "main")], None, false, &[])
    }

    #[test]
    fn protected_branch_rejects_force_push_but_accepts_other_refs_in_the_same_push() {
        let server = TestDirectory::new("server");
        let client = TestDirectory::new("client");
        GitRepository::init_bare(&server.0).unwrap();
        GitRepository::init_bare(&client.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");
        let unrelated = commit(&client.0, "main", None, "unrelated.txt");

        let body = request(
            &[
                (base.to_hex(), unrelated.to_hex(), "refs/heads/main"),
                (ZERO.to_owned(), unrelated.to_hex(), "refs/heads/scratch"),
            ],
            Some(pack(&client.0, unrelated)),
        );
        let (outcome, report) = push(&server.0, &main_guard(), body);

        assert!(
            report
                .contains("ng refs/heads/main protected branch `main`: force-push is not allowed"),
            "{report}"
        );
        assert!(report.contains("ok refs/heads/scratch"), "{report}");
        assert!(outcome.landed);
        assert_eq!(tip(&server.0, "main"), Some(base));
        assert_eq!(tip(&server.0, "scratch"), Some(unrelated));
        let leftovers = std::fs::read_dir(server.0.join("objects"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "incoming")
            })
            .count();
        assert_eq!(leftovers, 0, "probe quarantine was not removed");
    }

    #[test]
    fn protected_branch_accepts_fast_forward_pushes() {
        let server = TestDirectory::new("server");
        GitRepository::init_bare(&server.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");
        let next = commit(&server.0, "main", Some(base), "next.txt");
        let repository = GitRepository::open_exact_bare(&server.0).unwrap();
        let references = repository.references();
        let mut transaction = references.transaction();
        transaction.update_to(
            "refs/heads/main".to_owned(),
            ReferenceTarget::Direct(base),
            RefPrecondition::MustExistAndMatch(ReferenceTarget::Direct(next)),
            None,
        );
        transaction.commit().unwrap();

        let body = request(
            &[(base.to_hex(), next.to_hex(), "refs/heads/main")],
            Some(pack(&server.0, next)),
        );
        let (outcome, report) = push(&server.0, &main_guard(), body);

        assert!(report.contains("ok refs/heads/main"), "{report}");
        assert!(outcome.landed);
        assert_eq!(tip(&server.0, "main"), Some(next));
    }

    #[test]
    fn protected_branch_rejects_deletion() {
        let server = TestDirectory::new("server");
        GitRepository::init_bare(&server.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");

        let body = request(&[(base.to_hex(), ZERO.to_owned(), "refs/heads/main")], None);
        let (outcome, report) = push(&server.0, &main_guard(), body);

        assert!(
            report.contains("ng refs/heads/main protected branch `main`: deletion is not allowed"),
            "{report}"
        );
        assert!(!outcome.landed);
        assert_eq!(tip(&server.0, "main"), Some(base));
    }

    #[test]
    fn pushes_to_names_outside_refs_are_rejected_before_any_ref_is_written() {
        let server = TestDirectory::new("server");
        let client = TestDirectory::new("client");
        GitRepository::init_bare(&server.0).unwrap();
        GitRepository::init_bare(&client.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");
        let unrelated = commit(&client.0, "main", None, "unrelated.txt");
        // Point HEAD at the protected branch so a `HEAD` command would be
        // followed through the symref.
        std::fs::write(server.0.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        for guard in [main_guard(), RefGuard::default()] {
            let body = request(
                &[
                    (base.to_hex(), unrelated.to_hex(), "HEAD"),
                    (ZERO.to_owned(), unrelated.to_hex(), "shallow"),
                    (ZERO.to_owned(), unrelated.to_hex(), "packed-refs"),
                ],
                Some(pack(&client.0, unrelated)),
            );
            let (outcome, report) = push(&server.0, &guard, body);

            for name in ["HEAD", "shallow", "packed-refs"] {
                assert!(
                    report.contains(&format!("ng {name} ref names must start with `refs/`")),
                    "{report}"
                );
            }
            assert!(!outcome.landed);
            assert_eq!(tip(&server.0, "main"), Some(base));
            assert_eq!(
                std::fs::read_to_string(server.0.join("HEAD")).unwrap(),
                "ref: refs/heads/main\n"
            );
            assert!(!server.0.join("shallow").exists());
            assert!(!server.0.join("packed-refs").exists());
        }
    }

    #[test]
    fn unprotected_force_push_still_lands() {
        let server = TestDirectory::new("server");
        let client = TestDirectory::new("client");
        GitRepository::init_bare(&server.0).unwrap();
        GitRepository::init_bare(&client.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");
        let unrelated = commit(&client.0, "main", None, "unrelated.txt");

        let body = request(
            &[(base.to_hex(), unrelated.to_hex(), "refs/heads/main")],
            Some(pack(&client.0, unrelated)),
        );
        let (outcome, report) = push(&server.0, &RefGuard::default(), body);

        assert!(report.contains("ok refs/heads/main"), "{report}");
        assert!(outcome.landed);
        assert_eq!(tip(&server.0, "main"), Some(unrelated));
    }

    #[test]
    fn protected_tags_reject_moves_and_allow_new_tags() {
        let server = TestDirectory::new("server");
        GitRepository::init_bare(&server.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");
        let next = commit(&server.0, "main", Some(base), "next.txt");
        let guard = RefGuard::for_tests(vec![test_rule("tag", "v*")], None, false, &[]);

        let body = request(
            &[(ZERO.to_owned(), base.to_hex(), "refs/tags/v1")],
            Some(pack(&server.0, base)),
        );
        let (_, report) = push(&server.0, &guard, body);
        assert!(report.contains("ok refs/tags/v1"), "{report}");

        let body = request(
            &[(base.to_hex(), next.to_hex(), "refs/tags/v1")],
            Some(pack(&server.0, next)),
        );
        let (outcome, report) = push(&server.0, &guard, body);
        assert!(
            report.contains(
                "ng refs/tags/v1 protected tag `v1`: existing tags cannot be moved or deleted"
            ),
            "{report}"
        );
        assert!(!outcome.landed);
    }
}
