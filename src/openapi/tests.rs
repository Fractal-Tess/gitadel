use super::{document, instance_router};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use tower::ServiceExt;

#[test]
fn registered_contract_has_concrete_json_bodies_and_resolved_references() {
    let value = serde_json::to_value(document()).unwrap();
    let schemas = value["components"]["schemas"].as_object().unwrap();
    let paths = value["paths"].as_object().unwrap();
    let mut count = 0;
    let mut ids = std::collections::BTreeSet::new();
    for (path, item) in paths {
        for (method, operation) in item.as_object().unwrap() {
            if !["get", "post", "put", "patch", "delete"].contains(&method.as_str()) {
                continue;
            }
            count += 1;
            assert!(
                ids.insert(operation["operationId"].as_str().unwrap().to_owned()),
                "duplicate operation id: {path}"
            );
            for response in operation["responses"].as_object().unwrap().values() {
                if let Some(schema) = response.pointer("/content/application~1json/schema") {
                    assert!(
                        schema.get("$ref").is_some()
                            || schema.get("type").is_some()
                            || schema.get("anyOf").is_some()
                            || schema.get("oneOf").is_some()
                            || schema.get("allOf").is_some(),
                        "{method} {path}: unspecified JSON response"
                    );
                }
            }
        }
    }
    assert_eq!(
        count, 294,
        "check additions/removals against the registered routes"
    );
    for reference in value.to_string().split("\"#/components/schemas/").skip(1) {
        let name = &reference[..reference.find('"').unwrap()];
        assert!(schemas.contains_key(name), "unresolved schema {name}");
    }
}

#[test]
fn compatibility_aliases_and_native_transfers_are_described() {
    let value = serde_json::to_value(document()).unwrap();
    let paths = &value["paths"];
    assert!(paths["/repos/{namespace}/{name}/statuses/{sha}"]["get"].is_object());
    assert!(paths["/repos/{namespace}/{name}/commits/{revision}/statuses"]["get"].is_object());
    assert!(paths["/repositories/{namespace}/{name}/source"]["get"]["responses"]["200"]["content"]["application/octet-stream"].is_object());
    assert!(paths["/admin/backups/progress/{operation_id}"]["get"]["responses"]["200"]["content"]["text/event-stream"].is_object());
    assert!(
        paths["/auth/status"]["get"]["security"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn export_is_deterministic_and_matches_the_committed_contract() {
    let first = super::json().unwrap();
    assert_eq!(first, super::json().unwrap());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("api/openapi.json");
    assert_eq!(
        first,
        std::fs::read_to_string(path).expect("run just api-generate")
    );
}

#[tokio::test]
async fn registered_version_and_contract_handlers_return_the_documented_bodies() {
    let router: axum::Router = instance_router().into();
    for path in ["/", "/version", "/changelog", "/openapi.json"] {
        let response = router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        match path {
            "/" => assert_eq!(value["application_version"], env!("CARGO_PKG_VERSION")),
            "/version" => assert!(value["version"].is_string()),
            "/changelog" => assert!(value["rendered_html"].is_string()),
            _ => assert_eq!(value, serde_json::to_value(document()).unwrap()),
        }
    }
}
