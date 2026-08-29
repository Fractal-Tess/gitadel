//! Integration registry and event contract.
//!
//! Namespace-owned connections and repository-owned settings are storage
//! concerns. Provider modules implement this contract and decide which events
//! and settings they support. Adding a provider requires one registry entry;
//! event dispatch and the HTTP APIs do not contain provider-specific matches.

mod dokploy;

use serde_json::Value;

use crate::entity::repository;
use crate::repository::RepositoryState;

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

/// A Gitadel event delivered to an enabled repository integration.
///
/// Pushes retain their webhook-compatible payload. Manual deployment is a
/// direct command and deliberately carries no synthetic Git event.
pub enum Event<'a> {
    Push {
        reference: &'a str,
        deleted: bool,
        payload: &'a Value,
    },
    Manual,
}

/// Service and repository state available while handling one event.
pub struct EventContext<'a> {
    pub state: &'a RepositoryState,
    pub event: Event<'a>,
    /// Provider-owned JSON stored on the repository integration row.
    pub resource: Option<&'a str>,
    pub credential: Credential<'a>,
}

/// One external service implementation.
///
/// Event failures are reportable but never fatal to the Git operation that
/// emitted them. Management methods default to unsupported so non-deployment
/// integrations only implement the capabilities they expose.
#[async_trait::async_trait]
pub trait Integration: Send + Sync {
    async fn test(&self, credential: Credential<'_>) -> Result<TestReport, String>;

    async fn handle_event(&self, context: EventContext<'_>) -> Result<String, String>;

    async fn remote_sources(
        &self,
        credential: Credential<'_>,
    ) -> Result<Vec<RemoteSource>, String> {
        let _ = credential;
        Ok(Vec::new())
    }

    async fn provision_remote_source(
        &self,
        credential: Credential<'_>,
        application: SourceApplication<'_>,
    ) -> Result<ProvisionedSource, String> {
        let _ = (credential, application);
        Err("This integration does not support source provisioning.".to_owned())
    }
    async fn update_remote_source_internal_url(
        &self,
        credential: Credential<'_>,
        source_id: &str,
        internal_url: &str,
    ) -> Result<(), String> {
        let _ = (credential, source_id, internal_url);
        Err("This integration does not support source updates.".to_owned())
    }

    async fn remove_remote_source(
        &self,
        credential: Credential<'_>,
        parent_id: &str,
    ) -> Result<(), String> {
        let _ = (credential, parent_id);
        Err("This integration does not support source removal.".to_owned())
    }

    /// Reject provider-owned repository settings before they reach storage.
    fn validate_repository_settings(
        &self,
        _resource: Option<&Value>,
        _config: Option<&Value>,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn remote_resources(
        &self,
        state: &RepositoryState,
        credential: Credential<'_>,
    ) -> Result<Value, String> {
        let _ = (state, credential);
        Err("This integration does not expose remote resources.".to_owned())
    }

    async fn create_remote_project(
        &self,
        state: &RepositoryState,
        credential: Credential<'_>,
        request: Value,
    ) -> Result<Value, String> {
        let _ = (state, credential, request);
        Err("This integration does not support creating projects.".to_owned())
    }

    async fn create_remote_environment(
        &self,
        state: &RepositoryState,
        credential: Credential<'_>,
        request: Value,
    ) -> Result<Value, String> {
        let _ = (state, credential, request);
        Err("This integration does not support creating environments.".to_owned())
    }

    async fn link_remote(
        &self,
        state: &RepositoryState,
        repository: &repository::Model,
        credential: Credential<'_>,
        request: Value,
    ) -> Result<Value, String> {
        let _ = (state, repository, credential, request);
        Err("This integration does not support linking resources.".to_owned())
    }

    async fn create_remote(
        &self,
        state: &RepositoryState,
        repository: &repository::Model,
        credential: Credential<'_>,
        request: Value,
    ) -> Result<Value, String> {
        let _ = (state, repository, credential, request);
        Err("This integration does not support creating resources.".to_owned())
    }

    async fn set_remote_enabled(
        &self,
        state: &RepositoryState,
        credential: Credential<'_>,
        resource: &Value,
        enabled: bool,
    ) -> Result<(), String> {
        let _ = (state, credential, resource, enabled);
        Ok(())
    }
}
