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
    /// The forge URL the remote provider was configured with, when known.
    pub url: Option<String>,
}

impl RemoteSource {
    /// Whether this provider was configured for the Gitadel at `public_url`.
    /// A provider configured for another forge lists fine but cannot reach
    /// this instance's repositories. An unknown URL is not held against it.
    pub fn targets(&self, public_url: &str) -> bool {
        self.url
            .as_deref()
            .is_none_or(|url| same_origin(url, public_url))
    }

    /// Explains why a listed provider cannot serve this Gitadel, if it cannot.
    pub fn problem(&self, public_url: &str) -> Option<String> {
        if !self.targets(public_url) {
            return Some(format!(
                "The Dokploy provider {} points at {}, not this Gitadel ({}). Choose or \
                 create a provider for this Gitadel.",
                self.name,
                self.url.as_deref().unwrap_or_default(),
                public_url.trim_end_matches('/'),
            ));
        }
        (!self.ready).then(|| {
            format!(
                "The Dokploy provider {} has not been authorized yet. Authorize it in Dokploy.",
                self.name
            )
        })
    }
}

fn same_origin(left: &str, right: &str) -> bool {
    match (url::Url::parse(left), url::Url::parse(right)) {
        (Ok(left), Ok(right)) => {
            left.origin() == right.origin()
                && left.path().trim_end_matches('/') == right.path().trim_end_matches('/')
        }
        _ => left.trim_end_matches('/') == right.trim_end_matches('/'),
    }
}

/// OAuth client details used to create a remote source provider.
pub struct SourceApplication<'a> {
    pub name: &'a str,
    pub client_id: &'a str,
    pub client_secret: &'a str,
    pub redirect_uri: &'a str,
    pub public_url: &'a str,
    pub internal_url: &'a str,
    /// A token pair Gitadel issued for this application, so the remote
    /// provider is usable without an interactive authorization.
    pub access_token: &'a str,
    pub refresh_token: &'a str,
    /// Unix seconds at which `access_token` expires.
    pub expires_at: i64,
    pub username: &'a str,
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

/// The remote project a new deployment goes into.
pub enum ProjectChoice {
    Existing(String),
    New(String),
}

/// A public host name routed to a new deployment.
pub struct DomainRequest {
    pub host: String,
    pub port: u16,
    pub https: bool,
}

/// Request to create and fully configure a deployment in one call.
pub struct SetupRemoteRequest<'a> {
    pub repository: RepositoryIdentity<'a>,
    pub credential: Credential<'a>,
    pub kind: String,
    pub name: String,
    pub branch: String,
    pub server_id: Option<String>,
    pub repository_path: String,
    pub compose_path: String,
    /// Build with this Dockerfile; `None` leaves the build type to the remote.
    pub dockerfile: Option<String>,
    pub project: ProjectChoice,
    pub environment: String,
    pub domain: Option<DomainRequest>,
}

/// A completed setup: the stored resource link and what was done, in order.
pub struct SetupOutcome {
    pub link: Value,
    pub steps: Vec<String>,
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

    /// Ask the remote service to list repositories through the bound source,
    /// proving it can reach Gitadel and holds a working token. Returns how
    /// many repositories it sees.
    async fn check_remote_source(&self, credential: Credential<'_>) -> Result<usize, String>;

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

    /// Create the project, environment, and resource a repository deploys
    /// to, configure its build and domain, and attach the repository. A
    /// failure removes what this call created.
    async fn setup_remote(&self, request: SetupRemoteRequest<'_>) -> Result<SetupOutcome, String>;

    /// The newest deployment of a linked resource.
    async fn deployment_status(
        &self,
        credential: Credential<'_>,
        resource: &str,
    ) -> Result<Value, String>;
}

#[cfg(test)]
mod tests {
    use super::RemoteSource;

    fn source(url: Option<&str>, ready: bool) -> RemoteSource {
        RemoteSource {
            id: "source-1".to_owned(),
            parent_id: "provider-1".to_owned(),
            name: "Gitadel".to_owned(),
            ready,
            authorization_url: String::new(),
            url: url.map(str::to_owned),
        }
    }

    #[test]
    fn sources_for_another_forge_explain_the_mismatch() {
        let public_url = "http://neo.netbird.cloud:3030/";

        assert!(source(Some("http://neo.netbird.cloud:3030"), true).targets(public_url));
        assert!(source(None, true).targets(public_url));
        assert_eq!(
            source(Some("http://NEO.netbird.cloud:3030/"), true).problem(public_url),
            None
        );

        let other = source(Some("http://kiwi.netbird.cloud:3030"), true);
        assert!(!other.targets(public_url));
        assert!(
            other
                .problem(public_url)
                .unwrap()
                .contains("points at http://kiwi.netbird.cloud:3030, not this Gitadel")
        );
        assert!(
            source(Some("http://neo.netbird.cloud:3030"), false)
                .problem(public_url)
                .unwrap()
                .contains("not been authorized")
        );
    }
}
