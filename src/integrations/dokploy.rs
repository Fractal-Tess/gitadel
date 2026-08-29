//! Dokploy deployment wiring.
//!
//! Gitadel creates or links one Dokploy Application or Compose resource, wires
//! its Gitea source to the repository, and forwards push events. Everything
//! about how that resource builds and runs remains owned by Dokploy.

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod transport;

use transport::{
    Resource, account_label, fresh_client, get_json, get_resource, post_for_status, post_json,
    post_push_event,
};

use super::{
    CreateEnvironmentRequest, CreateProjectRequest, CreateRemoteRequest, Credential,
    EnablementRequest, Event, EventContext, Integration, LinkRemoteRequest, ProvisionedSource,
    PushEvent, RemoteResourcesRequest, RemoteSource, RepositoryIdentity, SourceApplication,
    TestReport,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Application,
    Compose,
}

impl Kind {
    fn detail_path(self, id: &str) -> String {
        match self {
            Self::Application => format!("/api/application.one?applicationId={id}"),
            Self::Compose => format!("/api/compose.one?composeId={id}"),
        }
    }

    fn webhook_path(self, refresh_token: &str) -> String {
        match self {
            Self::Application => format!("/api/deploy/{refresh_token}"),
            Self::Compose => format!("/api/deploy/compose/{refresh_token}"),
        }
    }

    fn listing(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Application => ("applications", "applicationId", "applicationStatus"),
            Self::Compose => ("compose", "composeId", "composeStatus"),
        }
    }

    fn update_path(self) -> &'static str {
        match self {
            Self::Application => "/api/application.update",
            Self::Compose => "/api/compose.update",
        }
    }

    fn manual_deploy_path(self) -> &'static str {
        match self {
            Self::Application => "/api/application.deploy",
            Self::Compose => "/api/compose.deploy",
        }
    }

    fn id_key(self) -> &'static str {
        match self {
            Self::Application => "applicationId",
            Self::Compose => "composeId",
        }
    }

    fn route_segment(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Compose => "compose",
        }
    }
}

fn parse_kind(value: String) -> Result<Kind, String> {
    serde_json::from_value(Value::String(value)).map_err(|error| error.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Link {
    pub kind: Kind,
    pub id: String,
    pub name: String,
    pub branch: String,
    pub project_id: String,
    pub project_name: String,
    pub environment_id: String,
    pub environment_name: String,
    pub external_url: String,
}

#[derive(Serialize)]
struct Catalog {
    external_url: String,
    projects: Vec<ProjectSummary>,
    servers: Vec<ServerSummary>,
}

#[derive(Serialize)]
struct ProjectSummary {
    id: String,
    name: String,
    environments: Vec<EnvironmentSummary>,
}

#[derive(Serialize)]
struct EnvironmentSummary {
    id: String,
    name: String,
    description: Option<String>,
    resources: Vec<ResourceSummary>,
}

#[derive(Serialize)]
struct ResourceSummary {
    kind: Kind,
    id: String,
    name: String,
    status: Option<String>,
    server_name: Option<String>,
}

#[derive(Serialize)]
struct ServerSummary {
    id: String,
    name: String,
}

#[derive(Clone)]
struct Location {
    project_id: String,
    project_name: String,
    environment_id: String,
    environment_name: String,
}

async fn test(base: &str, api_key: &str) -> Result<TestReport, String> {
    let client = fresh_client().await?;
    let user = get_json(&client, base, "/api/user.get", api_key)
        .await
        .map_err(|error| format!("Dokploy rejected the credential: {error}"))?;
    let account = account_label(&user);

    let mut warnings = Vec::new();
    match get_json(&client, base, "/api/gitea.giteaProviders", api_key).await {
        Ok(providers) if providers.as_array().is_none_or(Vec::is_empty) => {
            warnings.push(
                "No Gitea provider is connected in Dokploy yet. Connect Gitadel under \
                 Settings → Git Providers before applications can be created or linked."
                    .to_owned(),
            );
        }
        Ok(_) => {}
        Err(error) => tracing::debug!(%error, "gitea provider listing unavailable"),
    }

    Ok(TestReport { account, warnings })
}

fn value_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
}

fn find_location(projects: &Value, kind: Kind, id: &str) -> Option<(Location, String)> {
    for project in projects.as_array()? {
        let project_id = value_string(project, "projectId")?;
        let project_name = value_string(project, "name")?;
        for environment in project.get("environments")?.as_array()? {
            let environment_id = value_string(environment, "environmentId")?;
            let environment_name = value_string(environment, "name")?;
            let (field, id_field, _) = kind.listing();
            let resource = environment
                .get(field)
                .and_then(Value::as_array)
                .and_then(|resources| {
                    resources
                        .iter()
                        .find(|resource| resource.get(id_field).and_then(Value::as_str) == Some(id))
                });
            if let Some(resource) = resource {
                let name = value_string(resource, "name").unwrap_or_else(|| id.to_owned());
                return Some((
                    Location {
                        project_id,
                        project_name,
                        environment_id,
                        environment_name,
                    },
                    name,
                ));
            }
        }
    }
    None
}

fn find_environment(projects: &Value, environment_id: &str) -> Option<Location> {
    for project in projects.as_array()? {
        let project_id = value_string(project, "projectId")?;
        let project_name = value_string(project, "name")?;
        for environment in project.get("environments")?.as_array()? {
            if environment.get("environmentId").and_then(Value::as_str) != Some(environment_id) {
                continue;
            }
            return Some(Location {
                project_id,
                project_name,
                environment_id: environment_id.to_owned(),
                environment_name: value_string(environment, "name")?,
            });
        }
    }
    None
}

fn external_url(base: &str, location: &Location, kind: Kind, id: &str) -> String {
    format!(
        "{}/dashboard/project/{}/environment/{}/services/{}/{}",
        base.trim_end_matches('/'),
        location.project_id,
        location.environment_id,
        kind.route_segment(),
        id
    )
}

fn link(
    base: &str,
    location: Location,
    kind: Kind,
    id: String,
    name: String,
    branch: String,
) -> Link {
    Link {
        external_url: external_url(base, &location, kind, &id),
        kind,
        id,
        name,
        branch,
        project_id: location.project_id,
        project_name: location.project_name,
        environment_id: location.environment_id,
        environment_name: location.environment_name,
    }
}

fn catalog_from(base: &str, projects: &Value, servers: &Value) -> Catalog {
    let projects = projects
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|project| {
            let id = value_string(project, "projectId")?;
            let name = value_string(project, "name")?;
            let environments = project
                .get("environments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|environment| {
                    let id = value_string(environment, "environmentId")?;
                    let name = value_string(environment, "name")?;
                    let description = value_string(environment, "description");
                    let mut resources = Vec::new();
                    for kind in [Kind::Application, Kind::Compose] {
                        let (field, id_field, status_field) = kind.listing();
                        for resource in environment
                            .get(field)
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            let Some(id) = value_string(resource, id_field) else {
                                continue;
                            };
                            resources.push(ResourceSummary {
                                kind,
                                name: value_string(resource, "name").unwrap_or_else(|| id.clone()),
                                id,
                                status: value_string(resource, status_field),
                                server_name: resource
                                    .pointer("/server/name")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            });
                        }
                    }
                    Some(EnvironmentSummary {
                        id,
                        name,
                        description,
                        resources,
                    })
                })
                .collect();
            Some(ProjectSummary {
                id,
                name,
                environments,
            })
        })
        .collect();
    let servers = servers
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|server| {
            Some(ServerSummary {
                id: value_string(server, "serverId")?,
                name: value_string(server, "name")?,
            })
        })
        .collect();
    Catalog {
        external_url: base.trim_end_matches('/').to_owned(),
        projects,
        servers,
    }
}

async fn list_resources(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
) -> Result<Value, String> {
    let projects = get_json(client, base, "/api/project.all", api_key).await?;
    let servers = match get_json(client, base, "/api/server.withSSHKey", api_key).await {
        Ok(servers) => servers,
        Err(error) => {
            tracing::debug!(%error, "Dokploy server listing unavailable");
            Value::Array(Vec::new())
        }
    };
    serde_json::to_value(catalog_from(base, &projects, &servers)).map_err(|error| error.to_string())
}

fn created_project_summary(created: &Value) -> Result<ProjectSummary, String> {
    let project = created
        .get("project")
        .ok_or("Dokploy did not return the created project.")?;
    let environment = created
        .get("environment")
        .ok_or("Dokploy did not return the default environment.")?;
    Ok(ProjectSummary {
        id: value_string(project, "projectId")
            .ok_or("Dokploy did not return the created project id.")?,
        name: value_string(project, "name")
            .ok_or("Dokploy did not return the created project name.")?,
        environments: vec![EnvironmentSummary {
            id: value_string(environment, "environmentId")
                .ok_or("Dokploy did not return the default environment id.")?,
            name: value_string(environment, "name")
                .ok_or("Dokploy did not return the default environment name.")?,
            description: value_string(environment, "description"),
            resources: Vec::new(),
        }],
    })
}

async fn create_project(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    request: &CreateProjectRequest<'_>,
) -> Result<Value, String> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err("Project names cannot be empty.".to_owned());
    }
    let created = post_json(
        client,
        base,
        "/api/project.create",
        api_key,
        &serde_json::json!({
            "name": name,
            "description": request.description.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        }),
    )
    .await?;
    serde_json::to_value(created_project_summary(&created)?).map_err(|error| error.to_string())
}

async fn create_environment(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    request: &CreateEnvironmentRequest<'_>,
) -> Result<Value, String> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err("Environment names cannot be empty.".to_owned());
    }
    let projects = get_json(client, base, "/api/project.all", api_key).await?;
    let project = projects
        .as_array()
        .and_then(|projects| {
            projects.iter().find(|project| {
                project.get("projectId").and_then(Value::as_str)
                    == Some(request.project_id.as_str())
            })
        })
        .ok_or("The chosen Dokploy project no longer exists.")?;
    let project_name = value_string(project, "name").unwrap_or_else(|| request.project_id.clone());
    let created = post_json(
        client,
        base,
        "/api/environment.create",
        api_key,
        &serde_json::json!({
            "projectId": request.project_id,
            "name": name,
            "description": request.description.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        }),
    )
    .await?;
    let id = value_string(&created, "environmentId")
        .ok_or("Dokploy did not return an environment id.")?;
    Ok(serde_json::json!({
        "id": id,
        "name": value_string(&created, "name").unwrap_or_else(|| name.to_owned()),
        "description": value_string(&created, "description"),
        "project_id": request.project_id,
        "project_name": project_name,
        "resources": [],
    }))
}

fn source_query_path(endpoint: &str, source_id: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("giteaId", source_id)
        .finish();
    format!("{endpoint}?{query}")
}

fn source_authorization_url(base: &str, source_id: &str) -> Result<String, String> {
    let mut url = url::Url::parse(&format!(
        "{}/api/providers/gitea/authorize",
        base.trim_end_matches('/')
    ))
    .map_err(|error| format!("The Dokploy URL cannot be used for authorization: {error}"))?;
    url.query_pairs_mut().append_pair("giteaId", source_id);
    Ok(url.into())
}

fn source_from_value(base: &str, value: &Value, ready: bool) -> Result<RemoteSource, String> {
    let id =
        value_string(value, "giteaId").ok_or("Dokploy returned a Gitea provider without an id.")?;
    let parent = value
        .get("gitProvider")
        .ok_or("Dokploy returned a Gitea provider without its parent provider.")?;
    let parent_id = value_string(parent, "gitProviderId")
        .ok_or("Dokploy returned a Gitea provider without a parent id.")?;
    let name =
        value_string(parent, "name").ok_or("Dokploy returned a Gitea provider without a name.")?;
    Ok(RemoteSource {
        authorization_url: source_authorization_url(base, &id)?,
        id,
        parent_id,
        name,
        ready,
    })
}

async fn list_sources(credential: Credential<'_>) -> Result<Vec<RemoteSource>, String> {
    let client = fresh_client().await?;
    let providers = get_json(
        &client,
        credential.url,
        "/api/gitea.giteaProviders",
        credential.api_key,
    )
    .await?;
    let mut sources = providers
        .as_array()
        .ok_or("Dokploy returned an invalid Gitea provider list.")?
        .iter()
        .map(|provider| source_from_value(credential.url, provider, true))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(bound_id) = credential.source_id
        && !sources.iter().any(|source| source.id == bound_id)
        && let Ok(provider) = get_json(
            &client,
            credential.url,
            &source_query_path("/api/gitea.one", bound_id),
            credential.api_key,
        )
        .await
    {
        sources.push(source_from_value(credential.url, &provider, false)?);
    }
    Ok(sources)
}
async fn update_source_internal_url(
    credential: Credential<'_>,
    source_id: &str,
    internal_url: &str,
) -> Result<(), String> {
    let client = fresh_client().await?;
    let source = get_json(
        &client,
        credential.url,
        &source_query_path("/api/gitea.one", source_id),
        credential.api_key,
    )
    .await?;
    let parent = source
        .get("gitProvider")
        .ok_or("Dokploy returned a Gitea provider without its parent provider.")?;
    let parent_id = value_string(parent, "gitProviderId")
        .ok_or("Dokploy returned a Gitea provider without a parent id.")?;
    let name =
        value_string(parent, "name").ok_or("Dokploy returned a Gitea provider without a name.")?;
    let public_url = value_string(&source, "giteaUrl")
        .ok_or("Dokploy returned a Gitea provider without a URL.")?;

    post_for_status(
        &client,
        credential.url,
        "/api/gitea.update",
        credential.api_key,
        &serde_json::json!({
            "giteaId": source_id,
            "gitProviderId": parent_id,
            "name": name,
            "giteaUrl": public_url,
            "giteaInternalUrl": internal_url.trim_end_matches('/'),
        }),
    )
    .await
}

async fn create_source(
    credential: Credential<'_>,
    application: SourceApplication<'_>,
) -> Result<ProvisionedSource, String> {
    let client = fresh_client().await?;
    let created = post_json(
        &client,
        credential.url,
        "/api/gitea.create",
        credential.api_key,
        &serde_json::json!({
            "name": application.name,
            "clientId": application.client_id,
            "clientSecret": application.client_secret,
            "redirectUri": application.redirect_uri,
            "giteaUrl": application.public_url.trim_end_matches('/'),
            "giteaInternalUrl": application.internal_url.trim_end_matches('/'),
        }),
    )
    .await?;
    let id =
        value_string(&created, "giteaId").ok_or("Dokploy did not return a Gitea provider id.")?;
    let provider = get_json(
        &client,
        credential.url,
        &source_query_path("/api/gitea.one", &id),
        credential.api_key,
    )
    .await?;
    let source = source_from_value(credential.url, &provider, false)?;
    Ok(ProvisionedSource {
        id: source.id,
        parent_id: source.parent_id,
        authorization_url: source.authorization_url,
    })
}

async fn remove_source(credential: Credential<'_>, parent_id: &str) -> Result<(), String> {
    let client = fresh_client().await?;
    post_for_status(
        &client,
        credential.url,
        "/api/gitProvider.remove",
        credential.api_key,
        &serde_json::json!({ "gitProviderId": parent_id }),
    )
    .await
}

async fn validate_gitea_provider(
    client: &reqwest::Client,
    repository: RepositoryIdentity<'_>,
    base: &str,
    api_key: &str,
    source_id: &str,
) -> Result<(), String> {
    let path = source_query_path("/api/gitea.getGiteaRepositories", source_id);
    let repositories = get_json(client, base, &path, api_key).await?;
    let accessible = repositories.as_array().is_some_and(|repositories| {
        repositories.iter().any(|remote| {
            remote.get("name").and_then(Value::as_str) == Some(repository.name)
                && remote
                    .get("owner")
                    .and_then(|owner| owner.get("username"))
                    .and_then(Value::as_str)
                    .is_some_and(|owner| owner.eq_ignore_ascii_case(repository.namespace))
        })
    });
    if accessible {
        Ok(())
    } else {
        Err(format!(
            "The Dokploy Gitea provider linked to this integration cannot access {}/{}. Reauthorize it in Dokploy.",
            repository.namespace, repository.name
        ))
    }
}

fn sanitized_app_name(repository: RepositoryIdentity<'_>) -> String {
    let mut result = String::with_capacity(repository.namespace.len() + repository.name.len() + 1);
    for character in repository
        .namespace
        .chars()
        .chain(std::iter::once('-'))
        .chain(repository.name.chars())
    {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            result.push(character);
        } else {
            result.push('-');
        }
    }
    result.truncate(63);
    result
}

#[expect(
    clippy::too_many_arguments,
    reason = "arguments mirror Dokploy's source attachment contract"
)]
async fn attach_source(
    client: &reqwest::Client,
    repository: RepositoryIdentity<'_>,
    base: &str,
    api_key: &str,
    source_id: &str,
    kind: Kind,
    id: &str,
    branch: &str,
    repository_path: &str,
    compose_path: &str,
) -> Result<(), String> {
    validate_gitea_provider(client, repository, base, api_key, source_id).await?;
    let gitea_id = source_id;
    match kind {
        Kind::Application => {
            post_json(
                client,
                base,
                "/api/application.saveGiteaProvider",
                api_key,
                &serde_json::json!({
                    "applicationId": id,
                    "giteaId": gitea_id,
                    "giteaOwner": repository.namespace,
                    "giteaRepository": repository.name,
                    "giteaBranch": branch,
                    "giteaBuildPath": repository_path,
                    "enableSubmodules": false,
                    "watchPaths": null,
                }),
            )
            .await?;
        }
        Kind::Compose => {
            post_json(
                client,
                base,
                "/api/compose.update",
                api_key,
                &serde_json::json!({
                    "composeId": id,
                    "sourceType": "gitea",
                    "giteaId": gitea_id,
                    "giteaOwner": repository.namespace,
                    "giteaRepository": repository.name,
                    "giteaBranch": branch,
                    "composePath": compose_path,
                    "composeStatus": "idle",
                    "enableSubmodules": false,
                    "watchPaths": null,
                }),
            )
            .await?;
        }
    }
    Ok(())
}

struct CreateResourceRequest {
    kind: Kind,
    name: String,
    environment_id: String,
    branch: String,
    server_id: Option<String>,
    repository_path: String,
    compose_path: String,
}

async fn create_resource(
    client: &reqwest::Client,
    repository: RepositoryIdentity<'_>,
    base: &str,
    api_key: &str,
    source_id: &str,
    request: &CreateResourceRequest,
) -> Result<Link, String> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err("Resource names cannot be empty.".to_owned());
    }
    let branch = request.branch.trim();
    if branch.is_empty() {
        return Err("A deployment branch is required.".to_owned());
    }
    let projects = get_json(client, base, "/api/project.all", api_key).await?;
    let location = find_environment(&projects, &request.environment_id)
        .ok_or("The chosen Dokploy environment no longer exists.")?;
    let app_name = sanitized_app_name(repository);
    let (id, kind) = match request.kind {
        Kind::Application => {
            let created = post_json(
                client,
                base,
                "/api/application.create",
                api_key,
                &serde_json::json!({
                    "name": name,
                    "appName": app_name,
                    "description": null,
                    "environmentId": request.environment_id,
                    "serverId": request.server_id,
                }),
            )
            .await?;
            (
                value_string(&created, "applicationId")
                    .ok_or("Dokploy did not return an application id.")?,
                Kind::Application,
            )
        }
        Kind::Compose => {
            let created = post_json(
                client,
                base,
                "/api/compose.create",
                api_key,
                &serde_json::json!({
                    "name": name,
                    "appName": app_name,
                    "description": null,
                    "environmentId": request.environment_id,
                    "composeType": "docker-compose",
                    "serverId": request.server_id,
                }),
            )
            .await?;
            (
                value_string(&created, "composeId")
                    .ok_or("Dokploy did not return a Compose id.")?,
                Kind::Compose,
            )
        }
    };
    attach_source(
        client,
        repository,
        base,
        api_key,
        source_id,
        kind,
        &id,
        branch,
        &request.repository_path,
        &request.compose_path,
    )
    .await?;
    Ok(link(
        base,
        location,
        kind,
        id,
        name.to_owned(),
        branch.to_owned(),
    ))
}

struct LinkResourceRequest {
    kind: Kind,
    id: String,
    branch: String,
    repository_path: String,
    compose_path: String,
}

enum SourceState {
    Matching,
    Unconfigured,
}

fn source_state(
    resource: &Resource,
    repository: RepositoryIdentity<'_>,
) -> Result<SourceState, String> {
    let source = resource.source_type.as_deref().unwrap_or_default();
    let owner = resource.gitea_owner.as_deref().unwrap_or_default();
    let name = resource.gitea_repository.as_deref().unwrap_or_default();
    if source.is_empty() || (owner.is_empty() && name.is_empty()) {
        return Ok(SourceState::Unconfigured);
    }
    if source == "gitea"
        && owner.eq_ignore_ascii_case(repository.namespace)
        && name.eq_ignore_ascii_case(repository.name)
    {
        return Ok(SourceState::Matching);
    }
    Err(format!(
        "This Dokploy resource already uses another source ({source}: {owner}/{name})."
    ))
}

async fn link_resource(
    client: &reqwest::Client,
    repository: RepositoryIdentity<'_>,
    base: &str,
    api_key: &str,
    source_id: &str,
    request: &LinkResourceRequest,
) -> Result<Link, String> {
    let projects = get_json(client, base, "/api/project.all", api_key).await?;
    let (location, listed_name) = find_location(&projects, request.kind, &request.id)
        .ok_or("The chosen Dokploy resource no longer exists.")?;
    let resource = get_resource(
        client,
        base,
        &request.kind.detail_path(&request.id),
        api_key,
    )
    .await?;
    let branch = match source_state(&resource, repository)? {
        SourceState::Matching => resource
            .gitea_branch
            .clone()
            .filter(|branch| !branch.trim().is_empty())
            .unwrap_or_else(|| request.branch.clone()),
        SourceState::Unconfigured => {
            attach_source(
                client,
                repository,
                base,
                api_key,
                source_id,
                request.kind,
                &request.id,
                &request.branch,
                &request.repository_path,
                &request.compose_path,
            )
            .await?;
            request.branch.clone()
        }
    };
    Ok(link(
        base,
        location,
        request.kind,
        request.id.clone(),
        resource.name.unwrap_or(listed_name),
        branch,
    ))
}

async fn set_enabled(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    kind: Kind,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    post_json(
        client,
        base,
        kind.update_path(),
        api_key,
        &serde_json::json!({ kind.id_key(): id, "autoDeploy": enabled }),
    )
    .await?;
    Ok(())
}

async fn trigger_push(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    link: &Link,
    payload: &Value,
) -> Result<bool, String> {
    let resource = get_resource(client, base, &link.kind.detail_path(&link.id), api_key).await?;
    let refresh_token = resource
        .refresh_token
        .as_deref()
        .ok_or("The linked Dokploy resource has no deployment webhook.")?;
    let (status, body) = post_push_event(
        client,
        base,
        &link.kind.webhook_path(refresh_token),
        api_key,
        payload,
    )
    .await?;
    if status.is_success() {
        tracing::info!(resource = %link.name, %status, "Dokploy deployment triggered");
        Ok(true)
    } else {
        tracing::info!(resource = %link.name, %status, body = body.trim(), "Dokploy declined the deployment");
        Ok(false)
    }
}

async fn deploy_now(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    link: &Link,
) -> Result<(), String> {
    post_for_status(
        client,
        base,
        link.kind.manual_deploy_path(),
        api_key,
        &serde_json::json!({
            link.kind.id_key(): link.id,
            "title": "Manual deployment from Gitadel",
        }),
    )
    .await
}

pub(crate) struct DokployIntegration;

#[async_trait::async_trait]
impl Integration for DokployIntegration {
    async fn test(&self, credential: Credential<'_>) -> Result<TestReport, String> {
        test(credential.url, credential.api_key).await
    }

    async fn remote_sources(
        &self,
        credential: Credential<'_>,
    ) -> Result<Vec<RemoteSource>, String> {
        list_sources(credential).await
    }

    async fn provision_remote_source(
        &self,
        credential: Credential<'_>,
        application: SourceApplication<'_>,
    ) -> Result<ProvisionedSource, String> {
        create_source(credential, application).await
    }
    async fn update_remote_source_internal_url(
        &self,
        credential: Credential<'_>,
        source_id: &str,
        internal_url: &str,
    ) -> Result<(), String> {
        update_source_internal_url(credential, source_id, internal_url).await
    }

    async fn remove_remote_source(
        &self,
        credential: Credential<'_>,
        parent_id: &str,
    ) -> Result<(), String> {
        remove_source(credential, parent_id).await
    }

    fn validate_repository_settings(
        &self,
        resource: Option<&Value>,
        config: Option<&Value>,
    ) -> Result<(), String> {
        if let Some(resource) = resource {
            let link: Link =
                serde_json::from_value(resource.clone()).map_err(|error| error.to_string())?;
            if [
                link.id.as_str(),
                link.name.as_str(),
                link.branch.as_str(),
                link.project_id.as_str(),
                link.environment_id.as_str(),
                link.external_url.as_str(),
            ]
            .iter()
            .any(|value| value.trim().is_empty())
            {
                return Err("Dokploy resource links cannot contain empty fields.".to_owned());
            }
        }
        if config.is_some() {
            return Err("Deployment configuration is managed in Dokploy.".to_owned());
        }
        Ok(())
    }

    async fn remote_resources(&self, request: RemoteResourcesRequest<'_>) -> Result<Value, String> {
        let client = fresh_client().await?;
        list_resources(&client, request.credential.url, request.credential.api_key).await
    }

    async fn create_remote_project(
        &self,
        request: CreateProjectRequest<'_>,
    ) -> Result<Value, String> {
        let client = fresh_client().await?;
        create_project(
            &client,
            request.credential.url,
            request.credential.api_key,
            &request,
        )
        .await
    }

    async fn create_remote_environment(
        &self,
        request: CreateEnvironmentRequest<'_>,
    ) -> Result<Value, String> {
        let client = fresh_client().await?;
        create_environment(
            &client,
            request.credential.url,
            request.credential.api_key,
            &request,
        )
        .await
    }

    async fn link_remote(&self, request: LinkRemoteRequest<'_>) -> Result<Value, String> {
        let client = fresh_client().await?;
        let body = LinkResourceRequest {
            kind: parse_kind(request.kind)?,
            id: request.id,
            branch: request.branch,
            repository_path: request.repository_path,
            compose_path: request.compose_path,
        };
        let link = link_resource(
            &client,
            request.repository,
            request.credential.url,
            request.credential.api_key,
            request
                .credential
                .source_id
                .ok_or("Connect this integration to a Dokploy Gitea provider first.")?,
            &body,
        )
        .await?;
        serde_json::to_value(link).map_err(|error| error.to_string())
    }

    async fn create_remote(&self, request: CreateRemoteRequest<'_>) -> Result<Value, String> {
        let client = fresh_client().await?;
        let body = CreateResourceRequest {
            kind: parse_kind(request.kind)?,
            name: request.name,
            environment_id: request.environment_id,
            branch: request.branch,
            server_id: request.server_id,
            repository_path: request.repository_path,
            compose_path: request.compose_path,
        };
        let link = create_resource(
            &client,
            request.repository,
            request.credential.url,
            request.credential.api_key,
            request
                .credential
                .source_id
                .ok_or("Connect this integration to a Dokploy Gitea provider first.")?,
            &body,
        )
        .await?;
        serde_json::to_value(link).map_err(|error| error.to_string())
    }

    async fn set_remote_enabled(&self, request: EnablementRequest<'_>) -> Result<(), String> {
        let client = fresh_client().await?;
        let kind = parse_kind(request.kind)?;
        set_enabled(
            &client,
            request.credential.url,
            request.credential.api_key,
            kind,
            &request.id,
            request.enabled,
        )
        .await
    }

    async fn handle_event(&self, context: EventContext<'_>) -> Result<String, String> {
        let stored = context
            .resource
            .ok_or("Choose a Dokploy deployment resource first.")?;
        let link: Link = serde_json::from_str(stored)
            .map_err(|error| format!("stored Dokploy resource is malformed: {error}"))?;
        let client = fresh_client().await?;
        match context.event {
            Event::Push(PushEvent {
                reference,
                deleted: false,
                payload,
            }) if reference.starts_with("refs/heads/") => {
                let accepted = trigger_push(
                    &client,
                    context.credential.url,
                    context.credential.api_key,
                    &link,
                    payload,
                )
                .await?;
                Ok(if accepted {
                    format!("Deployment triggered for {}.", link.name)
                } else {
                    format!(
                        "Dokploy declined deployment for {} (branch or watch paths).",
                        link.name
                    )
                })
            }
            Event::Push(_) => Ok(String::new()),
            Event::Manual => {
                deploy_now(
                    &client,
                    context.credential.url,
                    context.credential.api_key,
                    &link,
                )
                .await?;
                Ok(format!("Deployment triggered for {}.", link.name))
            }
        }
    }
}

pub(crate) static INTEGRATION: DokployIntegration = DokployIntegration;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn repository() -> RepositoryIdentity<'static> {
        RepositoryIdentity {
            namespace: "fractal-tess",
            name: "gitadel",
        }
    }

    #[test]
    fn account_label_reads_the_nested_dokploy_user() {
        let response = json!({
            "id": "organization-member-id",
            "user": {
                "id": "user-id",
                "email": "owner@example.com",
                "firstName": "Example",
                "lastName": "Owner"
            }
        });

        assert_eq!(
            account_label(&response).as_deref(),
            Some("owner@example.com")
        );
    }

    #[test]
    fn account_label_does_not_present_internal_ids_as_usernames() {
        let response = json!({
            "id": "organization-member-id",
            "user": { "id": "user-id" }
        });

        assert_eq!(account_label(&response), None);
    }

    #[test]
    fn repository_settings_require_complete_resource_link() {
        let resource = json!({ "kind": "application", "id": "app-1" });

        let result = INTEGRATION.validate_repository_settings(Some(&resource), None);

        assert!(result.is_err());
    }

    #[test]
    fn repository_settings_reject_local_deployment_configuration() {
        let result = INTEGRATION.validate_repository_settings(None, Some(&json!({})));

        assert!(result.is_err());
    }

    #[test]
    fn source_state_accepts_the_same_repository() {
        let resource = Resource {
            name: Some("Gitadel".to_owned()),
            source_type: Some("gitea".to_owned()),
            gitea_owner: Some("Fractal-Tess".to_owned()),
            gitea_repository: Some("Gitadel".to_owned()),
            gitea_branch: Some("main".to_owned()),
            refresh_token: None,
        };

        assert!(matches!(
            source_state(&resource, repository()),
            Ok(SourceState::Matching)
        ));
    }

    #[test]
    fn source_state_rejects_another_repository() {
        let resource = Resource {
            name: Some("Other".to_owned()),
            source_type: Some("gitea".to_owned()),
            gitea_owner: Some("someone".to_owned()),
            gitea_repository: Some("else".to_owned()),
            gitea_branch: Some("main".to_owned()),
            refresh_token: None,
        };

        assert!(source_state(&resource, repository()).is_err());
    }

    #[test]
    fn resource_link_uses_the_exact_dokploy_route() {
        let location = Location {
            project_id: "project-1".to_owned(),
            project_name: "Gitadel".to_owned(),
            environment_id: "environment-1".to_owned(),
            environment_name: "Production".to_owned(),
        };

        assert_eq!(
            external_url(
                "https://dokploy.example.com/",
                &location,
                Kind::Application,
                "application-1"
            ),
            "https://dokploy.example.com/dashboard/project/project-1/environment/environment-1/services/application/application-1"
        );
    }
    #[test]
    fn created_project_includes_dokploy_default_environment() {
        let response = json!({
            "project": {
                "projectId": "project-1",
                "name": "Gitadel"
            },
            "environment": {
                "environmentId": "environment-1",
                "name": "production",
                "description": null
            }
        });

        let summary = created_project_summary(&response).unwrap();

        assert_eq!(
            serde_json::to_value(summary).unwrap(),
            json!({
                "id": "project-1",
                "name": "Gitadel",
                "environments": [{
                    "id": "environment-1",
                    "name": "production",
                    "description": null,
                    "resources": []
                }]
            })
        );
    }

    #[tokio::test]
    async fn manual_deploy_accepts_dokploys_empty_success_response() {
        let app = axum::Router::new().route(
            "/api/application.deploy",
            axum::routing::post(|| async { axum::http::StatusCode::OK }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let result = post_for_status(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            "/api/application.deploy",
            "api-key",
            &json!({ "applicationId": "application-1" }),
        )
        .await;
        server.abort();

        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn source_provider_maps_stable_ids_and_authorization_url() {
        let source = source_from_value(
            "https://dokploy.example.com",
            &json!({
                "giteaId": "gitea/source id",
                "gitProvider": {
                    "gitProviderId": "provider-1",
                    "name": "Gitadel"
                }
            }),
            true,
        )
        .unwrap();

        assert_eq!(source.id, "gitea/source id");
        assert_eq!(source.parent_id, "provider-1");
        assert_eq!(source.name, "Gitadel");
        assert!(source.ready);
        assert_eq!(
            source.authorization_url,
            "https://dokploy.example.com/api/providers/gitea/authorize?giteaId=gitea%2Fsource+id"
        );
    }

    #[tokio::test]
    async fn repository_access_checks_only_the_bound_gitea_provider() {
        async fn repositories(
            axum::extract::Query(query): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >,
        ) -> axum::Json<Value> {
            let repositories = if query.get("giteaId").map(String::as_str) == Some("bound-source") {
                json!([{
                    "name": "gitadel",
                    "owner": { "username": "fractal-tess" }
                }])
            } else {
                json!([])
            };
            axum::Json(repositories)
        }

        let app = axum::Router::new().route(
            "/api/gitea.getGiteaRepositories",
            axum::routing::get(repositories),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::new();
        let base = format!("http://{address}");

        let bound =
            validate_gitea_provider(&client, repository(), &base, "api-key", "bound-source").await;
        let other =
            validate_gitea_provider(&client, repository(), &base, "api-key", "other-source").await;
        server.abort();

        assert!(bound.is_ok(), "{bound:?}");
        assert!(other.is_err());
    }

    #[tokio::test]
    async fn source_provisioning_sends_gitadel_oauth_client_to_dokploy() {
        async fn create(
            headers: axum::http::HeaderMap,
            axum::Json(body): axum::Json<Value>,
        ) -> Result<axum::Json<Value>, axum::http::StatusCode> {
            if headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
                != Some("api-key")
                || body.get("name").and_then(Value::as_str) != Some("Gitadel")
                || body.get("clientId").and_then(Value::as_str) != Some("client-id")
                || body.get("clientSecret").and_then(Value::as_str) != Some("client-secret")
                || body.get("redirectUri").and_then(Value::as_str)
                    != Some("https://dokploy.example.com/api/providers/gitea/callback")
                || body.get("giteaUrl").and_then(Value::as_str)
                    != Some("https://gitadel.example.com")
                || body.get("giteaInternalUrl").and_then(Value::as_str)
                    != Some("http://100.64.0.5:3030")
            {
                return Err(axum::http::StatusCode::BAD_REQUEST);
            }
            Ok(axum::Json(json!({ "giteaId": "gitea-1" })))
        }

        async fn one(
            axum::extract::Query(query): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >,
        ) -> Result<axum::Json<Value>, axum::http::StatusCode> {
            if query.get("giteaId").map(String::as_str) != Some("gitea-1") {
                return Err(axum::http::StatusCode::NOT_FOUND);
            }
            Ok(axum::Json(json!({
                "giteaId": "gitea-1",
                "gitProvider": {
                    "gitProviderId": "provider-1",
                    "name": "Gitadel"
                }
            })))
        }

        let app = axum::Router::new()
            .route("/api/gitea.create", axum::routing::post(create))
            .route("/api/gitea.one", axum::routing::get(one));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{address}");

        let provisioned = create_source(
            Credential {
                url: &base,
                api_key: "api-key",
                source_id: None,
            },
            SourceApplication {
                name: "Gitadel",
                client_id: "client-id",
                client_secret: "client-secret",
                redirect_uri: "https://dokploy.example.com/api/providers/gitea/callback",
                public_url: "https://gitadel.example.com/",
                internal_url: "http://100.64.0.5:3030/",
            },
        )
        .await;
        server.abort();

        let provisioned = provisioned.unwrap();
        assert_eq!(provisioned.id, "gitea-1");
        assert_eq!(provisioned.parent_id, "provider-1");
    }

    #[tokio::test]
    async fn source_update_sets_internal_url_without_changing_public_url() {
        async fn one(
            axum::extract::Query(query): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >,
        ) -> Result<axum::Json<Value>, axum::http::StatusCode> {
            if query.get("giteaId").map(String::as_str) != Some("gitea-1") {
                return Err(axum::http::StatusCode::NOT_FOUND);
            }
            Ok(axum::Json(json!({
                "giteaId": "gitea-1",
                "giteaUrl": "https://gitadel.example.com",
                "gitProvider": {
                    "gitProviderId": "provider-1",
                    "name": "Gitadel"
                }
            })))
        }

        async fn update(
            headers: axum::http::HeaderMap,
            axum::Json(body): axum::Json<Value>,
        ) -> axum::http::StatusCode {
            if headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
                == Some("api-key")
                && body.get("giteaId").and_then(Value::as_str) == Some("gitea-1")
                && body.get("gitProviderId").and_then(Value::as_str) == Some("provider-1")
                && body.get("name").and_then(Value::as_str) == Some("Gitadel")
                && body.get("giteaUrl").and_then(Value::as_str)
                    == Some("https://gitadel.example.com")
                && body.get("giteaInternalUrl").and_then(Value::as_str)
                    == Some("http://100.64.0.8:3030")
            {
                axum::http::StatusCode::NO_CONTENT
            } else {
                axum::http::StatusCode::BAD_REQUEST
            }
        }

        let app = axum::Router::new()
            .route("/api/gitea.one", axum::routing::get(one))
            .route("/api/gitea.update", axum::routing::post(update));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{address}");

        let result = update_source_internal_url(
            Credential {
                url: &base,
                api_key: "api-key",
                source_id: Some("gitea-1"),
            },
            "gitea-1",
            "http://100.64.0.8:3030/",
        )
        .await;
        server.abort();

        assert!(result.is_ok(), "{result:?}");
    }
}
