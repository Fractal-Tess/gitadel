//! Smart HTTP Git: ref advertisement authorization through the router, and
//! real `git push` round trips against an in-process server.

use std::process::{Command, Output};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use sea_orm::{ActiveModelTrait as _, IntoActiveModel as _, Set};
use uuid::Uuid;

use super::{
    git_http_router,
    http_test_support::{self as http, HttpResponse},
    resources::test_support::{TestGit, TestInstance, instance},
};
use crate::{
    actions::test_support,
    entity::{repository, repository_protection_rule},
};

const OWNER_TOKEN: &str = "alice-token";
const READER_TOKEN: &str = "bob-token";
const DATE: &str = "2024-01-01T00:00:00Z";

struct Server {
    instance: TestInstance,
    /// A public repository with an empty bare directory behind it.
    project: repository::Model,
    router: Router,
}

async fn server() -> (Server, TestGit) {
    let instance = instance().await;
    let (project, git) = instance.git_repository("project").await;
    http::api_token(&instance.database, &instance.owner, OWNER_TOKEN).await;
    let reader = test_support::owner(&instance.database, "bob").await;
    http::api_token(&instance.database, &reader, READER_TOKEN).await;
    http::collaborator(&instance.database, &project, &reader, "read").await;
    let router = git_http_router().with_state(http::git_http_state(&instance));
    (
        Server {
            instance,
            project,
            router,
        },
        git,
    )
}

fn basic(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let credentials = STANDARD.encode(format!("x-access-token:{token}"));
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Basic {credentials}")).unwrap(),
    );
    headers
}

impl Server {
    async fn info_refs(
        &self,
        repository: &str,
        service: &str,
        headers: &HeaderMap,
    ) -> HttpResponse {
        http::send(
            self.router.clone(),
            http::request(
                "GET",
                &format!("/alice/{repository}/info/refs?service={service}"),
                headers,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
    }

    /// Serves the router on an ephemeral port until the returned guard drops.
    async fn listen(&self) -> Listening {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = self.router.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Listening { port, task }
    }

    /// Waits for the push side effects the handler runs after responding.
    async fn settle(&self) {
        self.instance.state.close_tasks();
        self.instance.state.wait_for_tasks().await;
    }

    async fn protect(&self, pattern: &str, restrict_pushes: bool, block_force_push: bool) {
        let now = Utc::now();
        repository_protection_rule::ActiveModel {
            id: Set(Uuid::new_v4()),
            repository_id: Set(self.project.id),
            kind: Set("branch".to_owned()),
            pattern: Set(pattern.to_owned()),
            block_force_push: Set(block_force_push),
            block_deletion: Set(true),
            block_update: Set(false),
            restrict_pushes: Set(restrict_pushes),
            admins_bypass: Set(false),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&self.instance.database)
        .await
        .unwrap();
    }
}

struct Listening {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Listening {
    fn url(&self, token: &str, repository: &str) -> String {
        format!(
            "http://x-access-token:{token}@127.0.0.1:{}/alice/{repository}",
            self.port
        )
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Runs `git` in `client` without asserting on the outcome.
fn git(client: &TestGit, arguments: &[&str]) -> Output {
    Command::new("git")
        .arg("--git-dir")
        .arg(&client.git_dir)
        .arg("--work-tree")
        .arg(&client.work_tree)
        .args(arguments)
        .current_dir(&client.work_tree)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Gitadel Test")
        .env("GIT_AUTHOR_EMAIL", "gitadel@example.test")
        .env("GIT_COMMITTER_NAME", "Gitadel Test")
        .env("GIT_COMMITTER_EMAIL", "gitadel@example.test")
        .env("GIT_AUTHOR_DATE", DATE)
        .env("GIT_COMMITTER_DATE", DATE)
        .output()
        .expect("run git")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The commit `branch` points at in the server repository, if any.
fn tip(server_git: &TestGit, branch: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(&server_git.git_dir)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[tokio::test]
async fn ref_advertisement_is_public_for_fetches_and_guarded_for_pushes() {
    let (server, _git) = server().await;
    let anonymous = HeaderMap::new();

    let fetch = server
        .info_refs("project.git", "git-upload-pack", &anonymous)
        .await;
    assert_eq!(fetch.status, StatusCode::OK);
    assert_eq!(
        fetch.headers[header::CONTENT_TYPE],
        "application/x-git-upload-pack-advertisement"
    );
    assert!(
        fetch
            .text()
            .starts_with("001e# service=git-upload-pack\n0000")
    );
    assert_eq!(
        fetch.headers[header::CACHE_CONTROL],
        "no-cache, max-age=0, must-revalidate"
    );
    // The `.git` suffix is optional, as `actions/checkout` relies on.
    assert_eq!(
        server
            .info_refs("project", "git-upload-pack", &anonymous)
            .await
            .status,
        StatusCode::OK
    );

    let push = server
        .info_refs("project.git", "git-receive-pack", &anonymous)
        .await;
    assert_eq!(push.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        push.headers[header::WWW_AUTHENTICATE],
        "Basic realm=\"Gitadel\""
    );
    assert_eq!(
        server
            .info_refs("project.git", "git-receive-pack", &basic("bogus"))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    // A read-only collaborator learns no more than a stranger would.
    assert_eq!(
        server
            .info_refs("project.git", "git-receive-pack", &basic(READER_TOKEN))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    for headers in [basic(OWNER_TOKEN), http::bearer(OWNER_TOKEN)] {
        let push = server
            .info_refs("project.git", "git-receive-pack", &headers)
            .await;
        assert_eq!(push.status, StatusCode::OK);
        assert_eq!(
            push.headers[header::CONTENT_TYPE],
            "application/x-git-receive-pack-advertisement"
        );
        assert!(
            push.text()
                .starts_with("001f# service=git-receive-pack\n0000")
        );
    }

    assert_eq!(
        server
            .info_refs("project.git", "git-mirror", &basic(OWNER_TOKEN))
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    let mut archived = server.project.clone().into_active_model();
    archived.archived_at = Set(Some(Utc::now()));
    archived.update(&server.instance.database).await.unwrap();
    assert_eq!(
        server
            .info_refs("project.git", "git-receive-pack", &basic(OWNER_TOKEN))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        server
            .info_refs("project.git", "git-upload-pack", &anonymous)
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn private_repositories_hide_from_anonymous_fetches_but_not_from_members() {
    let (server, _git) = server().await;
    let vault =
        test_support::repository(&server.instance.database, &server.instance.owner, "vault").await;
    let (_, git) = server.instance.git_repository("vault-objects").await;
    // Give the private row a bare directory by pointing it at one.
    std::fs::rename(&git.git_dir, server.instance.state.repository_path(&vault)).unwrap();

    assert_eq!(
        server
            .info_refs("vault.git", "git-upload-pack", &HeaderMap::new())
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .info_refs("vault.git", "git-upload-pack", &basic(READER_TOKEN))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .info_refs("missing.git", "git-upload-pack", &basic(READER_TOKEN))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .info_refs("vault.git", "git-upload-pack", &basic(OWNER_TOKEN))
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_push_over_http_creates_the_branch_and_a_clone_sees_it() {
    let (server, server_git) = server().await;
    let (_, client) = server.instance.git_repository("client").await;
    let commit = client.commit_file("README.md", b"hello over http", DATE);
    let listening = server.listen().await;
    let url = listening.url(OWNER_TOKEN, "project.git");

    assert_eq!(tip(&server_git, "main"), None);
    let push = git(&client, &["push", &url, "HEAD:refs/heads/main"]);
    assert!(push.status.success(), "push failed: {}", stderr(&push));
    assert_eq!(tip(&server_git, "main").as_deref(), Some(commit.as_str()));

    // A read-only collaborator cannot push, and a stranger is challenged.
    let refused = git(
        &client,
        &[
            "push",
            &listening.url(READER_TOKEN, "project.git"),
            "HEAD:refs/heads/feature",
        ],
    );
    assert!(!refused.status.success());
    assert_eq!(tip(&server_git, "feature"), None);
    let anonymous = format!("http://127.0.0.1:{}/alice/project.git", listening.port);
    let challenged = git(&client, &["push", &anonymous, "HEAD:refs/heads/feature"]);
    assert!(!challenged.status.success());
    assert!(
        stderr(&challenged).contains("Authentication failed")
            || stderr(&challenged).contains("terminal prompts disabled"),
        "{}",
        stderr(&challenged)
    );

    // Anyone can fetch the public repository back.
    let (_, fetcher) = server.instance.git_repository("fetcher").await;
    let fetched = git(
        &fetcher,
        &["fetch", &anonymous, "refs/heads/main:refs/heads/copy"],
    );
    assert!(fetched.status.success(), "{}", stderr(&fetched));
    assert_eq!(tip(&fetcher, "copy").as_deref(), Some(commit.as_str()));

    drop(listening);
    server.settle().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_branches_reject_pushes_over_http() {
    let (server, server_git) = server().await;
    let (_, client) = server.instance.git_repository("client").await;
    let first = client.commit_file("README.md", b"first", DATE);
    let listening = server.listen().await;
    let url = listening.url(OWNER_TOKEN, "project.git");

    // Nobody is on the allow list, and admins do not bypass this rule.
    server.protect("main", true, true).await;
    let rejected = git(&client, &["push", &url, "HEAD:refs/heads/main"]);
    assert!(!rejected.status.success());
    assert!(
        stderr(&rejected)
            .contains("protected branch `main`: you are not allowed to push to this ref"),
        "{}",
        stderr(&rejected)
    );
    assert_eq!(tip(&server_git, "main"), None);
    // Other refs in the same repository are unaffected.
    let scratch = git(&client, &["push", &url, "HEAD:refs/heads/scratch"]);
    assert!(scratch.status.success(), "{}", stderr(&scratch));
    assert_eq!(tip(&server_git, "scratch").as_deref(), Some(first.as_str()));

    // With only force-push protection, a fast-forward lands and a rewrite
    // of history does not.
    server.protect("release", false, true).await;
    let release = git(&client, &["push", &url, "HEAD:refs/heads/release"]);
    assert!(release.status.success(), "{}", stderr(&release));
    let second = client.commit_file("CHANGELOG.md", b"second", DATE);
    let forward = git(&client, &["push", &url, "HEAD:refs/heads/release"]);
    assert!(forward.status.success(), "{}", stderr(&forward));
    assert_eq!(
        tip(&server_git, "release").as_deref(),
        Some(second.as_str())
    );
    client.run(DATE, &["checkout", "--quiet", "--orphan", "rewrite"]);
    let unrelated = client.commit_file("OTHER.md", b"unrelated history", DATE);
    assert_ne!(unrelated, second);
    let forced = git(
        &client,
        &["push", "--force", &url, "HEAD:refs/heads/release"],
    );
    assert!(!forced.status.success());
    assert!(
        stderr(&forced).contains("protected branch `release`: force-push is not allowed"),
        "{}",
        stderr(&forced)
    );
    assert_eq!(
        tip(&server_git, "release").as_deref(),
        Some(second.as_str())
    );
    // The unprotected branch still takes the rewrite.
    let forced_scratch = git(
        &client,
        &["push", "--force", &url, "HEAD:refs/heads/scratch"],
    );
    assert!(
        forced_scratch.status.success(),
        "{}",
        stderr(&forced_scratch)
    );
    assert_eq!(
        tip(&server_git, "scratch").as_deref(),
        Some(unrelated.as_str())
    );

    drop(listening);
    server.settle().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pushing_to_a_missing_repository_creates_it_only_in_an_owned_namespace() {
    let (server, _git) = server().await;
    let (_, client) = server.instance.git_repository("client").await;
    let commit = client.commit_file("README.md", b"fresh", DATE);
    let listening = server.listen().await;

    let created = git(
        &client,
        &[
            "push",
            &listening.url(OWNER_TOKEN, "fresh.git"),
            "HEAD:refs/heads/main",
        ],
    );
    assert!(created.status.success(), "{}", stderr(&created));
    let fresh = server.instance.state.find("alice", "fresh").await.unwrap();
    assert_eq!(
        fresh.visibility,
        server.instance.owner.default_repository_visibility
    );
    let fresh_git = TestGit {
        git_dir: server.instance.state.repository_path(&fresh),
        work_tree: server.instance.root.join("unused"),
    };
    assert_eq!(tip(&fresh_git, "main").as_deref(), Some(commit.as_str()));

    // Bob's namespace is not Alice's to create repositories in.
    let foreign = format!(
        "http://x-access-token:{OWNER_TOKEN}@127.0.0.1:{}/bob/intruder.git",
        listening.port
    );
    let refused = git(&client, &["push", &foreign, "HEAD:refs/heads/main"]);
    assert!(!refused.status.success());
    assert!(server.instance.state.find("bob", "intruder").await.is_err());

    drop(listening);
    server.settle().await;
}
