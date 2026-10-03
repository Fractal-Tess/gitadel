//! Git LFS over the HTTP router: batch authorization, object transfer, token
//! scoping, and locks.

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
};
use sea_orm::{ColumnTrait as _, EntityTrait as _, QueryFilter as _};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    GitHttpState,
    http_test_support::{self as http, HttpResponse},
    lfs,
    resources::test_support::{TestInstance, instance},
};
use crate::{
    actions::test_support,
    entity::{lfs_lock, lfs_object, repository},
    identity::SCOPE_READ,
};

const OWNER_TOKEN: &str = "alice-token";
const OWNER_READ_ONLY_TOKEN: &str = "alice-read-only-token";
const READER_TOKEN: &str = "bob-token";
const WRITER_TOKEN: &str = "carol-token";
const LFS_MEDIA_TYPE: &str = "application/vnd.git-lfs+json";

fn oid_of(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn authorization(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
    headers
}

struct Lfs {
    instance: TestInstance,
    /// A public repository owned by `alice`.
    project: repository::Model,
    state: GitHttpState,
}

async fn setup() -> Lfs {
    let instance = instance().await;
    let project = instance.repository("project").await;
    let state = http::git_http_state(&instance);
    http::api_token(&instance.database, &instance.owner, OWNER_TOKEN).await;
    http::scoped_api_token(
        &instance.database,
        &instance.owner,
        OWNER_READ_ONLY_TOKEN,
        SCOPE_READ,
    )
    .await;
    let reader = test_support::owner(&instance.database, "bob").await;
    http::api_token(&instance.database, &reader, READER_TOKEN).await;
    http::collaborator(&instance.database, &project, &reader, "read").await;
    let writer = test_support::owner(&instance.database, "carol").await;
    http::api_token(&instance.database, &writer, WRITER_TOKEN).await;
    http::collaborator(&instance.database, &project, &writer, "write").await;
    Lfs {
        instance,
        project,
        state,
    }
}

impl Lfs {
    fn router(&self) -> Router {
        lfs::router().with_state(self.state.clone())
    }

    async fn batch(
        &self,
        repository: &str,
        headers: &HeaderMap,
        operation: &str,
        oid: &str,
    ) -> HttpResponse {
        let body = json!({
            "operation": operation,
            "transfers": ["basic"],
            "objects": [{ "oid": oid, "size": 3 }],
        });
        http::send(
            self.router(),
            http::request(
                "POST",
                &format!("/alice/{repository}.git/info/lfs/objects/batch"),
                headers,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        )
        .await
    }

    async fn upload(
        &self,
        repository: &str,
        headers: &HeaderMap,
        oid: &str,
        content: &[u8],
    ) -> HttpResponse {
        http::send(
            self.router(),
            http::request(
                "PUT",
                &format!("/alice/{repository}.git/info/lfs/objects/{oid}"),
                headers,
            )
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(content.to_vec()))
            .unwrap(),
        )
        .await
    }

    async fn download(&self, repository: &str, headers: &HeaderMap, oid: &str) -> HttpResponse {
        http::send(
            self.router(),
            http::request(
                "GET",
                &format!("/alice/{repository}.git/info/lfs/objects/{oid}"),
                headers,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
    }

    async fn lock(&self, repository: &str, headers: &HeaderMap, path: &str) -> HttpResponse {
        http::send(
            self.router(),
            http::request(
                "POST",
                &format!("/alice/{repository}.git/info/lfs/locks"),
                headers,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "path": path }).to_string()))
            .unwrap(),
        )
        .await
    }

    async fn locks(&self, headers: &HeaderMap, query: &str) -> HttpResponse {
        http::send(
            self.router(),
            http::request(
                "GET",
                &format!("/alice/project.git/info/lfs/locks{query}"),
                headers,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
    }

    async fn verify_locks(&self, headers: &HeaderMap) -> HttpResponse {
        http::send(
            self.router(),
            http::request("POST", "/alice/project.git/info/lfs/locks/verify", headers)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
    }

    async fn unlock(&self, headers: &HeaderMap, id: &str, force: bool) -> HttpResponse {
        http::send(
            self.router(),
            http::request(
                "POST",
                &format!("/alice/project.git/info/lfs/locks/{id}/unlock"),
                headers,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "force": force }).to_string()))
            .unwrap(),
        )
        .await
    }

    async fn stored_objects(&self, repository: &repository::Model) -> Vec<lfs_object::Model> {
        lfs_object::Entity::find()
            .filter(lfs_object::Column::RepositoryId.eq(repository.id))
            .all(&self.instance.database)
            .await
            .unwrap()
    }

    async fn stored_locks(&self, repository: &repository::Model) -> Vec<lfs_lock::Model> {
        lfs_lock::Entity::find()
            .filter(lfs_lock::Column::RepositoryId.eq(repository.id))
            .all(&self.instance.database)
            .await
            .unwrap()
    }
}

fn first_object(response: &HttpResponse) -> Value {
    assert_eq!(
        response.headers[header::CONTENT_TYPE],
        LFS_MEDIA_TYPE,
        "batch responses use the LFS media type"
    );
    response.json()["objects"][0].clone()
}

#[tokio::test]
async fn anonymous_clients_read_public_objects_but_cannot_upload() {
    let lfs = setup().await;
    let content = b"anonymous read";
    let oid = oid_of(content);
    let anonymous = HeaderMap::new();

    let missing = lfs.batch("project", &anonymous, "download", &oid).await;
    assert_eq!(missing.status, StatusCode::OK);
    let object = first_object(&missing);
    assert_eq!(object["error"]["code"], 404);
    assert!(object.get("actions").is_none());

    let upload = lfs.batch("project", &anonymous, "upload", &oid).await;
    assert_eq!(upload.status, StatusCode::UNAUTHORIZED);
    let put = lfs.upload("project", &anonymous, &oid, content).await;
    assert_eq!(put.status, StatusCode::UNAUTHORIZED);
    assert!(lfs.stored_objects(&lfs.project).await.is_empty());

    let owner = http::bearer(OWNER_TOKEN);
    assert_eq!(
        lfs.upload("project", &owner, &oid, content).await.status,
        StatusCode::OK
    );

    let present = lfs.batch("project", &anonymous, "download", &oid).await;
    let object = first_object(&present);
    assert!(object.get("error").is_none());
    assert_eq!(
        object["actions"]["download"]["href"],
        format!("https://gitadel.test/alice/project.git/info/lfs/objects/{oid}")
    );
    // Nobody authenticated, so there is no token to hand back.
    assert!(object["actions"]["download"].get("header").is_none());
    let download = lfs.download("project", &anonymous, &oid).await;
    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(download.body.as_ref(), content);
    assert_eq!(
        download.headers[header::CONTENT_LENGTH],
        content.len().to_string()
    );
}

#[tokio::test]
async fn batch_upload_follows_repository_write_access_and_token_scope() {
    let lfs = setup().await;
    let oid = oid_of(b"scoped");

    // A read-only collaborator is refused exactly like a stranger: the
    // repository's existence is never confirmed to someone who cannot write.
    let reader = lfs
        .batch("project", &http::bearer(READER_TOKEN), "upload", &oid)
        .await;
    assert_eq!(reader.status, StatusCode::NOT_FOUND);

    let read_only = lfs
        .batch(
            "project",
            &http::bearer(OWNER_READ_ONLY_TOKEN),
            "upload",
            &oid,
        )
        .await;
    assert_eq!(read_only.status, StatusCode::FORBIDDEN);
    assert_eq!(
        read_only.error_message(),
        "The API token does not have the required scope."
    );

    let writer = lfs
        .batch("project", &http::bearer(WRITER_TOKEN), "upload", &oid)
        .await;
    assert_eq!(writer.status, StatusCode::OK);
    let object = first_object(&writer);
    assert_eq!(
        object["actions"]["upload"]["href"],
        format!("https://gitadel.test/alice/project.git/info/lfs/objects/{oid}")
    );
    assert!(
        object["actions"]["upload"]["header"]["Authorization"]
            .as_str()
            .unwrap()
            .starts_with("Bearer ")
    );
    assert_eq!(object["actions"]["upload"]["expires_in"], 900);

    let unsupported = lfs
        .batch("project", &http::bearer(WRITER_TOKEN), "verify", &oid)
        .await;
    assert_eq!(unsupported.status, StatusCode::BAD_REQUEST);

    // Once the object exists an upload batch offers nothing, and no error.
    assert_eq!(
        lfs.upload("project", &http::bearer(WRITER_TOKEN), &oid, b"scoped")
            .await
            .status,
        StatusCode::OK
    );
    let existing = lfs
        .batch("project", &http::bearer(WRITER_TOKEN), "upload", &oid)
        .await;
    let object = first_object(&existing);
    assert!(object.get("actions").is_none());
    assert!(object.get("error").is_none());
}

#[tokio::test]
async fn uploads_are_verified_against_their_identifier() {
    let lfs = setup().await;
    let owner = http::bearer(OWNER_TOKEN);
    let content = b"verified content";
    let oid = oid_of(content);

    for wrong in [
        &b"verified"[..],
        &b"verified content plus trailing bytes"[..],
    ] {
        let response = lfs.upload("project", &owner, &oid, wrong).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            response.error_message(),
            "The uploaded LFS object does not match its identifier."
        );
    }
    assert!(lfs.stored_objects(&lfs.project).await.is_empty());
    assert_eq!(
        lfs.download("project", &owner, &oid).await.status,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        lfs.upload("project", &owner, &oid, content).await.status,
        StatusCode::OK
    );
    let stored = lfs.stored_objects(&lfs.project).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].oid, oid);
    assert_eq!(stored[0].size, content.len() as i64);

    // Re-uploading an existing object is idempotent.
    assert_eq!(
        lfs.upload("project", &owner, &oid, content).await.status,
        StatusCode::OK
    );
    assert_eq!(lfs.stored_objects(&lfs.project).await.len(), 1);

    let download = lfs.download("project", &owner, &oid).await;
    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(download.body.as_ref(), content);
}

#[tokio::test]
async fn object_identifiers_must_be_lowercase_sha256_hex() {
    let lfs = setup().await;
    let owner = http::bearer(OWNER_TOKEN);
    let uppercase = oid_of(b"upper").to_uppercase();
    let short = &oid_of(b"short")[..63];

    for invalid in [
        "..%2F..%2Fescape",
        uppercase.as_str(),
        short,
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg",
    ] {
        let put = lfs.upload("project", &owner, invalid, b"x").await;
        assert_eq!(put.status, StatusCode::BAD_REQUEST, "PUT {invalid}");
        assert_eq!(put.error_message(), "The LFS object identifier is invalid.");
        let get = lfs.download("project", &owner, invalid).await;
        assert_eq!(get.status, StatusCode::NOT_FOUND, "GET {invalid}");
        let batch = lfs.batch("project", &owner, "upload", invalid).await;
        assert_eq!(batch.status, StatusCode::OK);
        let object = first_object(&batch);
        assert_eq!(object["error"]["code"], 422, "batch {invalid}");
        assert!(object.get("actions").is_none());
    }
    assert!(lfs.stored_objects(&lfs.project).await.is_empty());
    assert!(
        !lfs.instance.root.join("escape").exists(),
        "nothing may be written outside the LFS root"
    );
}

#[tokio::test]
async fn lfs_tokens_are_bound_to_one_repository_and_permission() {
    let lfs = setup().await;
    let owner = http::bearer(OWNER_TOKEN);
    let other = lfs.instance.repository("other").await;
    let vault =
        test_support::repository(&lfs.instance.database, &lfs.instance.owner, "vault").await;
    assert_eq!(vault.visibility, "private");
    let content = b"token scoped";
    let oid = oid_of(content);

    let batch = lfs.batch("project", &owner, "upload", &oid).await;
    let write_token = first_object(&batch)["actions"]["upload"]["header"]["Authorization"]
        .as_str()
        .unwrap()
        .to_owned();
    let write = authorization(&write_token);

    // The token stands in for the owner on the repository it was issued for.
    assert_eq!(
        lfs.upload("other", &write, &oid, content).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(lfs.stored_objects(&other).await.is_empty());
    assert_eq!(
        lfs.upload("project", &write, &oid, content).await.status,
        StatusCode::OK
    );
    assert_eq!(lfs.stored_objects(&lfs.project).await.len(), 1);

    // A private repository hands an authenticated reader a read token.
    assert_eq!(
        lfs.upload("vault", &owner, &oid, content).await.status,
        StatusCode::OK
    );
    let batch = lfs.batch("vault", &owner, "download", &oid).await;
    let read_token = first_object(&batch)["actions"]["download"]["header"]["Authorization"]
        .as_str()
        .unwrap()
        .to_owned();
    let read = authorization(&read_token);
    assert_eq!(
        lfs.download("vault", &read, &oid).await.status,
        StatusCode::OK
    );
    // Not for writing, and not for another private repository.
    assert_eq!(
        lfs.upload("vault", &read, &oid_of(b"new"), b"new")
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        lfs.lock("vault", &read, "locked.bin").await.status,
        StatusCode::UNAUTHORIZED
    );
    let secret =
        test_support::repository(&lfs.instance.database, &lfs.instance.owner, "secret").await;
    assert_eq!(
        lfs.download("secret", &read, &oid).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(lfs.stored_objects(&secret).await.is_empty());
}

#[tokio::test]
async fn locks_belong_to_their_creator_and_repository() {
    let lfs = setup().await;
    let owner = http::bearer(OWNER_TOKEN);
    let writer = http::bearer(WRITER_TOKEN);
    let other = lfs.instance.repository("other").await;

    assert_eq!(
        lfs.lock("project", &HeaderMap::new(), "assets/model.bin")
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    for invalid in ["", "../escape", "/absolute", "./relative"] {
        let response = lfs.lock("project", &owner, invalid).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{invalid:?}");
    }
    assert!(lfs.stored_locks(&lfs.project).await.is_empty());

    let created = lfs.lock("project", &owner, "assets/model.bin").await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.headers[header::CONTENT_TYPE], LFS_MEDIA_TYPE);
    let lock = created.json()["lock"].clone();
    assert_eq!(lock["path"], "assets/model.bin");
    assert_eq!(lock["owner"]["name"], "alice");
    let id = lock["id"].as_str().unwrap().to_owned();

    let duplicate = lfs.lock("project", &writer, "assets/model.bin").await;
    assert_eq!(duplicate.status, StatusCode::CONFLICT);
    assert_eq!(duplicate.json()["message"], "already created lock");
    assert_eq!(duplicate.json()["lock"]["owner"]["name"], "alice");
    assert_eq!(lfs.stored_locks(&lfs.project).await.len(), 1);

    // Locks are per repository: the same path is free elsewhere.
    assert_eq!(
        lfs.lock("other", &owner, "assets/model.bin").await.status,
        StatusCode::CREATED
    );
    assert_eq!(lfs.stored_locks(&other).await.len(), 1);

    let listed = lfs.locks(&HeaderMap::new(), "").await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.json()["locks"].as_array().unwrap().len(), 1);
    let by_path = lfs.locks(&owner, "?path=assets/model.bin").await;
    assert_eq!(by_path.json()["locks"][0]["id"], id.as_str());
    let by_other_path = lfs.locks(&owner, "?path=missing.bin").await;
    assert!(by_other_path.json()["locks"].as_array().unwrap().is_empty());

    let ours = lfs.verify_locks(&owner).await.json();
    assert_eq!(ours["ours"][0]["id"], id.as_str());
    assert!(ours["theirs"].as_array().unwrap().is_empty());
    let theirs = lfs.verify_locks(&writer).await.json();
    assert!(theirs["ours"].as_array().unwrap().is_empty());
    assert_eq!(theirs["theirs"][0]["id"], id.as_str());
    assert_eq!(
        lfs.verify_locks(&HeaderMap::new()).await.status,
        StatusCode::UNAUTHORIZED
    );

    // Only the creator unlocks without force; force needs manage access,
    // which a write collaborator does not have.
    let refused = lfs.unlock(&writer, &id, false).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(
        refused.error_message(),
        "Only the lock owner can unlock this path without force."
    );
    assert_eq!(
        lfs.unlock(&writer, &id, true).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(lfs.stored_locks(&lfs.project).await.len(), 1);
    let unlocked = lfs.unlock(&owner, &id, false).await;
    assert_eq!(unlocked.status, StatusCode::OK);
    assert_eq!(unlocked.json()["lock"]["id"], id.as_str());
    assert!(lfs.stored_locks(&lfs.project).await.is_empty());
    assert_eq!(
        lfs.unlock(&owner, &id, false).await.status,
        StatusCode::NOT_FOUND
    );
    // The other repository's lock was never touched.
    assert_eq!(lfs.stored_locks(&other).await.len(), 1);
}

#[tokio::test]
async fn lock_listing_pages_with_a_cursor() {
    let lfs = setup().await;
    let owner = http::bearer(OWNER_TOKEN);
    let mut ids = Vec::new();
    for path in ["a.bin", "b.bin", "c.bin"] {
        let created = lfs.lock("project", &owner, path).await;
        assert_eq!(created.status, StatusCode::CREATED);
        ids.push(created.json()["lock"]["id"].as_str().unwrap().to_owned());
    }

    let first = lfs.locks(&owner, "?limit=2").await.json();
    let page = first["locks"].as_array().unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0]["id"], ids[0].as_str());
    assert_eq!(page[1]["id"], ids[1].as_str());
    let cursor = first["next_cursor"].as_str().unwrap();
    assert_eq!(cursor, ids[1]);

    let second = lfs
        .locks(&owner, &format!("?limit=2&cursor={cursor}"))
        .await
        .json();
    let page = second["locks"].as_array().unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0]["id"], ids[2].as_str());
    assert!(second.get("next_cursor").is_none());

    let invalid = lfs
        .locks(&owner, &format!("?cursor={}", uuid::Uuid::new_v4()))
        .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid.error_message(), "The lock cursor is invalid.");
}
