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
        .map_err(|error| error.to_string())?;
    let response = response
        .error_for_status()
        .map_err(|error| format!("{path} failed: {error}"))?;
    response
        .json()
        .await
        .map_err(|error| format!("{path} returned malformed JSON: {error}"))
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
        .map_err(|error| error.to_string())?;
    let response = response
        .error_for_status()
        .map_err(|error| format!("{path} failed: {error}"))?;
    response
        .json()
        .await
        .map_err(|error| format!("{path} returned malformed JSON: {error}"))
}

pub(super) async fn post_for_status(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    api_key: &str,
    body: &Value,
) -> Result<(), String> {
    client
        .post(format!("{base}{path}"))
        .headers(headers(api_key)?)
        .json(body)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| format!("{path} failed: {error}"))?;
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

pub(super) async fn fresh_client() -> Result<reqwest::Client, String> {
    crate::repository::outbound_http_client().map_err(|error| error.to_string())
}
