use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::{Response, header};
use serde_json::Value;
use tokio::net::lookup_host;
use url::Url;
use uuid::Uuid;

use super::{RepositoryState, issues, releases};
use crate::{entity::repository, identity::ApiError};

const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 100;
const MAX_REDIRECTS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Github,
    Gitlab,
    Gitea,
    Forgejo,
}

impl Provider {
    fn parse(value: &str) -> Result<Self, ApiError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "github" => Ok(Self::Github),
            "gitlab" => Ok(Self::Gitlab),
            "gitea" => Ok(Self::Gitea),
            "forgejo" => Ok(Self::Forgejo),
            _ => Err(ApiError::bad_request("Unsupported repository provider.")),
        }
    }

    const fn slug(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Gitea => "gitea",
            Self::Forgejo => "forgejo",
        }
    }
}

pub(super) struct ImportMetadataReport {
    pub(super) labels: usize,
    pub(super) releases: usize,
    pub(super) assets: usize,
    pub(super) skipped_assets: usize,
}

/// Imports provider metadata after the destination repository and its tags exist.
pub async fn import_repository_metadata(
    state: &RepositoryState,
    repository: &repository::Model,
    provider_slug: &str,
    instance_url: &str,
    source_id: &str,
    source_full_name: &str,
    token: &str,
    importer_user_id: Uuid,
) -> Result<ImportMetadataReport, ApiError> {
    let provider = Provider::parse(provider_slug)?;
    let instance = normalize_instance_url(provider, instance_url)?;
    let locator = source_locator(provider, source_id, source_full_name)?;
    if token.trim().is_empty() || token.len() > 8_192 {
        return Err(ApiError::bad_request("Enter a valid source access token."));
    }
    let client = ProviderClient::new(provider, instance.clone(), token).await?;
    let labels = client.list_labels(&locator).await?;
    let mut report = ImportMetadataReport {
        labels: 0,
        releases: 0,
        assets: 0,
        skipped_assets: 0,
    };

    for value in labels {
        let label = parse_label(provider, &value, instance.as_str())?;
        issues::upsert_imported_label(state, repository.id, label).await?;
        report.labels += 1;
    }

    let releases = client.list_releases(&locator).await?;
    for value in releases {
        let Some(release) = parse_release(provider, &value)? else {
            continue;
        };
        let imported_release = releases::ImportedRelease {
            external_source: provider.slug().to_owned(),
            external_instance_url: instance.to_string(),
            external_id: release.external_id,
            external_url: release.external_url,
            external_author: release.external_author,
            external_author_url: release.external_author_url,
            external_updated_at: release.external_updated_at,
            target_revision: release.tag_name,
            title: release.title,
            body: release.body,
            prerelease: release.prerelease,
            published_at: release.published_at,
            created_at: release.created_at,
        };
        let release_id = releases::upsert_imported_release(
            state,
            repository,
            importer_user_id,
            imported_release,
        )
        .await?
        .id;
        report.releases += 1;

        for asset in release.assets {
            if asset
                .size_bytes
                .is_some_and(|size| size > releases::MAX_RELEASE_ASSET_BYTES as i64)
            {
                report.skipped_assets += 1;
                continue;
            }
            if releases::imported_asset_exists(
                state,
                release_id,
                provider.slug(),
                instance.as_str(),
                &asset.external_id,
            )
            .await?
            {
                continue;
            }
            let response = client.download(&asset.download_url).await?;
            let imported_asset = releases::ImportedAsset {
                external_source: provider.slug().to_owned(),
                external_instance_url: instance.to_string(),
                external_id: asset.external_id,
                external_url: asset.download_url,
                name: asset.name,
                content_type: asset.content_type,
                expected_size: asset.size_bytes,
                external_updated_at: asset.external_updated_at,
            };
            releases::store_imported_asset(state, repository, release_id, imported_asset, response)
                .await?;
            report.assets += 1;
        }
    }
    Ok(report)
}

struct ProviderClient {
    provider: Provider,
    instance: Url,
    token: String,
}

impl ProviderClient {
    async fn new(provider: Provider, instance: Url, token: &str) -> Result<Self, ApiError> {
        let client = Self {
            provider,
            instance,
            token: token.to_owned(),
        };
        client.client_for(&client.instance).await?;
        Ok(client)
    }

    async fn list_labels(&self, locator: &str) -> Result<Vec<Value>, ApiError> {
        let url = self.repository_endpoint(locator, "labels")?;
        self.get_pages(url).await
    }

    async fn list_releases(&self, locator: &str) -> Result<Vec<Value>, ApiError> {
        let url = self.repository_endpoint(locator, "releases")?;
        self.get_pages(url).await
    }

    fn repository_endpoint(&self, locator: &str, suffix: &str) -> Result<Url, ApiError> {
        let mut segments = match self.provider {
            Provider::Github => vec!["repos".to_owned()],
            Provider::Gitea | Provider::Forgejo => {
                vec!["api".to_owned(), "v1".to_owned(), "repos".to_owned()]
            }
            Provider::Gitlab => vec!["api".to_owned(), "v4".to_owned(), "projects".to_owned()],
        };
        if matches!(
            self.provider,
            Provider::Github | Provider::Gitea | Provider::Forgejo
        ) {
            segments.extend(locator.split('/').map(str::to_owned));
        } else {
            segments.push(locator.to_owned());
        }
        segments.push(suffix.to_owned());
        self.endpoint(segments)
    }

    fn endpoint(&self, segments: Vec<String>) -> Result<Url, ApiError> {
        let mut url = self.instance.clone();
        let mut all_segments = self
            .instance
            .path_segments()
            .map(|segments| {
                segments
                    .filter(|segment| !segment.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        all_segments.extend(segments);
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| ApiError::bad_request("Source instance URL cannot be a base URL."))?;
            path.clear();
            path.extend(all_segments);
        }
        url.set_query(None);
        url.set_fragment(None);
        Ok(url)
    }

    async fn get_pages(&self, mut url: Url) -> Result<Vec<Value>, ApiError> {
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            url.query_pairs_mut()
                .clear()
                .append_pair("page", &page.to_string())
                .append_pair("per_page", &PAGE_SIZE.to_string())
                .append_pair("limit", &PAGE_SIZE.to_string());
            let values = self.get_json(url.clone()).await?;
            let values = values.as_array().ok_or_else(|| {
                ApiError::bad_request("Source provider returned an invalid metadata list.")
            })?;
            if values.is_empty() {
                return Ok(all);
            }
            all.extend(values.iter().cloned());
        }
        Err(ApiError::bad_request(
            "Source provider returned too many metadata pages.",
        ))
    }

    async fn get_json(&self, url: Url) -> Result<Value, ApiError> {
        let response = self
            .request(url, true)
            .await?
            .send()
            .await
            .map_err(|error| ApiError::bad_request(format!("Source request failed: {error}")))?;
        ensure_success(response).await
    }

    async fn download(&self, raw_url: &str) -> Result<Response, ApiError> {
        let mut url = Url::parse(raw_url)
            .map_err(|_| ApiError::bad_request("Source release asset URL is invalid."))?;
        for _ in 0..=MAX_REDIRECTS {
            validate_download_url(self.provider, &self.instance, &url)?;
            let authenticate = same_origin(&self.instance, &url);
            let response = self
                .request(url.clone(), authenticate)
                .await?
                .header(header::ACCEPT, "application/octet-stream")
                .send()
                .await
                .map_err(|error| {
                    ApiError::bad_request(format!("Source asset request failed: {error}"))
                })?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .ok_or_else(|| ApiError::bad_request("Source redirect has no location."))?
                    .to_str()
                    .map_err(|_| ApiError::bad_request("Source redirect location is invalid."))?;
                url = url
                    .join(location)
                    .map_err(|_| ApiError::bad_request("Source redirect location is invalid."))?;
                continue;
            }
            if !response.status().is_success() {
                return Err(ApiError::bad_request(format!(
                    "Source asset request returned HTTP {}.",
                    response.status()
                )));
            }
            return Ok(response);
        }
        Err(ApiError::bad_request(
            "Source asset redirected too many times.",
        ))
    }

    async fn request(
        &self,
        url: Url,
        authenticate: bool,
    ) -> Result<reqwest::RequestBuilder, ApiError> {
        let client = self.client_for(&url).await?;
        let mut request = client
            .get(url)
            .header(header::USER_AGENT, "Gitadel repository importer")
            .header(header::ACCEPT, "application/json");
        if authenticate {
            request = match self.provider {
                Provider::Github => request
                    .header(
                        header::AUTHORIZATION,
                        header_value(&format!("Bearer {}", self.token))?,
                    )
                    .header("x-github-api-version", "2022-11-28"),
                Provider::Gitlab => request.header("PRIVATE-TOKEN", header_value(&self.token)?),
                Provider::Gitea | Provider::Forgejo => request.header(
                    header::AUTHORIZATION,
                    header_value(&format!("token {}", self.token))?,
                ),
            };
        }
        Ok(request)
    }

    async fn client_for(&self, url: &Url) -> Result<reqwest::Client, ApiError> {
        let host = url
            .host_str()
            .ok_or_else(|| ApiError::bad_request("Source URL has no host."))?;
        let port = url
            .port_or_known_default()
            .unwrap_or_else(|| if url.scheme() == "http" { 80 } else { 443 });
        let addresses = lookup_host((host, port))
            .await
            .map_err(|error| {
                ApiError::bad_request(format!("Could not resolve source host: {error}"))
            })?
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(ApiError::bad_request(
                "Source URL did not resolve to an address.",
            ));
        }
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(ApiError::internal)
    }
}

async fn ensure_success(response: Response) -> Result<Value, ApiError> {
    let status = response.status();
    if !status.is_success() {
        return Err(ApiError::bad_request(format!(
            "Source provider returned HTTP {status}."
        )));
    }
    response
        .json()
        .await
        .map_err(|error| ApiError::bad_request(format!("Source returned invalid JSON: {error}")))
}

fn header_value(value: &str) -> Result<reqwest::header::HeaderValue, ApiError> {
    reqwest::header::HeaderValue::from_str(value)
        .map_err(|_| ApiError::bad_request("The source token is not a valid HTTP header value."))
}

struct ParsedRelease {
    external_id: String,
    external_url: Option<String>,
    external_author: Option<String>,
    external_author_url: Option<String>,
    external_updated_at: Option<DateTime<Utc>>,
    tag_name: String,
    title: String,
    body: String,
    prerelease: bool,
    published_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    assets: Vec<ParsedAsset>,
}

struct ParsedAsset {
    external_id: String,
    download_url: String,
    name: String,
    content_type: String,
    size_bytes: Option<i64>,
    external_updated_at: Option<DateTime<Utc>>,
}

fn parse_label(
    provider: Provider,
    value: &Value,
    instance_url: &str,
) -> Result<issues::ImportedLabel, ApiError> {
    let name = bounded(
        required_text(value, "name", "source label")?
            .trim()
            .to_owned(),
        64,
        "source label name",
    );
    if name.is_empty() {
        return Err(ApiError::bad_request("Source label has an empty name."));
    }
    let color = normalize_color(required_text(value, "color", "source label")?)?;
    let external_id = value
        .get("id")
        .and_then(value_id)
        .unwrap_or_else(|| format!("name:{name}"));
    if external_id.len() > 255 {
        return Err(ApiError::bad_request("Source label id is too long."));
    }
    let external_url = optional_text(value, "url").or_else(|| optional_text(value, "html_url"));
    Ok(issues::ImportedLabel {
        external_source: provider.slug().to_owned(),
        external_instance_url: instance_url.to_owned(),
        external_id,
        external_url,
        name,
        color,
        description: bounded(
            optional_text(value, "description").unwrap_or_default(),
            255,
            "label description",
        ),
        external_updated_at: None,
    })
}

fn parse_release(provider: Provider, value: &Value) -> Result<Option<ParsedRelease>, ApiError> {
    if value.get("draft").and_then(Value::as_bool).unwrap_or(false)
        || value
            .get("upcoming_release")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Ok(None);
    }
    let tag_name = required_text(value, "tag_name", "source release")?;
    if tag_name.is_empty() || tag_name.len() > 255 || tag_name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("Source release has an invalid tag."));
    }
    let created_at = optional_datetime(value, "created_at");
    let published_at = optional_datetime(value, "published_at")
        .or_else(|| optional_datetime(value, "released_at"))
        .or(created_at)
        .unwrap_or_else(Utc::now);
    if published_at > Utc::now() {
        return Ok(None);
    }
    let external_id = value
        .get("id")
        .and_then(value_id)
        .unwrap_or_else(|| format!("tag:{tag_name}"));
    if external_id.len() > 255 {
        return Err(ApiError::bad_request("Source release id is too long."));
    }
    let title = bounded(
        optional_text(value, "name")
            .or_else(|| optional_text(value, "title"))
            .unwrap_or_else(|| tag_name.clone()),
        255,
        "release title",
    );
    let body = bounded(
        optional_text(value, "body")
            .or_else(|| optional_text(value, "description"))
            .unwrap_or_default(),
        1_000_000,
        "release notes",
    );
    let author = value.get("author").or_else(|| value.get("release_author"));
    let external_author = author
        .and_then(|author| {
            optional_text(author, "login")
                .or_else(|| optional_text(author, "username"))
                .or_else(|| optional_text(author, "name"))
        })
        .map(|author| bounded(author, 255, "release author"));
    let external_author_url = author
        .and_then(|author| {
            optional_text(author, "html_url").or_else(|| optional_text(author, "web_url"))
        })
        .map(|url| bounded(url, 2048, "release author URL"));
    let external_url = optional_text(value, "html_url").or_else(|| {
        value
            .get("_links")
            .and_then(|links| optional_text(links, "self"))
    });
    let assets = release_assets(provider, value)?;
    Ok(Some(ParsedRelease {
        external_id,
        external_url,
        external_author,
        external_author_url,
        external_updated_at: optional_datetime(value, "updated_at").or(created_at),
        tag_name,
        title,
        body,
        prerelease: value
            .get("prerelease")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        published_at,
        created_at: created_at.unwrap_or(published_at),
        assets,
    }))
}

fn release_assets(provider: Provider, value: &Value) -> Result<Vec<ParsedAsset>, ApiError> {
    let values = value
        .get("assets")
        .and_then(|assets| assets.get("links"))
        .or_else(|| value.get("assets"));
    let Some(values) = values.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    values
        .iter()
        .filter_map(|asset| {
            let download_url = match provider {
                Provider::Github => optional_text(asset, "url")
                    .or_else(|| optional_text(asset, "browser_download_url")),
                Provider::Gitlab => {
                    optional_text(asset, "direct_asset_url").or_else(|| optional_text(asset, "url"))
                }
                Provider::Gitea | Provider::Forgejo => optional_text(asset, "browser_download_url")
                    .or_else(|| optional_text(asset, "url")),
            }?;
            if is_generated_archive(provider, &download_url) {
                return None;
            }
            Some(parse_asset(asset, download_url))
        })
        .collect()
}

fn parse_asset(value: &Value, download_url: String) -> Result<ParsedAsset, ApiError> {
    let name = bounded(
        required_text(value, "name", "source release asset")?,
        255,
        "asset name",
    );
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(ApiError::bad_request(
            "Source release asset has an invalid name.",
        ));
    }
    let external_id = value
        .get("id")
        .and_then(value_id)
        .unwrap_or_else(|| bounded(download_url.clone(), 255, "asset URL"));
    if external_id.len() > 255 {
        return Err(ApiError::bad_request(
            "Source release asset id is too long.",
        ));
    }
    let size_bytes = value
        .get("size")
        .and_then(Value::as_i64)
        .or_else(|| value.get("size_bytes").and_then(Value::as_i64));
    Ok(ParsedAsset {
        external_id,
        download_url,
        name,
        content_type: bounded(
            optional_text(value, "content_type")
                .unwrap_or_else(|| "application/octet-stream".to_owned()),
            255,
            "asset content type",
        ),
        size_bytes,
        external_updated_at: optional_datetime(value, "updated_at")
            .or_else(|| optional_datetime(value, "created_at")),
    })
}

fn source_locator(
    provider: Provider,
    source_id: &str,
    source_full_name: &str,
) -> Result<String, ApiError> {
    let full_name = source_full_name.trim().trim_matches('/');
    if full_name.is_empty() || full_name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("Source repository name is invalid."));
    }
    if source_id.len() > 255 || source_id.chars().any(char::is_control) {
        return Err(ApiError::bad_request("Source repository id is invalid."));
    }
    match provider {
        Provider::Gitlab if !source_id.trim().is_empty() => Ok(source_id.trim().to_owned()),
        Provider::Github | Provider::Gitea | Provider::Forgejo => {
            let parts = full_name.split('/').collect::<Vec<_>>();
            if parts.len() != 2 || parts.iter().any(|part| part.is_empty()) {
                return Err(ApiError::bad_request(
                    "Source repository must have an owner and name.",
                ));
            }
            Ok(full_name.to_owned())
        }
        Provider::Gitlab => Ok(full_name.to_owned()),
    }
}

fn normalize_instance_url(provider: Provider, raw: &str) -> Result<Url, ApiError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ApiError::bad_request("Enter the source instance URL."));
    }
    let mut url =
        Url::parse(raw).map_err(|_| ApiError::bad_request("Source instance URL is invalid."))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ApiError::bad_request(
            "Source instance URL must be HTTP or HTTPS without credentials.",
        ));
    }
    if provider == Provider::Github && url.host_str() != Some("api.github.com") {
        return Err(ApiError::bad_request(
            "GitHub imports currently use api.github.com.",
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    Ok(url)
}

fn validate_download_url(provider: Provider, instance: &Url, url: &Url) -> Result<(), ApiError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ApiError::bad_request(
            "Source release asset URL is not safe to download.",
        ));
    }
    if same_origin(instance, url) {
        return Ok(());
    }
    let github_cdn = provider == Provider::Github
        && instance.scheme() == "https"
        && instance.host_str() == Some("api.github.com")
        && url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "api.github.com"
                    | "objects.githubusercontent.com"
                    | "release-assets.githubusercontent.com"
                    | "github-releases.githubusercontent.com"
            )
        );
    if github_cdn {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "Source release asset URL is outside the source origin.",
        ))
    }
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme().eq_ignore_ascii_case(right.scheme())
        && left
            .host_str()
            .zip(right.host_str())
            .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
        && left.port_or_known_default() == right.port_or_known_default()
}

fn is_generated_archive(provider: Provider, url: &str) -> bool {
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    let path = url.path().to_ascii_lowercase();
    match provider {
        Provider::Github => {
            path.contains("/archive/refs/")
                || path.contains("/archive/")
                || path.contains("/zipball/")
                || path.contains("/tarball/")
        }
        Provider::Gitlab => path.contains("/-/archive/") || path.contains("/repository/archive/"),
        Provider::Gitea | Provider::Forgejo => path.contains("/archive/"),
    }
}

fn required_text(value: &Value, key: &str, subject: &str) -> Result<String, ApiError> {
    optional_text(value, key)
        .ok_or_else(|| ApiError::bad_request(format!("{subject} is missing {key}.")))
}

fn optional_text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

fn value_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
}

fn optional_datetime(value: &Value, key: &str) -> Option<DateTime<Utc>> {
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn normalize_color(value: String) -> Result<String, ApiError> {
    let value = value.trim().trim_start_matches('#').to_ascii_lowercase();
    if value.len() == 6 && value.chars().all(|character| character.is_ascii_hexdigit()) {
        Ok(value)
    } else {
        Err(ApiError::bad_request("Source label has an invalid color."))
    }
}

fn bounded(value: String, max: usize, subject: &str) -> String {
    if value.len() <= max {
        return value;
    }
    tracing::warn!(subject, "truncating oversized imported metadata field");
    let end = value
        .char_indices()
        .take_while(|(index, character)| index + character.len_utf8() <= max)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_label_normalizes_provider_color() {
        let label = parse_label(
            Provider::Github,
            &json!({"id": 7, "name": "bug", "color": "#F29513"}),
            "https://api.github.com",
        )
        .expect("label should parse");
        assert_eq!(label.color, "f29513");
    }

    #[test]
    fn parse_gitlab_release_ignores_generated_sources() {
        let release = parse_release(
            Provider::Gitlab,
            &json!({
                "tag_name": "v1.0.0",
                "name": "First",
                "description": "notes",
                "released_at": "2026-01-01T00:00:00Z",
                "assets": {
                    "sources": [{"format": "zip", "url": "https://gitlab.example/a/-/archive/v1/a.zip"}],
                    "links": [{"id": 3, "name": "linux", "url": "https://gitlab.example/download/linux"}]
                }
            }),
        )
        .expect("release should parse")
        .expect("release is published");
        assert_eq!(release.assets.len(), 1);
    }

    #[test]
    fn generated_archive_paths_are_skipped_without_skipping_uploaded_archives() {
        assert!(is_generated_archive(
            Provider::Github,
            "https://github.com/a/b/archive/refs/tags/v1.tar.gz"
        ));
        assert!(!is_generated_archive(
            Provider::Github,
            "https://github.com/a/b/releases/download/v1/tool.tar.gz"
        ));
    }

    #[test]
    fn unsafe_redirect_origin_is_rejected() {
        let instance = Url::parse("https://gitlab.example").expect("valid URL");
        let evil = Url::parse("https://evil.example/asset").expect("valid URL");
        assert!(validate_download_url(Provider::Gitlab, &instance, &evil).is_err());
    }

    #[test]
    fn gitlab_source_id_is_preferred_over_nested_full_name() {
        assert_eq!(
            source_locator(Provider::Gitlab, "42", "group/subgroup/repo")
                .expect("locator should parse"),
            "42"
        );
    }

    #[test]
    fn repository_endpoint_keeps_owner_and_name_as_path_segments() {
        let client = ProviderClient {
            provider: Provider::Github,
            instance: Url::parse("https://api.github.com").expect("valid URL"),
            token: "token".to_owned(),
        };
        let url = client
            .repository_endpoint("octocat/hello-world", "releases")
            .expect("endpoint should parse");
        assert_eq!(url.path(), "/repos/octocat/hello-world/releases");
    }

    #[test]
    fn gitea_repository_endpoint_includes_the_api_prefix() {
        let client = ProviderClient {
            provider: Provider::Gitea,
            instance: Url::parse("http://forge.local:3000").expect("valid URL"),
            token: "token".to_owned(),
        };
        let url = client
            .repository_endpoint("team/project", "labels")
            .expect("endpoint should parse");
        assert_eq!(url.path(), "/api/v1/repos/team/project/labels");
    }

    #[test]
    fn github_release_assets_prefer_the_authenticated_api_url() {
        let assets = release_assets(
            Provider::Github,
            &json!({
                "assets": [{
                    "id": 7,
                    "name": "tool",
                    "url": "https://api.github.com/repos/team/project/releases/assets/7",
                    "browser_download_url": "https://github.com/team/project/releases/download/v1/tool"
                }]
            }),
        )
        .expect("assets should parse");
        assert_eq!(
            assets[0].download_url,
            "https://api.github.com/repos/team/project/releases/assets/7"
        );
    }
}
