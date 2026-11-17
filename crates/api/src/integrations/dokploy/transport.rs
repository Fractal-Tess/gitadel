use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Resource {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) source_type: Option<String>,
    #[serde(default)]
    pub(super) gitea_owner: Option<String>,
    #[serde(default)]
    pub(super) gitea_repository: Option<String>,
    #[serde(default)]
    pub(super) gitea_branch: Option<String>,
    #[serde(default)]
    pub(super) refresh_token: Option<String>,
}
pub(super) fn account_label(response: &Value) -> Option<String> {
    response
        .pointer("/user/email")
        .and_then(Value::as_str)
        .or_else(|| response.get("email").and_then(Value::as_str))
        .or_else(|| response.get("username").and_then(Value::as_str))
        .filter(|label| !label.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let user = response.get("user")?;
            let first_name = user
                .get("firstName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            let last_name = user
                .get("lastName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            let name = format!("{first_name} {last_name}").trim().to_owned();
            (!name.is_empty()).then_some(name)
        })
}

const MAX_ERROR_MESSAGE: usize = 400;

fn unreachable_error(base: &str, error: &reqwest::Error) -> String {
    if error.is_timeout() {
        format!("Dokploy at {base} did not answer in time.")
    } else {
        format!("Could not reach Dokploy at {base}: {error}")
    }
}

/// Keep Dokploy's own explanation for a refused call. tRPC errors carry it in
/// `message`; anything else falls back to the status line.
async fn checked(path: &str, response: reqwest::Response) -> Result<reqwest::Response, String> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(refusal(path, status, &body))
}

fn refusal(path: &str, status: reqwest::StatusCode, body: &str) -> String {
    let endpoint = path.split('?').next().unwrap_or(path);
    let message = serde_json::from_str::<Value>(body).ok().and_then(|body| {
        body.get("message")
            .or_else(|| body.pointer("/error/message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_owned)
    });
    match message {
        Some(message) => {
            let message: String = message.chars().take(MAX_ERROR_MESSAGE).collect();
            format!("Dokploy refused {endpoint} ({status}): {message}")
        }
        None => format!("Dokploy refused {endpoint} ({status})."),
    }
}

fn headers(api_key: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-api-key",
        HeaderValue::from_str(api_key)
            .map_err(|_| "the stored Dokploy API key is not a valid header value".to_owned())?,
    );
    Ok(headers)
}

pub(super) async fn get_json(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    api_key: &str,
) -> Result<Value, String> {
    let response = client
        .get(format!("{base}{path}"))
        .headers(headers(api_key)?)
        .send()
        .await
        .map_err(|error| unreachable_error(base, &error))?;
    let response = checked(path, response).await?;
    response
        .json()
        .await
        .map_err(|error| format!("Dokploy returned malformed JSON for {path}: {error}"))
}

pub(super) async fn get_resource(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    api_key: &str,
) -> Result<Resource, String> {
    let detail = get_json(client, base, path, api_key).await?;
    serde_json::from_value(detail)
        .map_err(|error| format!("Dokploy returned a malformed resource: {error}"))
}

pub(super) async fn post_json(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    api_key: &str,
    body: &Value,
) -> Result<Value, String> {
    let response = client
        .post(format!("{base}{path}"))
        .headers(headers(api_key)?)
        .json(body)
        .send()
        .await
        .map_err(|error| unreachable_error(base, &error))?;
    let response = checked(path, response).await?;
    response
        .json()
        .await
        .map_err(|error| format!("Dokploy returned malformed JSON for {path}: {error}"))
}

pub(super) async fn post_for_status(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    api_key: &str,
    body: &Value,
) -> Result<(), String> {
    let response = client
        .post(format!("{base}{path}"))
        .headers(headers(api_key)?)
        .json(body)
        .send()
        .await
        .map_err(|error| unreachable_error(base, &error))?;
    checked(path, response).await?;
    Ok(())
}

pub(super) async fn post_push_event(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    api_key: &str,
    payload: &Value,
) -> Result<(reqwest::StatusCode, String), String> {
    let response = client
        .post(format!("{base}{path}"))
        .headers(headers(api_key)?)
        .header("x-github-event", "push")
        .header("x-gitea-event", "push")
        .json(payload)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Build the client for one Dokploy instance.
///
/// Dokploy usually lives on a private network, so private ranges are allowed,
/// but loopback and link-local (cloud metadata) addresses are refused and the
/// resolved address is pinned for the lifetime of the client so DNS cannot
/// rebind it between the check and the request. Redirects are not followed.
pub(super) async fn client_for(base: &str) -> Result<reqwest::Client, String> {
    let endpoint = url::Url::parse(base)
        .map_err(|_| "the stored Dokploy URL is not a valid URL".to_owned())?;
    if endpoint.query().is_some() || endpoint.fragment().is_some() {
        return Err("the stored Dokploy URL must not contain a query or fragment".to_owned());
    }
    if cfg!(test) {
        // Unit tests talk to servers bound on loopback.
        return crate::repository::outbound_http_client().map_err(|error| error.to_string());
    }
    crate::network::pinned_service_http_client(&endpoint, REQUEST_TIMEOUT).await
}

#[cfg(test)]
mod tests {
    use super::refusal;
    use reqwest::StatusCode;

    #[test]
    fn refusals_keep_dokploy_messages_and_drop_query_strings() {
        assert_eq!(
            refusal(
                "/api/gitea.getGiteaRepositories?giteaId=abc",
                StatusCode::BAD_REQUEST,
                r#"{"message":"fetch failed","code":"BAD_REQUEST"}"#,
            ),
            "Dokploy refused /api/gitea.getGiteaRepositories (400 Bad Request): fetch failed"
        );
        assert_eq!(
            refusal("/api/project.all", StatusCode::BAD_GATEWAY, "<html>"),
            "Dokploy refused /api/project.all (502 Bad Gateway)."
        );
    }
}
