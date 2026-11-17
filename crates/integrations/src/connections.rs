//! Namespace integration connections and source provisioning workflows.
use crate::{
    Credential, PROVIDERS, Provider, RemoteSource, SourceApplication, TestReport, provider,
};
use chrono::Utc;
use gitadel_core::audit::AuditContext;
use gitadel_db::entity::{dokploy_source_binding, namespace_integration, oauth_application};
use gitadel_identity::permissions::RepositoryPrincipal;
use gitadel_identity::{
    IdentityError, hash_secret, permissions::authorize_namespace, random_secret,
};
use gitadel_secrets::{SecretCipher, SecretKind};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use std::collections::HashMap;
use uuid::Uuid;
const MAX_API_KEY_LENGTH: usize = 512;
const MAX_NAME_LENGTH: usize = 80;
/// OAuth material issued by identity; its storage format stays identity-owned.
pub struct SourceTokens {
    pub access_token: String,
    pub refresh_token: String,
}
#[async_trait::async_trait]
pub trait SourceTokenPort: Send + Sync {
    fn generate(&self) -> SourceTokens;
    fn access_token_lifetime(&self) -> chrono::Duration;
    async fn store(
        &self,
        tokens: &SourceTokens,
        transaction: &DatabaseTransaction,
        application_id: Uuid,
        user_id: Uuid,
        scope: &str,
    ) -> Result<(), IdentityError>;
}
/// Construction is side-effect free. The server supplies identity's token port.
pub struct ConnectionService<'a> {
    database: &'a DatabaseConnection,
    cipher: &'a SecretCipher,
    public_url: &'a url::Url,
    tokens: &'a dyn SourceTokenPort,
    audit: AuditContext,
}
impl<'a> ConnectionService<'a> {
    pub fn new(
        database: &'a DatabaseConnection,
        cipher: &'a SecretCipher,
        public_url: &'a url::Url,
        tokens: &'a dyn SourceTokenPort,
        audit: AuditContext,
    ) -> Self {
        Self {
            database,
            cipher,
            public_url,
            tokens,
            audit,
        }
    }
    pub async fn list_integrations(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
    ) -> Result<NamespaceIntegrationsResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_READ)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let stored = namespace_integration::Entity::find()
            .filter(namespace_integration::Column::Namespace.eq(slug.clone()))
            .order_by_asc(namespace_integration::Column::CreatedAt)
            .all(self.database)
            .await?;
        let bindings = dokploy_source_binding::Entity::find()
            .filter(
                dokploy_source_binding::Column::IntegrationId
                    .is_in(stored.iter().map(|integration| integration.id)),
            )
            .all(self.database)
            .await?
            .into_iter()
            .map(|binding| (binding.integration_id, binding))
            .collect::<HashMap<_, _>>();
        let integrations = stored
            .into_iter()
            .filter_map(|row| {
                provider(&row.provider).map(|provider| {
                    let binding = bindings.get(&row.id);
                    IntegrationResponse::new(provider, row, binding, self.public_url.as_str())
                })
            })
            .collect();
        Ok(NamespaceIntegrationsResponse {
            namespace: slug,
            providers: PROVIDERS.iter().map(ProviderResponse::from).collect(),
            integrations,
        })
    }
    pub async fn create_integration(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
        request: CreateIntegrationRequest,
    ) -> Result<IntegrationResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let provider = provider(&request.provider).ok_or_else(IdentityError::not_found)?;
        let name = validate_name(&request.name)?;
        let url = validate_outbound_url(&request.url, provider.name).await?;
        let api_key =
            validate_api_key(&request.api_key, true)?.expect("required key was validated");
        let internal_url = validate_url(&request.internal_url, "Gitadel")?;
        let now = Utc::now();
        let id = Uuid::new_v4();
        let sealed = seal_api_key(self.cipher, id, &api_key)?;
        let transaction = self.database.begin().await?;
        let integration = namespace_integration::ActiveModel {
            id: Set(id),
            namespace: Set(slug.clone()),
            provider: Set(provider.slug.to_owned()),
            name: Set(name),
            enabled: Set(true),
            url: Set(url),
            api_key: Set(sealed),
            dokploy_internal_url: Set(Some(internal_url)),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&transaction)
        .await?;
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            "integration.create",
            Some(format!("{slug}/{}", integration.id)),
            self.audit,
        )
        .await?;
        transaction.commit().await?;
        Ok(IntegrationResponse::new(
            provider,
            integration,
            None,
            self.public_url.as_str(),
        ))
    }
    pub async fn update_integration(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
        id: Uuid,
        request: UpdateIntegrationRequest,
    ) -> Result<IntegrationResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let existing = owned_integration(self.cipher, self.database, &slug, id).await?;
        let provider = provider(&existing.provider).ok_or_else(IdentityError::not_found)?;
        let binding = source_binding(self.database, id).await?;
        let url = match request.url.as_deref() {
            Some(url) => Some(validate_outbound_url(url, provider.name).await?),
            None => None,
        };
        let api_key = validate_api_key(request.api_key.as_deref().unwrap_or_default(), false)?;
        let internal_url = request
            .internal_url
            .as_deref()
            .map(|url| validate_url(url, "Gitadel"))
            .transpose()?;
        if binding.is_some()
            && (url.as_ref().is_some_and(|url| url != &existing.url)
                || api_key
                    .as_ref()
                    .is_some_and(|api_key| api_key != &existing.api_key))
        {
            return Err(IdentityError::conflict(
                "Disconnect the Dokploy source before changing this connection's URL or API key.",
            ));
        }
        if let Some(binding) = binding.as_ref().filter(|binding| binding.managed)
            && internal_url.as_ref().is_some_and(|internal_url| {
                internal_url
                    != existing
                        .dokploy_internal_url
                        .as_deref()
                        .unwrap_or(self.public_url.as_str())
            })
        {
            provider
                .integration
                .update_remote_source_internal_url(
                    Credential {
                        url: &existing.url,
                        api_key: &existing.api_key,
                        source_id: Some(&binding.gitea_id),
                    },
                    &binding.gitea_id,
                    internal_url.as_deref().expect("change was checked"),
                )
                .await
                .map_err(IdentityError::bad_request)?;
        }

        let transaction = self.database.begin().await?;
        let mut active = stored_integration(&transaction, &slug, id)
            .await?
            .into_active_model();
        if let Some(name) = request.name {
            active.name = Set(validate_name(&name)?);
        }
        if let Some(enabled) = request.enabled {
            active.enabled = Set(enabled);
        }
        if let Some(url) = url {
            active.url = Set(url);
        }
        if let Some(api_key) = api_key {
            active.api_key = Set(seal_api_key(self.cipher, id, &api_key)?);
        }
        if let Some(internal_url) = internal_url {
            active.dokploy_internal_url = Set(Some(internal_url));
        }
        active.updated_at = Set(Utc::now());
        let integration = active.update(&transaction).await?;
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            "integration.update",
            Some(format!("{slug}/{id}")),
            self.audit,
        )
        .await?;
        transaction.commit().await?;
        Ok(IntegrationResponse::new(
            provider,
            integration,
            binding.as_ref(),
            self.public_url.as_str(),
        ))
    }
    pub async fn delete_integration(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
        id: Uuid,
    ) -> Result<(), IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let integration = owned_integration(self.cipher, self.database, &slug, id).await?;
        let binding = source_binding(self.database, id).await?;
        if let Some(binding) = binding.as_ref() {
            let provider = provider(&integration.provider).ok_or_else(IdentityError::not_found)?;
            cleanup_managed_source(provider, &integration, binding).await;
        }
        let transaction = self.database.begin().await?;
        namespace_integration::Entity::delete_by_id(id)
            .exec(&transaction)
            .await?;
        if let Some(application_id) = binding.and_then(|binding| binding.oauth_application_id) {
            oauth_application::Entity::delete_by_id(application_id)
                .exec(&transaction)
                .await?;
        }
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            "integration.delete",
            Some(format!("{slug}/{id}")),
            self.audit,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }
    pub async fn get_integration_credential(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
        id: Uuid,
    ) -> Result<IntegrationCredentialResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let stored = owned_integration(self.cipher, self.database, &slug, id).await?;
        Ok(IntegrationCredentialResponse {
            api_key: stored.api_key,
        })
    }
    pub async fn get_integration_source(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
        id: Uuid,
    ) -> Result<SourceConnectionResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_READ)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let integration = owned_integration(self.cipher, self.database, &slug, id).await?;
        let provider = provider(&integration.provider).ok_or_else(IdentityError::not_found)?;
        if !provider.source_required {
            return Err(IdentityError::not_found());
        }
        let binding = source_binding(self.database, id).await?;
        let sources = provider
            .integration
            .remote_sources(Credential {
                url: &integration.url,
                api_key: &integration.api_key,
                source_id: binding.as_ref().map(|binding| binding.gitea_id.as_str()),
            })
            .await
            .map_err(IdentityError::bad_request)?;
        Ok(source_connection_response(
            provider,
            binding.as_ref(),
            &sources,
            self.public_url.as_str(),
        ))
    }
    pub async fn configure_integration_source(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
        id: Uuid,
        request: ConfigureSourceRequest,
    ) -> Result<SourceConnectionResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let integration = owned_integration(self.cipher, self.database, &slug, id).await?;
        let provider = provider(&integration.provider).ok_or_else(IdentityError::not_found)?;
        if !provider.source_required {
            return Err(IdentityError::not_found());
        }
        if source_binding(self.database, id).await?.is_some() {
            return Err(IdentityError::conflict(
                "Disconnect the current Dokploy source before choosing another one.",
            ));
        }

        let credential = Credential {
            url: &integration.url,
            api_key: &integration.api_key,
            source_id: None,
        };
        let (binding, sources) = match request {
            ConfigureSourceRequest::Bind { source_id } => {
                let source_id = source_id.trim();
                if source_id.is_empty() {
                    return Err(IdentityError::bad_request(
                        "Choose a Dokploy Gitea provider.",
                    ));
                }
                let sources = provider
                    .integration
                    .remote_sources(credential)
                    .await
                    .map_err(IdentityError::bad_request)?;
                let source = sources
                    .iter()
                    .find(|source| source.id == source_id)
                    .ok_or_else(|| {
                        IdentityError::bad_request(
                            "That Dokploy Gitea provider is no longer available.",
                        )
                    })?;
                if let Some(problem) = source.problem(self.public_url.as_str()) {
                    return Err(IdentityError::bad_request(problem));
                }
                let now = Utc::now();
                let transaction = self.database.begin().await?;
                let binding = dokploy_source_binding::ActiveModel {
                    integration_id: Set(id),
                    gitea_id: Set(source.id.clone()),
                    git_provider_id: Set(source.parent_id.clone()),
                    oauth_application_id: Set(None),
                    managed: Set(false),
                    created_at: Set(now),
                    updated_at: Set(now),
                }
                .insert(&transaction)
                .await?;
                gitadel_db::audit::record(
                    &transaction,
                    Some(actor.user.id),
                    "integration.source.connect",
                    Some(format!("{slug}/{id}/{}", binding.gitea_id)),
                    self.audit,
                )
                .await?;
                transaction.commit().await?;
                (binding, sources)
            }
            ConfigureSourceRequest::Create { name } => {
                if actor.via_api_token {
                    return Err(IdentityError::forbidden(
                        "Create and authorize Dokploy source providers from a browser session.",
                    ));
                }
                let name = validate_name(&name)?;
                let redirect_uri = format!(
                    "{}/api/providers/gitea/callback",
                    integration.url.trim_end_matches('/')
                );
                let client_id = format!("goc_{}", random_secret(24));
                let client_secret = format!("gcs_{}", random_secret(32));
                let application_id = Uuid::new_v4();
                // Gitadel issues Dokploy's tokens itself, so the provider works
                // without anyone completing an OAuth authorization in Dokploy.
                let tokens = self.tokens.generate();
                let expires_at = Utc::now() + self.tokens.access_token_lifetime();
                let internal_url = integration
                    .dokploy_internal_url
                    .as_deref()
                    .unwrap_or(self.public_url.as_str());
                let provisioned = match provider
                    .integration
                    .provision_remote_source(
                        credential,
                        SourceApplication {
                            name: &name,
                            client_id: &client_id,
                            client_secret: &client_secret,
                            redirect_uri: &redirect_uri,
                            public_url: self.public_url.as_str(),
                            internal_url,
                            access_token: &tokens.access_token,
                            refresh_token: &tokens.refresh_token,
                            expires_at: expires_at.timestamp(),
                            username: &actor.user.username,
                        },
                    )
                    .await
                {
                    Ok(provisioned) => provisioned,
                    Err(error) => return Err(IdentityError::bad_request(error)),
                };
                let transaction = self.database.begin().await?;
                let now = Utc::now();
                let binding_result: Result<dokploy_source_binding::Model, IdentityError> = async {
                    let application = oauth_application::ActiveModel {
                        id: Set(application_id),
                        user_id: Set(actor.user.id),
                        name: Set(format!("Dokploy: {}", integration.name)),
                        client_id: Set(client_id),
                        client_secret_hash: Set(hash_secret(&client_secret)),
                        redirect_uri: Set(redirect_uri),
                        created_at: Set(Utc::now()),
                    }
                    .insert(&transaction)
                    .await?;
                    self.tokens
                        .store(
                            &tokens,
                            &transaction,
                            application.id,
                            actor.user.id,
                            DOKPLOY_SCOPE,
                        )
                        .await?;
                    let binding = dokploy_source_binding::ActiveModel {
                        integration_id: Set(id),
                        gitea_id: Set(provisioned.id.clone()),
                        git_provider_id: Set(provisioned.parent_id.clone()),
                        oauth_application_id: Set(Some(application.id)),
                        managed: Set(true),
                        created_at: Set(now),
                        updated_at: Set(now),
                    }
                    .insert(&transaction)
                    .await?;
                    gitadel_db::audit::record(
                        &transaction,
                        Some(actor.user.id),
                        "integration.source.connect",
                        Some(format!("{slug}/{id}/{}", binding.gitea_id)),
                        self.audit,
                    )
                    .await?;
                    transaction.commit().await?;
                    Ok(binding)
                }
                .await;
                let binding = match binding_result {
                    Ok(binding) => binding,
                    Err(error) => {
                        if let Err(cleanup_error) = provider
                            .integration
                            .remove_remote_source(credential, &provisioned.parent_id)
                            .await
                        {
                            tracing::warn!(%cleanup_error, integration_id = %id, "could not remove orphaned Dokploy source");
                        }
                        return Err(error);
                    }
                };
                let source = RemoteSource {
                    id: provisioned.id,
                    parent_id: provisioned.parent_id,
                    name,
                    ready: true,
                    authorization_url: provisioned.authorization_url,
                    url: Some(self.public_url.as_str().trim_end_matches('/').to_owned()),
                };
                (binding, vec![source])
            }
        };
        let check = verify_source(self, provider, &slug, &integration, &binding).await?;
        let mut response = source_connection_response(
            provider,
            Some(&binding),
            &sources,
            self.public_url.as_str(),
        );
        response.check = Some(check);
        Ok(response)
    }
    pub async fn verify_integration_source(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
        id: Uuid,
    ) -> Result<SourceCheckResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let integration = owned_integration(self.cipher, self.database, &slug, id).await?;
        let provider = provider(&integration.provider).ok_or_else(IdentityError::not_found)?;
        let binding = source_binding(self.database, id)
            .await?
            .ok_or_else(|| IdentityError::bad_request("Connect a Dokploy source first."))?;
        verify_source(self, provider, &slug, &integration, &binding).await
    }
    pub async fn disconnect_integration_source(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
        id: Uuid,
    ) -> Result<(), IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let integration = owned_integration(self.cipher, self.database, &slug, id).await?;
        let provider = provider(&integration.provider).ok_or_else(IdentityError::not_found)?;
        let binding = source_binding(self.database, id)
            .await?
            .ok_or_else(IdentityError::not_found)?;
        cleanup_managed_source(provider, &integration, &binding).await;
        let transaction = self.database.begin().await?;
        dokploy_source_binding::Entity::delete_by_id(id)
            .exec(&transaction)
            .await?;
        if let Some(application_id) = binding.oauth_application_id {
            oauth_application::Entity::delete_by_id(application_id)
                .exec(&transaction)
                .await?;
        }
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            "integration.source.disconnect",
            Some(format!("{slug}/{id}/{}", binding.gitea_id)),
            self.audit,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }
    pub async fn test_new_integration(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
        request: TestIntegrationRequest,
    ) -> Result<IntegrationTestResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let provider = provider(&request.provider).ok_or_else(IdentityError::not_found)?;
        let url = validate_outbound_url(&request.url, provider.name).await?;
        let api_key =
            validate_api_key(&request.api_key, true)?.expect("required key was validated");
        test_credential(provider, &url, &api_key, None).await
    }
    pub async fn test_integration(
        &self,
        principal: &RepositoryPrincipal,
        slug: String,
        id: Uuid,
    ) -> Result<IntegrationTestResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_READ)
            .await?;
        authorize_namespace(self.database, actor, &slug).await?;
        let stored = owned_integration(self.cipher, self.database, &slug, id).await?;
        let provider = provider(&stored.provider).ok_or_else(IdentityError::not_found)?;
        let binding = source_binding(self.database, id).await?;
        test_credential(
            provider,
            &stored.url,
            &stored.api_key,
            binding.as_ref().map(|binding| binding.gitea_id.as_str()),
        )
        .await
    }
}
pub struct ProviderResponse {
    pub slug: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub source_required: bool,
}

impl From<&'static Provider> for ProviderResponse {
    fn from(provider: &'static Provider) -> Self {
        Self {
            slug: provider.slug,
            name: provider.name,
            description: provider.description,
            icon: provider.icon,
            source_required: provider.source_required,
        }
    }
}

pub struct IntegrationResponse {
    pub id: Uuid,
    pub provider: &'static str,
    pub provider_name: &'static str,
    pub name: String,
    pub enabled: bool,
    pub url: String,
    pub internal_url: String,
    pub api_key_set: bool,
    pub source: Option<SourceBindingSummary>,
}

pub struct SourceBindingSummary {
    pub id: String,
    pub name: String,
    pub managed: bool,
}

impl From<&dokploy_source_binding::Model> for SourceBindingSummary {
    fn from(binding: &dokploy_source_binding::Model) -> Self {
        Self {
            id: binding.gitea_id.clone(),
            name: "Gitea provider".to_owned(),
            managed: binding.managed,
        }
    }
}

impl IntegrationResponse {
    fn new(
        provider: &'static Provider,
        integration: namespace_integration::Model,
        binding: Option<&dokploy_source_binding::Model>,
        default_internal_url: &str,
    ) -> Self {
        let internal_url = integration
            .dokploy_internal_url
            .clone()
            .unwrap_or_else(|| default_internal_url.to_owned());
        Self {
            id: integration.id,
            provider: provider.slug,
            provider_name: provider.name,
            name: integration.name,
            enabled: integration.enabled,
            url: integration.url,
            internal_url,
            api_key_set: true,
            source: binding.map(SourceBindingSummary::from),
        }
    }
}

pub struct NamespaceIntegrationsResponse {
    pub namespace: String,
    pub providers: Vec<ProviderResponse>,
    pub integrations: Vec<IntegrationResponse>,
}
pub struct IntegrationCredentialResponse {
    pub api_key: String,
}

pub struct RemoteSourceResponse {
    pub id: String,
    pub name: String,
    pub ready: bool,
    pub url: Option<String>,
    /// False when the provider was configured for another forge.
    pub targets_instance: bool,
    /// Why this provider cannot be used by this Gitadel, if it cannot.
    pub problem: Option<String>,
}

impl RemoteSourceResponse {
    fn new(source: &RemoteSource, public_url: &str) -> Self {
        Self {
            id: source.id.clone(),
            name: source.name.clone(),
            ready: source.ready && source.targets(public_url),
            url: source.url.clone(),
            targets_instance: source.targets(public_url),
            problem: source.problem(public_url),
        }
    }
}

pub struct SourceConnectionResponse {
    pub required: bool,
    pub binding: Option<SourceBindingStatus>,
    pub sources: Vec<RemoteSourceResponse>,
    /// Set when this response follows a reachability check.
    pub check: Option<SourceCheckResponse>,
}

/// Whether Dokploy can list repositories through the bound provider.
pub struct SourceCheckResponse {
    pub reachable: bool,
    /// The Gitadel address Dokploy uses after the check.
    pub internal_url: String,
    pub repositories: Option<usize>,
    /// Addresses that failed, in the order they were tried.
    pub attempts: Vec<SourceAttempt>,
}

pub struct SourceAttempt {
    pub url: String,
    pub error: String,
}

/// What a Gitadel-managed Dokploy provider may do: read repositories and the
/// account they belong to.
const DOKPLOY_SCOPE: &str = "read:repository read:user";

pub struct SourceBindingStatus {
    pub id: String,
    pub name: String,
    pub managed: bool,
    pub ready: bool,
    pub authorization_url: Option<String>,
    pub url: Option<String>,
    pub targets_instance: bool,
    pub problem: Option<String>,
}

pub enum ConfigureSourceRequest {
    Bind { source_id: String },
    Create { name: String },
}

pub struct CreateIntegrationRequest {
    pub provider: String,
    pub name: String,
    pub url: String,
    pub api_key: String,
    pub internal_url: String,
}

pub struct UpdateIntegrationRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub internal_url: Option<String>,
}

pub struct TestIntegrationRequest {
    pub provider: String,
    pub url: String,
    pub api_key: String,
}

pub struct IntegrationTestResponse {
    pub account: Option<String>,
    pub warnings: Vec<String>,
}

async fn verify_source(
    state: &ConnectionService<'_>,
    provider: &'static Provider,
    slug: &str,
    integration: &namespace_integration::Model,
    binding: &dokploy_source_binding::Model,
) -> Result<SourceCheckResponse, IdentityError> {
    let credential = Credential {
        url: &integration.url,
        api_key: &integration.api_key,
        source_id: Some(&binding.gitea_id),
    };
    let current = integration
        .dokploy_internal_url
        .clone()
        .unwrap_or_else(|| state.public_url.as_str().trim_end_matches('/').to_owned());
    let mut attempts = Vec::new();
    match provider.integration.check_remote_source(credential).await {
        Ok(repositories) => {
            return Ok(SourceCheckResponse {
                reachable: true,
                internal_url: current,
                repositories: Some(repositories),
                attempts,
            });
        }
        Err(error) => attempts.push(SourceAttempt {
            url: current.clone(),
            error,
        }),
    }
    if !binding.managed {
        return Ok(SourceCheckResponse {
            reachable: false,
            internal_url: current,
            repositories: None,
            attempts,
        });
    }
    for candidate in candidate_internal_urls(state.public_url.as_str()).await {
        if attempts.iter().any(|attempt| attempt.url == candidate) {
            continue;
        }
        let result = match provider
            .integration
            .update_remote_source_internal_url(credential, &binding.gitea_id, &candidate)
            .await
        {
            Ok(()) => provider.integration.check_remote_source(credential).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(repositories) => {
                let mut active = stored_integration(state.database, slug, integration.id)
                    .await?
                    .into_active_model();
                active.dokploy_internal_url = Set(Some(candidate.clone()));
                active.updated_at = Set(Utc::now());
                active.update(state.database).await?;
                return Ok(SourceCheckResponse {
                    reachable: true,
                    internal_url: candidate,
                    repositories: Some(repositories),
                    attempts,
                });
            }
            Err(error) => attempts.push(SourceAttempt {
                url: candidate,
                error,
            }),
        }
    }
    // Nothing worked; leave the provider on the address the user chose.
    if let Err(error) = provider
        .integration
        .update_remote_source_internal_url(credential, &binding.gitea_id, &current)
        .await
    {
        tracing::warn!(%error, "could not restore the Dokploy provider's internal URL");
    }
    Ok(SourceCheckResponse {
        reachable: false,
        internal_url: current,
        repositories: None,
        attempts,
    })
}

async fn candidate_internal_urls(public_url: &str) -> Vec<String> {
    let mut candidates = vec![public_url.trim_end_matches('/').to_owned()];
    let Ok(parsed) = url::Url::parse(public_url) else {
        return candidates;
    };
    let (Some(host), Some(port)) = (parsed.host_str(), parsed.port_or_known_default()) else {
        return candidates;
    };
    if let Ok(addresses) = tokio::net::lookup_host((host, port)).await {
        for address in addresses {
            if address.ip().is_loopback() {
                continue;
            }
            let mut candidate = parsed.clone();
            if candidate.set_ip_host(address.ip()).is_err() {
                continue;
            }
            let candidate = candidate.as_str().trim_end_matches('/').to_owned();
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

async fn test_credential(
    provider: &'static Provider,
    url: &str,
    api_key: &str,
    source_id: Option<&str>,
) -> Result<IntegrationTestResponse, IdentityError> {
    let TestReport { account, warnings } = provider
        .integration
        .test(Credential {
            url,
            api_key,
            source_id,
        })
        .await
        .map_err(|error| {
            IdentityError::bad_request(format!(
                "{} rejected the credential: {error}",
                provider.name
            ))
        })?;
    Ok(IntegrationTestResponse { account, warnings })
}
fn source_connection_response(
    provider: &'static Provider,
    binding: Option<&dokploy_source_binding::Model>,
    sources: &[RemoteSource],
    public_url: &str,
) -> SourceConnectionResponse {
    let binding = binding.map(|binding| {
        let remote = sources.iter().find(|source| source.id == binding.gitea_id);
        SourceBindingStatus {
            id: binding.gitea_id.clone(),
            name: remote
                .map(|source| source.name.clone())
                .unwrap_or_else(|| "Gitea provider".to_owned()),
            managed: binding.managed,
            ready: remote.is_some_and(|source| source.ready && source.targets(public_url)),
            authorization_url: remote.map(|source| source.authorization_url.clone()),
            url: remote.and_then(|source| source.url.clone()),
            targets_instance: remote.is_none_or(|source| source.targets(public_url)),
            problem: match remote {
                Some(source) => source.problem(public_url),
                None => Some(
                    "The bound Dokploy provider no longer exists. Disconnect it and choose another."
                        .to_owned(),
                ),
            },
        }
    });
    SourceConnectionResponse {
        required: provider.source_required,
        binding,
        sources: sources
            .iter()
            .map(|source| RemoteSourceResponse::new(source, public_url))
            .collect(),
        check: None,
    }
}

async fn cleanup_managed_source(
    provider: &'static Provider,
    integration: &namespace_integration::Model,
    binding: &dokploy_source_binding::Model,
) {
    if !binding.managed {
        return;
    }
    if let Err(error) = provider
        .integration
        .remove_remote_source(
            Credential {
                url: &integration.url,
                api_key: &integration.api_key,
                source_id: Some(&binding.gitea_id),
            },
            &binding.git_provider_id,
        )
        .await
    {
        tracing::warn!(
            %error,
            integration_id = %integration.id,
            source_id = %binding.gitea_id,
            "could not remove managed Dokploy source"
        );
    }
}

async fn source_binding<C>(
    database: &C,
    integration_id: Uuid,
) -> Result<Option<dokploy_source_binding::Model>, IdentityError>
where
    C: ConnectionTrait,
{
    Ok(dokploy_source_binding::Entity::find_by_id(integration_id)
        .one(database)
        .await?)
}

/// The stored row, with the API key still sealed.
async fn stored_integration<C>(
    database: &C,
    namespace: &str,
    id: Uuid,
) -> Result<namespace_integration::Model, IdentityError>
where
    C: sea_orm::ConnectionTrait,
{
    namespace_integration::Entity::find_by_id(id)
        .filter(namespace_integration::Column::Namespace.eq(namespace))
        .one(database)
        .await?
        .ok_or_else(IdentityError::not_found)
}

/// The row with its API key opened for use.
async fn owned_integration<C>(
    cipher: &SecretCipher,
    database: &C,
    namespace: &str,
    id: Uuid,
) -> Result<namespace_integration::Model, IdentityError>
where
    C: sea_orm::ConnectionTrait,
{
    open_api_key(cipher, stored_integration(database, namespace, id).await?)
}

/// Replaces the sealed API key of a loaded row with its plain value, so
/// callers can hand it to the provider. Fails with an error naming the
/// integration when the key was sealed under another host key.
pub fn open_api_key(
    cipher: &SecretCipher,
    mut integration: namespace_integration::Model,
) -> Result<namespace_integration::Model, IdentityError> {
    integration.api_key = cipher.open_text(
        SecretKind::DokployApiKey,
        &integration.id,
        &integration.api_key,
    )?;
    Ok(integration)
}

fn seal_api_key(cipher: &SecretCipher, id: Uuid, api_key: &str) -> Result<String, IdentityError> {
    cipher
        .seal_text(SecretKind::DokployApiKey, &id, api_key)
        .map_err(IdentityError::internal)
}

pub fn validate_name(value: &str) -> Result<String, IdentityError> {
    let name = value.trim();
    if name.is_empty() || name.len() > MAX_NAME_LENGTH {
        return Err(IdentityError::bad_request(
            "Integration names must contain between 1 and 80 characters.",
        ));
    }
    Ok(name.to_owned())
}

fn validate_api_key(value: &str, required: bool) -> Result<Option<String>, IdentityError> {
    let key = value.trim();
    if key.is_empty() {
        return if required {
            Err(IdentityError::bad_request("An API key is required."))
        } else {
            Ok(None)
        };
    }
    if key.len() > MAX_API_KEY_LENGTH {
        return Err(IdentityError::bad_request(
            "API keys cannot exceed 512 characters.",
        ));
    }
    Ok(Some(key.to_owned()))
}

fn validate_url(value: &str, name: &str) -> Result<String, IdentityError> {
    Ok(parse_url(value, name)?
        .as_str()
        .trim_end_matches('/')
        .to_owned())
}

fn parse_url(value: &str, name: &str) -> Result<url::Url, IdentityError> {
    let url = url::Url::parse(value.trim()).map_err(|_| {
        IdentityError::bad_request(format!("The {name} URL must be a valid HTTP or HTTPS URL."))
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.as_str().len() > 2048
    {
        return Err(IdentityError::bad_request(format!(
            "The {name} URL must be an HTTP or HTTPS URL without credentials, a query, or a fragment."
        )));
    }
    Ok(url)
}

/// Validate a URL that Gitadel itself will send requests to.
///
/// Beyond the syntax rules, the host must not resolve to this machine or to a
/// link-local address such as a cloud metadata service. Private networks stay
/// allowed because providers normally run on one. The same check runs again,
/// with the address pinned, whenever a request is made.
async fn validate_outbound_url(value: &str, name: &str) -> Result<String, IdentityError> {
    let url = parse_url(value, name)?;
    gitadel_core::network::pinned_service_http_client(&url, std::time::Duration::from_secs(10))
        .await
        .map_err(|error| {
            IdentityError::bad_request(format!("The {name} URL was refused: {error}"))
        })?;
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_reject_credentials_queries_and_fragments() {
        for refused in [
            "http://127.0.0.1:9000/x?",
            "http://dokploy.example/x?a=b",
            "http://dokploy.example/#fragment",
            "http://user@dokploy.example/",
            "http://user:secret@dokploy.example/",
            "ftp://dokploy.example/",
            "not a url",
        ] {
            assert!(validate_url(refused, "Dokploy").is_err(), "{refused}");
        }
        assert_eq!(
            validate_url(" https://dokploy.example/base/ ", "Dokploy")
                .ok()
                .as_deref(),
            Some("https://dokploy.example/base")
        );
    }

    #[tokio::test]
    async fn outbound_urls_refuse_loopback_and_metadata_but_allow_private_networks() {
        for refused in [
            "http://127.0.0.1:9000",
            "http://localhost:9000",
            "http://[::1]:9000",
            "http://169.254.169.254",
            "http://[fe80::1]",
            "http://0.0.0.0:9000",
            "http://[::ffff:127.0.0.1]",
        ] {
            assert!(
                validate_outbound_url(refused, "Dokploy").await.is_err(),
                "{refused}"
            );
        }
        for allowed in [
            "http://10.0.0.5:3000",
            "http://192.168.1.20:3000",
            "http://100.64.0.5:3000",
            "http://[fd00::5]:3000",
        ] {
            assert_eq!(
                validate_outbound_url(allowed, "Dokploy")
                    .await
                    .ok()
                    .as_deref(),
                Some(allowed),
                "{allowed}"
            );
        }
    }
}
