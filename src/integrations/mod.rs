//! Integration registry and event contract.
//!
//! Namespace-owned connections and repository-owned settings are storage
//! concerns. Provider modules implement this contract and decide which events
//! and settings they support. Adding a provider requires one registry entry;
//! event dispatch and the HTTP APIs do not contain provider-specific matches.

mod dokploy;

use serde_json::Value;

/// A supported external service and its implementation.
pub struct Provider {
    /// Stable slug used in URLs and stored rows. Never change a published one.
    pub slug: &'static str,
    /// Human-readable name shown in the UI.
    pub name: &'static str,
    /// Short catalog copy explaining what the provider does.
    pub description: &'static str,
    /// Stable icon identifier interpreted by the frontend.
    pub icon: &'static str,
    /// Whether this provider exposes repository-owned setup beyond enablement.
    pub repository_setup: bool,
    /// Whether repository setup requires one exact remote source provider.
    pub source_required: bool,
    pub integration: &'static dyn Integration,
}

pub const DOKPLOY: &str = "dokploy";

pub static PROVIDERS: &[Provider] = &[Provider {
    slug: DOKPLOY,
    name: "Dokploy",
    description: "Deploy Gitadel repositories to applications and Docker Compose services.",
    icon: "dokploy",
    integration: &dokploy::INTEGRATION,
    repository_setup: true,
    source_required: true,
}];

/// Resolves a slug from a request path, so unknown providers cannot be stored.
pub fn provider(slug: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|provider| provider.slug == slug)
}

/// A namespace's connection details for one service.
///
/// Source IDs bind provider-specific repository access to this exact
/// connection. Credentials stay out of event payloads and API responses.
#[derive(Clone, Copy)]
pub struct Credential<'a> {
    pub url: &'a str,
    pub api_key: &'a str,
    pub source_id: Option<&'a str>,
}

/// What a successful credential check learned about the remote.
pub struct TestReport {
    pub account: Option<String>,
    pub warnings: Vec<String>,
}

/// A provider account already configured on the remote service.
pub struct RemoteSource {
    pub id: String,
    pub parent_id: String,
    pub name: String,
    pub ready: bool,
    pub authorization_url: String,
}

/// OAuth client details used to create a remote source provider.
pub struct SourceApplication<'a> {
    pub name: &'a str,
    pub client_id: &'a str,
    pub client_secret: &'a str,
    pub redirect_uri: &'a str,
    pub public_url: &'a str,
    pub internal_url: &'a str,
}

/// The stable remote IDs returned after source provider creation.
pub struct ProvisionedSource {
    pub id: String,
    pub parent_id: String,
    pub authorization_url: String,
}

/// The repository coordinates needed by a provider when configuring a remote
/// source. Persistence IDs and storage paths remain repository concerns.
#[derive(Clone, Copy)]
pub struct RepositoryIdentity<'a> {
    pub namespace: &'a str,
    pub name: &'a str,
}

/// A push delivered to an enabled repository integration.
#[derive(Clone, Copy)]
pub struct PushEvent<'a> {
    pub reference: &'a str,
    pub deleted: bool,
    pub payload: &'a Value,
}

/// A Gitadel event delivered to an enabled repository integration.
///
/// Pushes retain their webhook-compatible payload. Manual deployment is a
/// direct command and deliberately carries no synthetic Git event.
pub enum Event<'a> {
    Push(PushEvent<'a>),
    Manual,
}

/// Provider-owned data needed while handling one event.
pub struct EventContext<'a> {
    pub event: Event<'a>,
    /// Provider-owned JSON stored on the repository integration row.
    pub resource: Option<&'a str>,
    pub credential: Credential<'a>,
}

/// Request to list the resources available through one connection.
pub struct RemoteResourcesRequest<'a> {
    pub credential: Credential<'a>,
}

/// Request to create a remote project.
pub struct CreateProjectRequest<'a> {
    pub credential: Credential<'a>,
    pub name: String,
    pub description: Option<String>,
}

/// Request to create an environment within a remote project.
pub struct CreateEnvironmentRequest<'a> {
    pub credential: Credential<'a>,
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
}

/// Request to link an existing remote resource to a repository.
pub struct LinkRemoteRequest<'a> {
    pub repository: RepositoryIdentity<'a>,
    pub credential: Credential<'a>,
    pub kind: String,
    pub id: String,
    pub branch: String,
    pub repository_path: String,
    pub compose_path: String,
}

/// Request to create and link a remote resource to a repository.
pub struct CreateRemoteRequest<'a> {
    pub repository: RepositoryIdentity<'a>,
    pub credential: Credential<'a>,
    pub kind: String,
    pub name: String,
    pub environment_id: String,
    pub branch: String,
    pub server_id: Option<String>,
    pub repository_path: String,
    pub compose_path: String,
}

/// Request to change remote deployment enablement.
pub struct EnablementRequest<'a> {
    pub credential: Credential<'a>,
    pub kind: String,
    pub id: String,
    pub enabled: bool,
}

/// One external service implementation.
///
/// Event failures are reportable but never fatal to the Git operation that
/// emitted them. Capability metadata controls which management operations the
/// repository API exposes.
#[async_trait::async_trait]
pub trait Integration: Send + Sync {
    async fn test(&self, credential: Credential<'_>) -> Result<TestReport, String>;

    async fn handle_event(&self, context: EventContext<'_>) -> Result<String, String>;

    async fn remote_sources(&self, credential: Credential<'_>)
    -> Result<Vec<RemoteSource>, String>;

    async fn provision_remote_source(
        &self,
        credential: Credential<'_>,
        application: SourceApplication<'_>,
    ) -> Result<ProvisionedSource, String>;

    async fn update_remote_source_internal_url(
        &self,
        credential: Credential<'_>,
        source_id: &str,
        internal_url: &str,
    ) -> Result<(), String>;

    async fn remove_remote_source(
        &self,
        credential: Credential<'_>,
        parent_id: &str,
    ) -> Result<(), String>;

    /// Reject provider-owned repository settings before they reach storage.
    fn validate_repository_settings(
        &self,
        resource: Option<&Value>,
        config: Option<&Value>,
    ) -> Result<(), String>;

    async fn remote_resources(&self, request: RemoteResourcesRequest<'_>) -> Result<Value, String>;

    async fn create_remote_project(
        &self,
        request: CreateProjectRequest<'_>,
    ) -> Result<Value, String>;

    async fn create_remote_environment(
        &self,
        request: CreateEnvironmentRequest<'_>,
    ) -> Result<Value, String>;

    async fn link_remote(&self, request: LinkRemoteRequest<'_>) -> Result<Value, String>;

    async fn create_remote(&self, request: CreateRemoteRequest<'_>) -> Result<Value, String>;

    async fn set_remote_enabled(&self, request: EnablementRequest<'_>) -> Result<(), String>;
}
