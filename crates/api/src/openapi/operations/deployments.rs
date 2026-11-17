//! Deployments: namespace Dokploy connections and repository deployments.

use crate::identity::integrations::{
    ConfigureSourceRequest, CreateIntegrationRequest, IntegrationCredentialResponse,
    IntegrationResponse, IntegrationTestResponse, NamespaceIntegrationsResponse,
    SourceCheckResponse, SourceConnectionResponse, TestIntegrationRequest,
    UpdateIntegrationRequest,
};
use crate::openapi::{Body, Method, Operation, json};
use crate::repository::integrations::{
    CreateRepositoryIntegrationRequest, CreateResourceMutation, DeployResponse,
    DeploymentSuggestionsResponse, EnvironmentMutation, LinkMutation, ProjectMutation,
    RepositoryIntegrationResponse, RepositoryIntegrationsResponse, SetupDeploymentResponse,
    SetupMutation, UpdateRepositoryIntegrationRequest,
};

const TAG: &str = "Deployments";

pub(super) const OPERATIONS: &[Operation] = &[
    // Namespace connections, owned by the namespace's owners.
    Operation::new(
        Method::Get,
        "/namespaces/{slug}/integrations",
        TAG,
        "List the namespace's connections and available providers",
    )
    .response(json::<NamespaceIntegrationsResponse>()),
    Operation::new(Method::Post, "/namespaces/{slug}/integrations", TAG, "Add a connection")
        .request(json::<CreateIntegrationRequest>())
        .response(json::<IntegrationResponse>())
        .created(),
    Operation::new(
        Method::Post,
        "/namespaces/{slug}/integrations/test",
        TAG,
        "Test credentials before saving a connection",
    )
    .request(json::<TestIntegrationRequest>())
    .response(json::<IntegrationTestResponse>()),
    Operation::new(
        Method::Put,
        "/namespaces/{slug}/integrations/{id}",
        TAG,
        "Update a connection",
    )
    .request(json::<UpdateIntegrationRequest>())
    .response(json::<IntegrationResponse>()),
    Operation::new(
        Method::Delete,
        "/namespaces/{slug}/integrations/{id}",
        TAG,
        "Remove a connection and the provider Gitadel created for it",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Get,
        "/namespaces/{slug}/integrations/{id}/credential",
        TAG,
        "Reveal a connection's stored API key",
    )
    .response(json::<IntegrationCredentialResponse>()),
    Operation::new(
        Method::Post,
        "/namespaces/{slug}/integrations/{id}/test",
        TAG,
        "Test a saved connection",
    )
    .request(Body::Empty)
    .response(json::<IntegrationTestResponse>()),
    Operation::new(
        Method::Get,
        "/namespaces/{slug}/integrations/{id}/source",
        TAG,
        "Show the connection's repository source and the providers it could use",
    )
    .response(json::<SourceConnectionResponse>()),
    Operation::new(
        Method::Post,
        "/namespaces/{slug}/integrations/{id}/source",
        TAG,
        "Connect a repository source, creating a provider with Gitadel-issued tokens or binding an existing one",
    )
    .request(json::<ConfigureSourceRequest>())
    .response(json::<SourceConnectionResponse>())
    .created(),
    Operation::new(
        Method::Delete,
        "/namespaces/{slug}/integrations/{id}/source",
        TAG,
        "Disconnect the repository source",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Post,
        "/namespaces/{slug}/integrations/{id}/source/verify",
        TAG,
        "Check that the remote reaches Gitadel through the source, trying other addresses if needed",
    )
    .request(Body::Empty)
    .response(json::<SourceCheckResponse>()),
    // Repository deployments, managed by repository managers.
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/deploy/suggestions",
        TAG,
        "Suggest builds from the default branch's Dockerfiles and Compose files",
    )
    .response(json::<DeploymentSuggestionsResponse>()),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/integrations",
        TAG,
        "List the repository's integrations and the connections it could use",
    )
    .response(json::<RepositoryIntegrationsResponse>()),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/integrations",
        TAG,
        "Add an integration through a ready connection",
    )
    .request(json::<CreateRepositoryIntegrationRequest>())
    .response(json::<RepositoryIntegrationResponse>()),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/integrations/{id}",
        TAG,
        "Get an integration",
    )
    .response(json::<RepositoryIntegrationResponse>()),
    Operation::new(
        Method::Put,
        "/repos/{namespace}/{name}/integrations/{id}",
        TAG,
        "Rename an integration, toggle deploy on push, or unlink its resource",
    )
    .request(json::<UpdateRepositoryIntegrationRequest>())
    .response(json::<RepositoryIntegrationResponse>()),
    Operation::new(
        Method::Delete,
        "/repos/{namespace}/{name}/integrations/{id}",
        TAG,
        "Remove an integration; the remote resource stays",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/integrations/{id}/setup",
        TAG,
        "Create, configure, link, and optionally deploy a resource in one step",
    )
    .request(json::<SetupMutation>())
    .response(json::<SetupDeploymentResponse>()),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/integrations/{id}/suggestions",
        TAG,
        "Suggest builds for this integration",
    )
    .response(json::<DeploymentSuggestionsResponse>()),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/integrations/{id}/deploy",
        TAG,
        "Deploy the linked resource now",
    )
    .request(Body::Empty)
    .response(json::<DeployResponse>()),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/integrations/{id}/deployment",
        TAG,
        "Get the latest deployment, with the log's end when it failed",
    ),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/integrations/{id}/resources",
        TAG,
        "List the remote projects, environments, resources, and servers",
    ),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/integrations/{id}/resources",
        TAG,
        "Create a remote resource and link it",
    )
    .request(json::<CreateResourceMutation>())
    .response(json::<RepositoryIntegrationResponse>()),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/integrations/{id}/resources/link",
        TAG,
        "Link an existing remote resource",
    )
    .request(json::<LinkMutation>())
    .response(json::<RepositoryIntegrationResponse>()),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/integrations/{id}/projects",
        TAG,
        "Create a remote project",
    )
    .request(json::<ProjectMutation>()),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/integrations/{id}/environments",
        TAG,
        "Create a remote environment, or select one that exists",
    )
    .request(json::<EnvironmentMutation>()),
];
