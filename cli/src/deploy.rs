//! `gtd deploy`: deploy a repository through a Dokploy connection.
//!
//! Gitadel gets a deployment off the ground: it creates the Dokploy project,
//! environment, and resource, points the build at the repository, routes a
//! domain, and starts the first deployment. Environment variables and every
//! other runtime setting stay in Dokploy.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{Value, json};

use crate::{ApiClient, route_path, route_repo};

/// How long `--wait` follows a deployment before giving up.
const WAIT_LIMIT: Duration = Duration::from_secs(20 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// How long `--wait` waits for a new domain to answer after deploying.
const SITE_WAIT_LIMIT: Duration = Duration::from_secs(3 * 60);

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Kind {
    /// Build one image from a Dockerfile.
    Dockerfile,
    /// Run a Docker Compose file.
    Compose,
}

#[derive(Debug, Subcommand)]
#[expect(
    clippy::large_enum_variant,
    reason = "parsed once per process; boxing would only obscure the flags"
)]
pub(crate) enum DeployCommand {
    /// Show how the repository can deploy, read from its default branch.
    Suggest { repository: String },
    /// Create, configure, and start a deployment in one step.
    ///
    /// Without flags it uses the first detected Dockerfile or Compose file,
    /// the only configured connection, a project named after the repository,
    /// and the production environment.
    Setup {
        repository: String,
        /// Public host name, for example app.example.com.
        #[arg(long)]
        domain: Option<String>,
        /// Connection name or ID, when the namespace has more than one.
        #[arg(long)]
        connection: Option<String>,
        /// Build from a Dockerfile or a Compose file; detected when omitted.
        #[arg(long, value_enum)]
        kind: Option<Kind>,
        /// Directory holding the Dockerfile, such as /web.
        #[arg(long)]
        build_path: Option<String>,
        /// Dockerfile name inside --build-path.
        #[arg(long)]
        dockerfile: Option<String>,
        /// Compose file, such as ./docker-compose.yml.
        #[arg(long)]
        compose_path: Option<String>,
        /// Compose service the domain routes to.
        #[arg(long)]
        service: Option<String>,
        /// Port the app listens on inside its container.
        #[arg(long)]
        port: Option<u16>,
        /// Serve the domain over plain HTTP instead of Let's Encrypt HTTPS.
        #[arg(long)]
        http: bool,
        /// Name of the Dokploy resource; defaults to the repository name.
        #[arg(long)]
        name: Option<String>,
        /// Branch to deploy; defaults to the repository's default branch.
        #[arg(long)]
        branch: Option<String>,
        /// Dokploy project name; an existing project with it is reused.
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "production")]
        environment: String,
        /// Do not start a deployment now.
        #[arg(long)]
        no_deploy: bool,
        /// Do not redeploy on every push.
        #[arg(long)]
        no_push_deploy: bool,
        /// Follow the first deployment until it finishes and the site answers.
        #[arg(long)]
        wait: bool,
        /// Use --domain even though something already answers on it.
        #[arg(long)]
        force: bool,
        /// Add a second deployment to a repository that already has one.
        #[arg(long)]
        additional: bool,
    },
    /// Show the latest deployment of each linked resource.
    Status {
        repository: String,
        /// Follow a running deployment until it finishes.
        #[arg(long)]
        wait: bool,
    },
    /// Start a deployment of each linked resource now.
    Now {
        repository: String,
        /// Follow the deployment until it finishes.
        #[arg(long)]
        wait: bool,
    },
}

fn integrations_path(repository: &str) -> Result<String> {
    Ok(route_repo("repos", repository)? + "/integrations")
}

fn instance_path(repository: &str, id: &str, suffix: &str) -> Result<String> {
    Ok(route_path(&integrations_path(repository)?, &[id])? + suffix)
}

/// Linked integration instances: the ones with a deployment resource.
async fn linked(api: &ApiClient, repository: &str) -> Result<Vec<Value>> {
    let catalog = api
        .request(Method::GET, &integrations_path(repository)?, None)
        .await?;
    let linked: Vec<Value> = catalog["integrations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|integration| !integration["resource"].is_null())
        .cloned()
        .collect();
    if linked.is_empty() {
        bail!("{repository} has no deployment yet; run `gtd deploy setup {repository}`");
    }
    Ok(linked)
}

async fn status(api: &ApiClient, repository: &str, id: &str) -> Result<Value> {
    api.request(
        Method::GET,
        &instance_path(repository, id, "/deployment")?,
        None,
    )
    .await
}

/// Poll until the newest deployment started after `after` (if given) is no
/// longer running. Progress goes to stderr; the final status is returned.
async fn wait_for(
    api: &ApiClient,
    repository: &str,
    id: &str,
    after: Option<String>,
) -> Result<Value> {
    let started = std::time::Instant::now();
    let mut last = String::new();
    loop {
        let current = status(api, repository, id).await?;
        let deployment = &current["deployment"];
        let created = deployment["created_at"].as_str().map(str::to_owned);
        let fresh = after.is_none() || (created.is_some() && created > after);
        let state = if fresh {
            deployment["status"].as_str().unwrap_or("pending")
        } else {
            "pending"
        };
        if state != last {
            eprintln!("deployment: {state}");
            last = state.to_owned();
        }
        if fresh && matches!(state, "done" | "error" | "cancelled") {
            if state != "done" {
                if let Some(log) = deployment["log"].as_str() {
                    eprintln!(
                        "--- last lines of the deployment log ---\n{}",
                        log.trim_end()
                    );
                }
                eprintln!("logs: {}", current["logs_url"].as_str().unwrap_or_default());
                return Err(anyhow!(
                    "deployment {state}: {}",
                    deployment["error"]
                        .as_str()
                        .unwrap_or("see the Dokploy logs")
                ));
            }
            return Ok(current);
        }
        if started.elapsed() > WAIT_LIMIT {
            bail!("the deployment is still {state} after 20 minutes");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until `url` answers over a verified connection. A new domain's
/// Let's Encrypt certificate is issued shortly after its first deployment.
async fn wait_for_site(url: &str) -> Value {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => return json!({ "url": url, "error": error.to_string() }),
    };
    let started = std::time::Instant::now();
    let mut last_error = String::new();
    while started.elapsed() < SITE_WAIT_LIMIT {
        match client.get(url).send().await {
            Ok(response) => {
                eprintln!("site: {url} answered {}", response.status());
                return json!({ "url": url, "status": response.status().as_u16() });
            }
            Err(error) => {
                let message = format!("{error:#}");
                if message != last_error {
                    eprintln!("site: waiting for {url} ({message})");
                    last_error = message;
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    json!({ "url": url, "error": last_error })
}

/// Refuse a host that already serves something. Dokploy accepts the same
/// domain on two resources and its proxy then routes it unpredictably;
/// for a host it does not know, the proxy answers a bare 404.
async fn ensure_domain_unused(host: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let Ok(response) = client.get(format!("http://{host}/")).send().await else {
        // Nothing answers yet, so nothing is using it.
        return Ok(());
    };
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::NOT_FOUND && body.trim() == "404 page not found" {
        return Ok(());
    }
    bail!(
        "{host} already answers ({status}); it is probably in use by another deployment. \
         Choose another subdomain, or pass --force to use it anyway"
    )
}

fn pick_connection(catalog: &Value, wanted: Option<&str>) -> Result<Value> {
    let configured: Vec<&Value> = catalog["connections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|connection| connection["configured"] == true)
        .collect();
    if let Some(wanted) = wanted {
        return configured
            .into_iter()
            .find(|connection| connection["id"] == wanted || connection["name"] == wanted)
            .cloned()
            .ok_or_else(|| anyhow!("no ready connection named {wanted}"));
    }
    match configured.as_slice() {
        [only] => Ok((*only).clone()),
        [] => bail!(
            "no ready Dokploy connection; add one under Account settings → Integrations \
             (once per namespace, in the browser)"
        ),
        many => bail!(
            "several connections are ready; choose one with --connection ({})",
            many.iter()
                .filter_map(|connection| connection["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub(crate) async fn run(api: &ApiClient, command: DeployCommand) -> Result<Value> {
    match command {
        DeployCommand::Suggest { repository } => {
            api.request(
                Method::GET,
                &(route_repo("repos", &repository)? + "/deploy/suggestions"),
                None,
            )
            .await
        }
        DeployCommand::Status { repository, wait } => {
            let mut results = Vec::new();
            for integration in linked(api, &repository).await? {
                let id = integration["id"].as_str().unwrap_or_default().to_owned();
                let current = if wait {
                    wait_for(api, &repository, &id, None).await?
                } else {
                    status(api, &repository, &id).await?
                };
                results.push(json!({
                    "integration": integration["name"],
                    "resource": integration["resource"]["name"],
                    "deploy_on_push": integration["enabled"],
                    "deployment": current["deployment"],
                    "logs_url": current["logs_url"],
                }));
            }
            Ok(Value::Array(results))
        }
        DeployCommand::Now { repository, wait } => {
            let mut results = Vec::new();
            for integration in linked(api, &repository).await? {
                let id = integration["id"].as_str().unwrap_or_default().to_owned();
                let before = status(api, &repository, &id).await?["deployment"]["created_at"]
                    .as_str()
                    .map(str::to_owned);
                api.request(
                    Method::POST,
                    &instance_path(&repository, &id, "/deploy")?,
                    None,
                )
                .await?;
                let current = if wait {
                    wait_for(api, &repository, &id, before.or(Some(String::new()))).await?
                } else {
                    json!({ "deployment": "started" })
                };
                results.push(json!({
                    "resource": integration["resource"]["name"],
                    "deployment": current["deployment"],
                }));
            }
            Ok(Value::Array(results))
        }
        DeployCommand::Setup {
            repository,
            domain,
            connection,
            kind,
            build_path,
            dockerfile,
            compose_path,
            service,
            port,
            http,
            name,
            branch,
            project,
            environment,
            no_deploy,
            no_push_deploy,
            wait,
            force,
            additional,
        } => {
            if let Some(host) = domain.as_deref()
                && !force
            {
                ensure_domain_unused(host).await?;
            }
            let (_, repository_name) = crate::parse_repo(&repository)?;
            let repository_name = repository_name.to_owned();
            let base = integrations_path(&repository)?;
            let catalog = api.request(Method::GET, &base, None).await?;
            let deployed: Vec<&str> = catalog["integrations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|integration| integration["resource"]["name"].as_str())
                .collect();
            if !deployed.is_empty() && !additional {
                bail!(
                    "{repository} already deploys to {}; use `gtd deploy status` or \
                     `gtd deploy now`, or pass --additional for a second deployment",
                    deployed.join(", ")
                );
            }
            let chosen = pick_connection(&catalog, connection.as_deref())?;
            let connection_id = chosen["id"].as_str().unwrap_or_default().to_owned();

            // Detect and validate everything before creating anything, so a
            // missing port or service leaves the repository untouched.
            let detected = api
                .request(
                    Method::GET,
                    &(route_repo("repos", &repository)? + "/deploy/suggestions"),
                    None,
                )
                .await?;
            let suggestions = detected["suggestions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let wanted_kind = match kind {
                Some(Kind::Dockerfile) => Some("application"),
                Some(Kind::Compose) => Some("compose"),
                None if compose_path.is_some() => Some("compose"),
                None if dockerfile.is_some() || build_path.is_some() => Some("application"),
                None => None,
            };
            let suggestion = suggestions
                .iter()
                .find(|candidate| {
                    wanted_kind.is_none_or(|kind| candidate["kind"] == kind)
                        && build_path
                            .as_deref()
                            .is_none_or(|path| candidate["build_path"] == path)
                        && compose_path
                            .as_deref()
                            .is_none_or(|path| candidate["compose_path"] == path)
                })
                .cloned()
                .unwrap_or(Value::Null);
            let kind = wanted_kind
                .or_else(|| suggestion["kind"].as_str())
                .ok_or_else(|| {
                    anyhow!(
                        "no Dockerfile or Compose file found on the default branch; add one \
                         or pass --kind with --build-path or --compose-path"
                    )
                })?
                .to_owned();
            for warning in suggestion["warnings"].as_array().into_iter().flatten() {
                eprintln!("warning: {}", warning.as_str().unwrap_or_default());
            }

            let mut body = json!({
                "kind": kind,
                "name": name.clone().unwrap_or_else(|| repository_name.clone()),
                "branch": branch
                    .or_else(|| detected["branch"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "main".to_owned()),
                "environment": environment,
                "deploy": !no_deploy,
                "deploy_on_push": !no_push_deploy,
            });
            if kind == "compose" {
                body["compose_path"] = json!(
                    compose_path
                        .or_else(|| suggestion["compose_path"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| "./docker-compose.yml".to_owned())
                );
            } else {
                body["repository_path"] = json!(
                    build_path
                        .or_else(|| suggestion["build_path"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| "/".to_owned())
                );
                if let Some(dockerfile) =
                    dockerfile.or_else(|| suggestion["dockerfile"].as_str().map(str::to_owned))
                {
                    body["dockerfile"] = json!(dockerfile);
                }
            }

            let site_url = domain
                .as_deref()
                .map(|host| format!("{}://{host}/", if http { "http" } else { "https" }));
            if let Some(host) = domain {
                let port = port
                    .or_else(|| {
                        suggestion["port"]
                            .as_u64()
                            .and_then(|port| u16::try_from(port).ok())
                    })
                    .ok_or_else(|| {
                        anyhow!("the port the app listens on is unknown; pass --port")
                    })?;
                let mut domain = json!({ "host": host, "port": port, "https": !http });
                if kind == "compose" {
                    domain["service"] = json!(
                        service
                            .or_else(|| suggestion["service"].as_str().map(str::to_owned))
                            .ok_or_else(|| anyhow!(
                                "choose the Compose service the domain routes to with --service"
                            ))?
                    );
                }
                body["domain"] = domain;
            }

            // Reuse an unlinked instance of this connection, else create one.
            let existing = catalog["integrations"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|integration| {
                    integration["connection_id"] == connection_id.as_str()
                        && integration["resource"].is_null()
                })
                .cloned();
            let instance = match existing {
                Some(instance) => instance,
                None => {
                    api.request(
                        Method::POST,
                        &base,
                        Some(json!({
                            "integration_id": connection_id,
                            "name": name.clone().unwrap_or_else(|| repository_name.clone()),
                        })),
                    )
                    .await?
                }
            };
            let id = instance["id"]
                .as_str()
                .context("Gitadel did not return the integration id")?
                .to_owned();

            // Reuse a Dokploy project with the requested name.
            let project = project.unwrap_or_else(|| repository_name.clone());
            let resources = api
                .request(
                    Method::GET,
                    &instance_path(&repository, &id, "/resources")?,
                    None,
                )
                .await?;
            let existing_project = resources["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|candidate| candidate["name"] == project.as_str())
                .and_then(|candidate| candidate["id"].as_str());
            match existing_project {
                Some(project_id) => body["project_id"] = json!(project_id),
                None => body["project_name"] = json!(project),
            }

            let mut result = api
                .request(
                    Method::POST,
                    &instance_path(&repository, &id, "/setup")?,
                    Some(body),
                )
                .await?;
            for step in result["steps"].as_array().into_iter().flatten() {
                eprintln!("done: {}", step.as_str().unwrap_or_default());
            }
            if let Some(warning) = result["warning"].as_str() {
                eprintln!("warning: {warning}");
            }
            if wait && !no_deploy {
                let finished = wait_for(api, &repository, &id, None).await?;
                result["deployment"] = finished["deployment"].clone();
                result["logs_url"] = finished["logs_url"].clone();
                if let Some(site) = site_url {
                    result["site"] = wait_for_site(&site).await;
                }
            }
            Ok(result)
        }
    }
}
