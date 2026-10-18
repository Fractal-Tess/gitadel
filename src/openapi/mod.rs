//! OpenAPI description of the `/api/v1` HTTP API.
//!
//! Every operation the API routers register is listed in [`operations`],
//! grouped by area. Request and response bodies refer to the Rust types the
//! handlers use, through their `ToSchema` derives, so renaming a type breaks
//! the build instead of silently leaving the description behind. A test
//! compares the listed operations with the routers' source and fails when a
//! route is added or removed without updating this module.
//!
//! Git over HTTP, the container registry, the OAuth endpoints, and the Actions
//! runner protocol are separate protocols and are not described here.

use std::sync::LazyLock;

use axum::{
    http::{HeaderValue, header},
    response::IntoResponse,
};
use utoipa::{
    ToSchema,
    openapi::{
        ComponentsBuilder, ContentBuilder, HttpMethod, InfoBuilder, LicenseBuilder, OpenApi,
        OpenApiBuilder, PathItem, PathsBuilder, Ref, RefOr, ResponseBuilder, Schema, ServerBuilder,
        path::{OperationBuilder, ParameterBuilder, ParameterIn},
        request_body::RequestBodyBuilder,
        schema::{ArrayBuilder, ObjectBuilder, SchemaType, Type},
        security::{
            ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme,
        },
        tag::TagBuilder,
    },
};

mod operations;

pub(crate) use operations::operations;

/// Named schemas collected while building the document.
type Schemas = Vec<(String, RefOr<Schema>)>;
/// Produces a body's schema and registers every named schema it refers to.
type SchemaFn = fn(&mut Schemas) -> RefOr<Schema>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    fn http(self) -> HttpMethod {
        match self {
            Self::Get => HttpMethod::Get,
            Self::Post => HttpMethod::Post,
            Self::Put => HttpMethod::Put,
            Self::Patch => HttpMethod::Patch,
            Self::Delete => HttpMethod::Delete,
        }
    }
}

/// Who may call an operation.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Auth {
    /// Anyone; credentials are ignored.
    Public,
    /// Anyone, but signed-in callers may see more (private repositories).
    Optional,
    /// A session cookie or an API token is required.
    Required,
}

/// A request or response body.
#[derive(Clone, Copy)]
pub(crate) enum Body {
    /// Not described yet; the operation still exchanges JSON.
    Unspecified,
    /// No body (requests) or `204 No Content` (responses).
    Empty,
    /// JSON described by a Rust type.
    Json(SchemaFn),
    /// Raw bytes, such as an uploaded image or a downloaded file.
    Binary(&'static str),
    /// Server-sent events.
    EventStream,
}

pub(crate) struct Operation {
    pub method: Method,
    /// Path below `/api/v1`, in the routers' `{param}` syntax.
    pub path: &'static str,
    pub tag: &'static str,
    pub summary: &'static str,
    pub auth: Auth,
    pub request: Body,
    pub response: Body,
    /// Query parameters, as `(name, description)`.
    pub query: &'static [(&'static str, &'static str)],
    /// Success status: 200 (204 without a body), 201 when the operation
    /// creates something and says so, or 202 when it queues work.
    pub status: u16,
}

impl Operation {
    pub(crate) const fn new(
        method: Method,
        path: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self {
            method,
            path,
            tag,
            summary,
            auth: Auth::Required,
            request: match method {
                Method::Get | Method::Delete => Body::Empty,
                _ => Body::Unspecified,
            },
            response: Body::Unspecified,
            query: &[],
            status: 200,
        }
    }

    /// The operation answers `202 Accepted`: the work is queued.
    pub(crate) const fn accepted(mut self) -> Self {
        self.status = 202;
        self
    }

    /// The operation answers `201 Created` with its body.
    pub(crate) const fn created(mut self) -> Self {
        self.status = 201;
        self
    }

    pub(crate) const fn auth(mut self, auth: Auth) -> Self {
        self.auth = auth;
        self
    }

    pub(crate) const fn request(mut self, body: Body) -> Self {
        self.request = body;
        self
    }

    pub(crate) const fn response(mut self, body: Body) -> Self {
        self.response = body;
        self
    }

    pub(crate) const fn query(mut self, query: &'static [(&'static str, &'static str)]) -> Self {
        self.query = query;
        self
    }
}

/// The JSON body of one Rust type.
pub(crate) const fn json<T: ToSchema>() -> Body {
    Body::Json(named::<T>)
}

/// A JSON array of one Rust type.
pub(crate) const fn json_list<T: ToSchema>() -> Body {
    Body::Json(list::<T>)
}

fn named<T: ToSchema>(schemas: &mut Schemas) -> RefOr<Schema> {
    T::schemas(schemas);
    let name = T::name().into_owned();
    schemas.push((name.clone(), T::schema()));
    Ref::from_schema_name(name).into()
}

fn list<T: ToSchema>(schemas: &mut Schemas) -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(named::<T>(schemas))
        .build()
        .into()
}

/// Tags in display order, with the text shown above each group.
const TAGS: &[(&str, &str)] = &[
    (
        "Instance",
        "Version, changelog, public instance settings, and first-run setup.",
    ),
    (
        "Authentication",
        "Sign-in, sign-up, password reset, passkeys, and single sign-on.",
    ),
    (
        "Account",
        "The signed-in user: profile, emails, keys, tokens, and preferences.",
    ),
    ("Organizations", "Organizations and their members."),
    (
        "Namespaces",
        "Users and organizations as repository owners: profile, activity, and pins.",
    ),
    (
        "Repositories",
        "Creating, listing, configuring, archiving, and deleting repositories.",
    ),
    (
        "Repository content",
        "Trees, files, commits, history, references, and rendered content.",
    ),
    ("Collaborators", "Repository collaborators and deploy keys."),
    (
        "Branch protection",
        "Rules that restrict pushes to matching branches.",
    ),
    ("Issues", "Issues, comments, labels, and attachments."),
    ("Releases", "Releases and their assets."),
    (
        "Commit statuses",
        "Statuses reported for commits, Gitea-compatible.",
    ),
    ("Webhooks", "Repository webhooks and their deliveries."),
    ("Mirrors", "Pull mirrors and the credentials they use."),
    ("Imports", "Importing repositories from other forges."),
    (
        "Deployments",
        "Dokploy connections and repository deployments.",
    ),
    (
        "Actions",
        "Workflows, runs, runners, secrets, and variables.",
    ),
    (
        "Administration",
        "Instance administration: users, authentication, email, and integrity.",
    ),
    ("Storage", "Storage targets, domains, and migrations."),
    ("Backups", "Backup providers, schedules, and restores."),
    (
        "Gitea compatibility",
        "Endpoints shaped like Gitea's API, for existing clients.",
    ),
];

const TOKEN_SCHEME: &str = "token";
const SESSION_SCHEME: &str = "session";

fn path_parameters(path: &str) -> impl Iterator<Item = &str> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
}

fn body_content(body: Body, schemas: &mut Schemas) -> Option<(String, utoipa::openapi::Content)> {
    match body {
        Body::Unspecified => Some((
            "application/json".to_owned(),
            ContentBuilder::new()
                .schema(Some(ObjectBuilder::new().build()))
                .build(),
        )),
        Body::Empty => None,
        Body::Json(schema) => Some((
            "application/json".to_owned(),
            ContentBuilder::new().schema(Some(schema(schemas))).build(),
        )),
        Body::Binary(media_type) => Some((
            media_type.to_owned(),
            ContentBuilder::new()
                .schema(Some(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .format(Some(utoipa::openapi::SchemaFormat::KnownFormat(
                            utoipa::openapi::KnownFormat::Binary,
                        )))
                        .build(),
                ))
                .build(),
        )),
        Body::EventStream => Some((
            "text/event-stream".to_owned(),
            ContentBuilder::new()
                .schema(Some(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .build(),
                ))
                .build(),
        )),
    }
}

fn operation(operation: &Operation, schemas: &mut Schemas) -> utoipa::openapi::path::Operation {
    let mut builder = OperationBuilder::new()
        .tag(operation.tag)
        .summary(Some(operation.summary))
        .operation_id(Some(operation_id(operation)));
    for name in path_parameters(operation.path) {
        builder = builder.parameter(
            ParameterBuilder::new()
                .name(name)
                .parameter_in(ParameterIn::Path)
                .required(utoipa::openapi::Required::True)
                .schema(Some(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .build(),
                )),
        );
    }
    for (name, description) in operation.query {
        builder = builder.parameter(
            ParameterBuilder::new()
                .name(*name)
                .parameter_in(ParameterIn::Query)
                .required(utoipa::openapi::Required::False)
                .description(Some(*description))
                .schema(Some(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .build(),
                )),
        );
    }
    if let Some((media_type, content)) = body_content(operation.request, schemas) {
        builder = builder.request_body(Some(
            RequestBodyBuilder::new()
                .content(media_type, content)
                .required(Some(utoipa::openapi::Required::True))
                .build(),
        ));
    }
    let success = match body_content(operation.response, schemas) {
        Some((media_type, content)) => (
            if operation.status == 201 {
                "201"
            } else {
                "200"
            },
            ResponseBuilder::new()
                .description("Success.")
                .content(media_type, content)
                .build(),
        ),
        None if operation.status == 201 => (
            "201",
            ResponseBuilder::new()
                .description("Created; no content.")
                .build(),
        ),
        None if operation.status == 202 => (
            "202",
            ResponseBuilder::new()
                .description("Accepted; the work continues in the background.")
                .build(),
        ),
        None => (
            "204",
            ResponseBuilder::new()
                .description("Done; no content.")
                .build(),
        ),
    };
    let error = |description: &str, schemas: &mut Schemas| {
        ResponseBuilder::new()
            .description(description)
            .content(
                "application/json",
                ContentBuilder::new()
                    .schema(Some(named::<ErrorEnvelope>(schemas)))
                    .build(),
            )
            .build()
    };
    builder = builder
        .response(success.0, success.1)
        .response(
            "4XX",
            error(
                "The request was refused: invalid input (400), missing or insufficient \
                 credentials (401, 403), not found (404), or a conflict (409).",
                schemas,
            ),
        )
        .response(
            "5XX",
            error(
                "The server or a remote service it called failed (500, 502).",
                schemas,
            ),
        );
    builder = match operation.auth {
        Auth::Public => builder.securities(Some(Vec::<SecurityRequirement>::new())),
        Auth::Optional => builder.securities(Some([
            SecurityRequirement::new(TOKEN_SCHEME, Vec::<String>::new()),
            SecurityRequirement::new(SESSION_SCHEME, Vec::<String>::new()),
            SecurityRequirement::default(),
        ])),
        Auth::Required => builder,
    };
    builder.build()
}

/// A stable identifier such as `get_repositories_namespace_name_issues`.
fn operation_id(operation: &Operation) -> String {
    let method = format!("{:?}", operation.method).to_ascii_lowercase();
    let path: Vec<String> = operation
        .path
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    if path.is_empty() {
        method
    } else {
        format!("{method}_{}", path.join("_"))
    }
}

/// The error body every failed request returns.
#[derive(ToSchema)]
#[expect(
    dead_code,
    reason = "only describes the error shape for the OpenAPI document"
)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(ToSchema)]
#[expect(
    dead_code,
    reason = "only describes the error shape for the OpenAPI document"
)]
struct ErrorBody {
    /// Machine-readable code, such as `not_found` or `bad_request`.
    code: String,
    /// A sentence meant for people.
    message: String,
}

pub(crate) fn document() -> OpenApi {
    let mut schemas = Schemas::new();
    let mut paths = PathsBuilder::new();
    let mut listed: Vec<&Operation> = operations().collect();
    listed.sort_by_key(|operation| (operation.path, operation.method));
    let mut current: Option<(&str, PathItem)> = None;
    for entry in listed {
        let built = operation(entry, &mut schemas);
        current = match current {
            Some((path, item)) if path == entry.path => {
                Some((path, merge(item, entry.method.http(), built)))
            }
            Some((path, item)) => {
                paths = paths.path(path, item);
                Some((entry.path, PathItem::new(entry.method.http(), built)))
            }
            None => Some((entry.path, PathItem::new(entry.method.http(), built))),
        };
    }
    if let Some((path, item)) = current {
        paths = paths.path(path, item);
    }

    let components = ComponentsBuilder::new()
        .schemas_from_iter(schemas)
        .security_scheme(
            TOKEN_SCHEME,
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "An API token (`gtd_…`) from Account settings → API tokens, or an \
                         OAuth access token. `Authorization: token …` is also accepted.",
                    ))
                    .build(),
            ),
        )
        .security_scheme(
            SESSION_SCHEME,
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                crate::identity::SESSION_COOKIE,
                "The browser session. Cross-site requests that rely on it are refused.",
            ))),
        )
        .build();

    OpenApiBuilder::new()
        .info(
            InfoBuilder::new()
                .title("Gitadel API")
                .version(env!("CARGO_PKG_VERSION"))
                .description(Some(
                    "The HTTP API behind Gitadel's web interface and the `gtd` command-line \
                     client. Paths are relative to `/api/v1`. Every error has the same \
                     `{\"error\": {\"code\", \"message\"}}` shape.",
                ))
                .license(Some(
                    LicenseBuilder::new()
                        .name("MIT")
                        .identifier(Some("MIT"))
                        .build(),
                ))
                .build(),
        )
        .servers(Some([ServerBuilder::new().url("/api/v1").build()]))
        .paths(paths.build())
        .components(Some(components))
        .security(Some([
            SecurityRequirement::new(TOKEN_SCHEME, Vec::<String>::new()),
            SecurityRequirement::new(SESSION_SCHEME, Vec::<String>::new()),
        ]))
        .tags(Some(TAGS.iter().map(|(name, description)| {
            TagBuilder::new()
                .name(*name)
                .description(Some(*description))
                .build()
        })))
        .build()
}

fn merge(
    item: PathItem,
    method: HttpMethod,
    operation: utoipa::openapi::path::Operation,
) -> PathItem {
    let mut merged = item;
    merged.merge_operations(PathItem::new(method, operation));
    merged
}

/// The document is identical for every caller, so it is rendered once.
static DOCUMENT: LazyLock<String> =
    LazyLock::new(|| serde_json::to_string(&document()).expect("the OpenAPI document serializes"));

/// `GET /api/v1/openapi.json`
pub async fn openapi_json() -> impl IntoResponse {
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
