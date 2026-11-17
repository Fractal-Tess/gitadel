//! The REST contract is collected by the same routers that serve requests.
//! Exporting it requires no settings, database, network or running service.

use axum::{
    http::{HeaderValue, header},
    response::IntoResponse,
};
use std::sync::LazyLock;
use utoipa::openapi::{
    InfoBuilder, OpenApi, ServerBuilder,
    security::{
        ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme,
    },
};
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouter};

mod tags;
mod typed;
pub(crate) use typed::{ContractRouterExt, TypedResponse, refine};

macro_rules! routes {
    ($($handler:path),+ $(,)?) => {{
        let mut router = utoipa_axum::routes!($($handler),+);
        $(crate::openapi::refine(&mut router, &$handler, utoipa_axum::routes!($handler));)+
        router
    }};
}
pub(crate) use routes;

/// Native HTTP bytes; the SDK must not model these as a JSON integer array.
pub(crate) struct Binary;

impl utoipa::ToSchema for Binary {}
impl utoipa::PartialSchema for Binary {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::Schema> {
        use utoipa::openapi::{
            KnownFormat, SchemaFormat,
            schema::{ObjectBuilder, SchemaType, Type},
        };
        ObjectBuilder::new()
            .schema_type(SchemaType::new(Type::String))
            .format(Some(SchemaFormat::KnownFormat(KnownFormat::Binary)))
            .build()
            .into()
    }
}

pub(crate) fn instance_router() -> OpenApiRouter {
    OpenApiRouter::new()
        .api_routes(routes!(crate::api::version))
        .api_routes(routes!(crate::api::forgejo_version))
        .api_routes(routes!(crate::api::changelog))
        .api_routes(routes!(openapi_json))
}

/// An existing compatibility URL serving the exact same handler and DTOs.
pub(crate) fn alias<S: Clone + Send + Sync + 'static>(
    path: &str,
    (schemas, mut paths, handler): UtoipaMethodRouter<S>,
) -> UtoipaMethodRouter<S> {
    let (original, mut item) = paths.paths.pop_first().expect("alias has one source path");
    let parameters = |p: &str| {
        p.split('/')
            .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}'))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let original_parameters = parameters(&original);
    let alias_parameters = parameters(path);
    for (method, operation) in [
        ("get", &mut item.get),
        ("post", &mut item.post),
        ("put", &mut item.put),
        ("patch", &mut item.patch),
        ("delete", &mut item.delete),
    ] {
        if let Some(operation) = operation {
            operation.operation_id = Some(format!(
                "{method}_{}",
                path.split(|c: char| !c.is_ascii_alphanumeric())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("_")
            ));
            for parameter in operation.parameters.iter_mut().flatten() {
                if let utoipa::openapi::RefOr::T(parameter) = parameter
                    && let Some(index) = original_parameters
                        .iter()
                        .position(|name| name == &parameter.name)
                {
                    parameter.name = alias_parameters[index].clone();
                }
            }
        }
    }
    paths.paths.insert(path.to_owned(), item);
    (schemas, paths, handler)
}

fn merge_contract(api: &mut OpenApi, other: OpenApi) {
    if let (Some(known), Some(incoming)) = (&api.components, &other.components) {
        for (name, schema) in &incoming.schemas {
            if let Some(previous) = known.schemas.get(name) {
                assert!(
                    previous == schema,
                    "different Rust DTOs share schema name {name}"
                );
            }
        }
    }
    api.merge(other);
}

pub(crate) fn document() -> OpenApi {
    let mut api = instance_router().into_openapi();
    // Routers carry no resources until their state is supplied. Collect their
    // descriptions independently so this command never initializes an instance.
    merge_contract(&mut api, crate::identity::contract_router().into_openapi());
    merge_contract(
        &mut api,
        crate::repository::contract_router().into_openapi(),
    );
    merge_contract(&mut api, crate::actions::contract_router().into_openapi());
    api.info = InfoBuilder::new().title("Gitadel API")
        .version(env!("CARGO_PKG_VERSION"))
        .description(Some("REST API. Paths are relative to /api/v1. Errors use {error: {code, message}}. Git, registry, OAuth token and runner protocols are separate."))
        .build();
    api.servers = Some(vec![ServerBuilder::new().url("/api/v1").build()]);
    api.info.license = Some(
        utoipa::openapi::LicenseBuilder::new()
            .name("MIT")
            .identifier(Some("MIT"))
            .build(),
    );
    api.tags = Some(
        tags::TAGS
            .iter()
            .map(|(name, description)| {
                utoipa::openapi::tag::TagBuilder::new()
                    .name(*name)
                    .description(Some(*description))
                    .build()
            })
            .collect(),
    );
    let components = api.components.get_or_insert_with(Default::default);
    // IntoParams enum references and a default generic row are not traversed
    // by utoipa-axum's handler-body collector.
    use utoipa::{PartialSchema, ToSchema};
    let mut extra = Vec::new();
    crate::identity::admin_storage::RepositoryUsageRow::schemas(&mut extra);
    extra.push((
        crate::identity::admin_storage::RepositoryUsageRow::name().into_owned(),
        crate::identity::admin_storage::RepositoryUsageRow::schema(),
    ));
    crate::identity::admin_storage::legacy::RegistryRepositoryUsageRow::schemas(&mut extra);
    extra.push((
        crate::identity::admin_storage::legacy::RegistryRepositoryUsageRow::name().into_owned(),
        crate::identity::admin_storage::legacy::RegistryRepositoryUsageRow::schema(),
    ));
    extra.push((
        crate::repository::source_archive::SourceArchiveFormat::name().into_owned(),
        crate::repository::source_archive::SourceArchiveFormat::schema(),
    ));
    components.schemas.extend(extra);
    components.add_security_scheme(
        "token",
        SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
    );
    components.add_security_scheme(
        "session",
        SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(
            crate::identity::SESSION_COOKIE,
        ))),
    );
    api.security = Some(vec![
        SecurityRequirement::new("token", Vec::<String>::new()),
        SecurityRequirement::new("session", Vec::<String>::new()),
    ]);
    api
}

/// Canonical key ordering makes the checked-in artifact independent of map
/// insertion order and permits byte-for-byte drift checks.
pub(crate) fn json() -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&serde_json::to_value(document())?).map(|s| s + "\n")
}

static DOCUMENT: LazyLock<String> =
    LazyLock::new(|| json().expect("the OpenAPI document serializes"));

#[utoipa::path(
    get, path = "/openapi.json", operation_id = "get_openapi_json",
    summary = "This OpenAPI document",
    tag = "Instance", security(),
    responses((status = 200, description = "OpenAPI document; consume over native HTTP"))
)]
pub async fn openapi_json() -> impl IntoResponse + TypedResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=300"),
            ),
        ],
        DOCUMENT.as_str(),
    )
}

#[cfg(test)]
mod tests;
