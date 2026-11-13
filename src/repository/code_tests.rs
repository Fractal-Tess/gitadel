//! Exercise browsing and website authorization through the real HTTP router.
use super::{
    http_test_support::{api_token, bearer, request, scoped_api_token, send},
    resources::test_support::instance,
};
use crate::identity::SCOPE_READ;
use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
};
use sea_orm::{ActiveModelTrait, IntoActiveModel, Set};
use serde_json::json;

#[tokio::test]
async fn code_endpoints_pin_ranges_attribute_edits_and_filter_history() {
    let instance = instance().await;
    let (repository, git) = instance.git_repository("code").await;
    let first = git.commit_file(
        "main.rs",
        b"fn main() {\n    first();\n}\n",
        "2026-01-01T00:00:00Z",
    );
    let second = git.commit_file(
        "main.rs",
        b"fn main() {\n    second();\n}\n",
        "2026-01-02T00:00:00Z",
    );
    let third = git.commit_file("other.txt", b"unrelated\n", "2026-01-03T00:00:00Z");
    let router = super::router().with_state(instance.state.clone());
    let prefix = "/repositories/alice/code";
    let get = |suffix: String| {
        request("GET", &format!("{prefix}{suffix}"), &HeaderMap::new())
            .body(Body::empty())
            .unwrap()
    };
    let snippet = send(
        router.clone(),
        get(format!("/snippet?rev={first}&path=main.rs&start=2&end=2")),
    )
    .await;
    assert_eq!(snippet.status, StatusCode::OK);
    assert_eq!(snippet.json()["content"], "    first();");
    assert_eq!(snippet.json()["line_count"], 3);
    let url = url::Url::parse(snippet.json()["permalink"].as_str().unwrap()).unwrap();
    assert_eq!(
        url.query_pairs().find(|(key, _)| key == "rev").unwrap().1,
        first
    );
    assert_eq!(url.fragment(), Some("L2"));
    let invalid = send(router.clone(), get("/snippet?path=main.rs&start=0".into())).await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    let symbols = send(router.clone(), get("/symbols?path=main.rs".into())).await;
    assert_eq!(symbols.json()["symbols"][0]["name"], "main");
    let blame = send(router.clone(), get("/blame?path=main.rs".into())).await;
    assert_eq!(blame.status, StatusCode::OK, "{}", blame.text());
    assert_eq!(blame.json()["commit_oid"], third);
    assert_eq!(blame.json()["lines"][0]["commit_oid"], first);
    assert_eq!(blame.json()["lines"][1]["commit_oid"], second);
    let history = send(
        router.clone(),
        get("/history?path=main.rs&per_page=1".into()),
    )
    .await;
    assert_eq!(history.json()["commits"][0]["oid"], second);
    assert_eq!(history.json()["has_next"], true);
    let history = send(
        router.clone(),
        get("/history?path=main.rs&per_page=1&page=2".into()),
    )
    .await;
    assert_eq!(history.json()["commits"][0]["oid"], first);
    assert_eq!(history.json()["has_next"], false);
    let mut private = repository.into_active_model();
    private.visibility = Set("private".into());
    private.update(&instance.database).await.unwrap();
    for endpoint in ["snippet", "symbols", "blame"] {
        assert_eq!(
            send(router.clone(), get(format!("/{endpoint}?path=main.rs")))
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn website_lists_preserve_order_validate_urls_and_require_management() {
    let instance = instance().await;
    let repository = instance.repository("sites").await;
    api_token(&instance.database, &instance.owner, "owner-write").await;
    scoped_api_token(
        &instance.database,
        &instance.owner,
        "owner-read",
        SCOPE_READ,
    )
    .await;
    let router = super::router().with_state(instance.state.clone());
    let path = "/repositories/alice/sites/websites";
    let update = |token: &str, body: serde_json::Value| {
        request("PUT", path, &bearer(token))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let links = json!({"websites":[{"label":"Demo","url":"https://example.com/demo"},{"label":"Docs","url":"https://docs.example.com"}]});
    assert_eq!(
        send(router.clone(), update("owner-read", links.clone()))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let saved = send(router.clone(), update("owner-write", links)).await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
    assert_eq!(saved.json()[0]["label"], "Demo");
    let read = || {
        request("GET", path, &HeaderMap::new())
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        send(router.clone(), read()).await.json()[1]["url"],
        "https://docs.example.com/"
    );
    let invalid = send(
        router.clone(),
        update(
            "owner-write",
            json!({"websites":[{"label":"Bad","url":"javascript:alert(1)"}]}),
        ),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        send(router.clone(), read())
            .await
            .json()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut private = repository.into_active_model();
    private.visibility = Set("private".into());
    private.update(&instance.database).await.unwrap();
    assert_eq!(
        send(router.clone(), read()).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            router.clone(),
            update("owner-write", json!({"websites":[]}))
        )
        .await
        .status,
        StatusCode::OK
    );
}
