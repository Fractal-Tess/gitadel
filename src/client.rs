use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::{GitadelCommand, RepositoryCommand};

#[derive(Serialize)]
struct CreateRepositoryRequest<'a> {
    namespace: &'a str,
    name: &'a str,
    description: &'a Option<String>,
    visibility: &'static str,
    object_format: &'a str,
}

#[derive(Deserialize)]
struct RepositoryResponse {
    namespace: String,
    name: String,
    visibility: String,
    object_format: String,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

pub async fn run(command: &GitadelCommand) -> Result<()> {
    match command {
        GitadelCommand::Repo { command } => run_repository(command).await,
    }
}

async fn run_repository(command: &RepositoryCommand) -> Result<()> {
    match command {
        RepositoryCommand::Create {
            repository,
            server,
            token,
            private,
            public: _,
            description,
            object_format,
        } => {
            create_repository(
                repository,
                server,
                token,
                *private,
                description,
                object_format,
            )
            .await
        }
    }
}

async fn create_repository(
    repository: &str,
    server: &Url,
    token: &str,
    private: bool,
    description: &Option<String>,
    object_format: &str,
) -> Result<()> {
    let Some((namespace, name)) = repository.split_once('/') else {
        bail!("repository must use namespace/name form");
    };
    if namespace.is_empty() || name.is_empty() || name.contains('/') {
        bail!("repository must use namespace/name form");
    }
    let endpoint = server
        .join("/api/v1/repositories")
        .context("Gitadel server URL is invalid")?;
    let response = reqwest::Client::new()
        .post(endpoint)
        .bearer_auth(token)
        .json(&CreateRepositoryRequest {
            namespace,
            name,
            description,
            visibility: if private { "private" } else { "public" },
            object_format,
        })
        .send()
        .await
        .context("could not reach the Gitadel server")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        if let Ok(error) = serde_json::from_str::<ErrorEnvelope>(&body) {
            bail!("Gitadel returned {status}: {}", error.error.message);
        }
        bail!("Gitadel returned {status}");
    }
    let repository: RepositoryResponse = response
        .json()
        .await
        .context("Gitadel returned an invalid repository response")?;
    println!(
        "Created {}/{} ({}; {}).",
        repository.namespace, repository.name, repository.visibility, repository.object_format
    );
    Ok(())
}
