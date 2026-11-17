//! Exercise the provider as a dependency, without its unit-test configuration.

use gitadel_integrations::{Credential, DokployIntegration, Integration, provider};

#[test]
fn public_result_schema_names_remain_stable() {
    use gitadel_integrations::{Catalog, CreatedEnvironment, DeploymentStatus, ProjectSummary};
    use utoipa::ToSchema;

    assert_eq!(Catalog::name(), "IntegrationRemoteCatalog");
    assert_eq!(ProjectSummary::name(), "IntegrationProject");
    assert_eq!(CreatedEnvironment::name(), "CreatedEnvironment");
    assert_eq!(DeploymentStatus::name(), "DeploymentStatus");
}

#[tokio::test]
async fn production_registry_refuses_reserved_addresses() {
    let integration = provider("dokploy").unwrap().integration;
    for url in [
        "http://127.0.0.1:9",
        "http://[::1]:9",
        "http://169.254.169.254",
        "http://[::ffff:127.0.0.1]:9",
    ] {
        let result = integration
            .test(Credential {
                url,
                api_key: "must-not-be-sent",
                source_id: None,
            })
            .await;
        let error = result.err().expect("reserved addresses must be refused");
        assert!(error.contains("reserved address"), "{url}: {error}");
    }
}

#[tokio::test]
async fn injected_mock_transport_does_not_change_the_production_registry() {
    let app = axum::Router::new()
        .route(
            "/api/user.get",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(headers["x-api-key"], "mock-key");
                axum::Json(serde_json::json!({"email": "owner@example.com"}))
            }),
        )
        .route(
            "/api/gitea.giteaProviders",
            axum::routing::get(|| async { axum::Json(serde_json::json!([])) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let url = format!("http://{address}");
    let credential = Credential {
        url: &url,
        api_key: "mock-key",
        source_id: None,
    };
    let mock = DokployIntegration::with_client_factory(|_| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|error| error.to_string())
    });
    let report = mock.test(credential).await;
    let production = provider("dokploy")
        .unwrap()
        .integration
        .test(credential)
        .await;
    server.abort();

    let report = report.unwrap();
    assert_eq!(report.account.as_deref(), Some("owner@example.com"));
    assert_eq!(report.warnings.len(), 1);
    assert!(production.is_err());
}
