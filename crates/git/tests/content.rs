use gitadel_git::{
    Error, analysis, archive, branches, browser,
    files::{self, FileWrite},
    git_service::{self, RefCheck, RefPolicy},
};
use sley::{ObjectFormat, Repository};

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("gitadel-repositories-{}", uuid::Uuid::new_v4())))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn edited_content_is_browsable_analyzable_and_archivable_in_both_object_formats() {
    for format in [ObjectFormat::Sha1, ObjectFormat::Sha256] {
        let scratch = Scratch::new();
        let repository = Repository::init_with_format(&scratch.0, format, true).unwrap();
        let first = files::write_file(
            &scratch.0,
            "main",
            None,
            "src/main.rs",
            b"fn main() {}\n".to_vec(),
            "Create",
            "Alice <alice@example.test>",
            FileWrite::Create,
        )
        .unwrap();
        let edited = files::write_file(
            &scratch.0,
            "main",
            Some(&first.to_hex()),
            "src/main.rs",
            b"fn main() { println!(\"hello\"); }\n".to_vec(),
            "Edit",
            "Alice <alice@example.test>",
            FileWrite::Update,
        )
        .unwrap();
        let stale = files::write_file(
            &scratch.0,
            "main",
            Some(&first.to_hex()),
            "src/main.rs",
            b"stale".to_vec(),
            "Stale",
            "Alice <alice@example.test>",
            FileWrite::Update,
        );
        assert!(matches!(stale, Err(Error::Conflict(_))));
        let (tree, tip) =
            browser::read_tree(&repository, "main".to_owned(), "src".to_owned()).unwrap();
        assert_eq!(tip, edited);
        assert_eq!(tree.entries[0].path, "src/main.rs");
        let (blob, size) =
            browser::resolve_blob(&repository, &edited.to_hex(), "src/main.rs").unwrap();
        assert_eq!(repository.blobs().read(blob).unwrap().len() as u64, size);
        assert_eq!(
            browser::count_reachable_commits(&repository, edited).unwrap(),
            2
        );
        assert_eq!(
            branches::select_default_branch(&repository, Some("missing"))
                .unwrap()
                .as_deref(),
            Some("main")
        );
        let commit = repository.read_commit(&edited).unwrap();
        let overview = analysis::walk_overview(
            &repository,
            &[("refs/heads/main".to_owned(), edited)],
            Some("refs/heads/main"),
            chrono::NaiveDate::MIN,
        )
        .unwrap();
        assert_eq!(overview.head.as_deref(), Some(edited.to_hex().as_str()));
        assert_eq!(
            overview
                .authors
                .iter()
                .map(|author| author.count)
                .sum::<usize>(),
            2
        );
        let stats = analysis::compute_stats(&repository, commit.tree).unwrap();
        assert_eq!(stats[0].language, "Rust");
        assert_eq!(stats[0].files, 1);
        assert_eq!(
            analysis::commit_authorship(&repository, &edited, commit.author_signature().as_ref())
                .unwrap(),
            ("alice@example.test".to_owned(), None)
        );
        for (encoding, magic) in [
            (archive::ArchiveFormat::Zip, b"PK".as_slice()),
            (archive::ArchiveFormat::TarGz, b"\x1f\x8b".as_slice()),
        ] {
            let output = scratch.0.join(match encoding {
                archive::ArchiveFormat::Zip => "archive.zip",
                archive::ArchiveFormat::TarGz => "archive.tar.gz",
            });
            archive::write_archive_file(
                &scratch.0,
                &output,
                encoding,
                edited,
                commit.tree,
                1_704_067_200,
                "project/".to_owned(),
            )
            .unwrap();
            assert!(std::fs::read(output).unwrap().starts_with(magic));
        }
    }
}

struct RejectUpdates;

impl RefPolicy for RejectUpdates {
    fn check(&self, _: &str, _: bool, _: bool) -> RefCheck {
        RefCheck::Rejected("adapter policy rejected update".to_owned())
    }
}

#[test]
fn adapter_policy_rejection_preserves_wire_report_order_and_leaves_refs_unchanged() {
    let scratch = Scratch::new();
    let repository = Repository::init_bare(&scratch.0).unwrap();
    let zero = "0".repeat(40);
    let mut request = Vec::new();
    for (index, name) in ["refs/heads/main", "refs/tags/v1"].into_iter().enumerate() {
        let capabilities = if index == 0 {
            "\0report-status-v2 atomic"
        } else {
            ""
        };
        let command = format!("{zero} {zero} {name}{capabilities}\n");
        request.extend(format!("{:04x}{command}", command.len() + 4).as_bytes());
    }
    request.extend(b"0000");
    let mut response = Vec::new();
    let outcome = git_service::serve_receive_pack(
        &scratch.0,
        ObjectFormat::Sha1,
        &RejectUpdates,
        &mut std::io::Cursor::new(request),
        &mut response,
    )
    .unwrap();
    let report = String::from_utf8(response).unwrap();
    assert!(!outcome.landed);
    assert!(outcome.response_error.is_none());
    assert!(report.find("ng refs/heads/main").unwrap() < report.find("ng refs/tags/v1").unwrap());
    assert!(repository.references().list_refs().unwrap().is_empty());
}
