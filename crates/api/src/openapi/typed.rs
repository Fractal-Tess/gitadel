//! The Axum handler's Rust output is authoritative for JSON response bodies.
//! Keep utoipa-axum's routing/parameter/status machinery, but collect the actual
//! Json<T> even when an older response annotation still mentions a different DTO.

use std::future::Future;
use utoipa::{
    ToSchema,
    openapi::{RefOr, Schema},
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::router::UtoipaMethodRouter;

type Schemas = Vec<(String, RefOr<Schema>)>;

pub(crate) trait ContractRouterExt<S> {
    fn api_routes(self, routes: UtoipaMethodRouter<S>) -> Self;
}
impl<S: Clone + Send + Sync + 'static> ContractRouterExt<S> for OpenApiRouter<S> {
    fn api_routes(self, routes: UtoipaMethodRouter<S>) -> Self {
        let mut known = self
            .get_openapi()
            .components
            .as_ref()
            .map(|c| c.schemas.clone())
            .unwrap_or_default();
        for (name, schema) in &routes.0 {
            if let Some(previous) = known.insert(name.clone(), schema.clone()) {
                assert!(
                    previous == *schema,
                    "different Rust DTOs share schema name {name}"
                );
            }
        }
        self.routes(routes)
    }
}

pub(crate) trait TypedResponse {
    fn json_schema(schemas: &mut Schemas) -> Option<RefOr<Schema>>;
}

impl<T: ToSchema> TypedResponse for axum::Json<T> {
    fn json_schema(schemas: &mut Schemas) -> Option<RefOr<Schema>> {
        T::schemas(schemas);
        Some(T::schema())
    }
}

impl<T: TypedResponse, E> TypedResponse for Result<T, E> {
    fn json_schema(schemas: &mut Schemas) -> Option<RefOr<Schema>> {
        T::json_schema(schemas)
    }
}
impl<H, T: TypedResponse> TypedResponse for (H, T) {
    fn json_schema(schemas: &mut Schemas) -> Option<RefOr<Schema>> {
        T::json_schema(schemas)
    }
}
impl<H1, H2, T: TypedResponse> TypedResponse for (H1, H2, T) {
    fn json_schema(schemas: &mut Schemas) -> Option<RefOr<Schema>> {
        T::json_schema(schemas)
    }
}

macro_rules! native {
    ($($ty:ty),* $(,)?) => { $(impl TypedResponse for $ty {
        fn json_schema(_: &mut Schemas) -> Option<RefOr<Schema>> { None }
    })* };
}
native!(
    axum::response::Response,
    axum::http::StatusCode,
    axum::response::Redirect,
    axum_extra::extract::cookie::CookieJar,
    String,
    &str,
    ()
);
impl<S> TypedResponse for axum::response::Sse<S> {
    fn json_schema(_: &mut Schemas) -> Option<RefOr<Schema>> {
        None
    }
}

pub(crate) trait HandlerResponse<Args> {
    fn json_schema(&self, schemas: &mut Schemas) -> Option<RefOr<Schema>>;
    fn request(&self, schemas: &mut Schemas) -> RequestMetadata;
}

#[derive(Default)]
pub(crate) struct RequestMetadata {
    parameters: Vec<utoipa::openapi::path::Parameter>,
    body: Option<utoipa::openapi::request_body::RequestBody>,
}

pub(crate) trait RequestArgument {
    fn collect(schemas: &mut Schemas, request: &mut RequestMetadata);
}
impl<T: ToSchema> RequestArgument for axum::Json<T> {
    fn collect(schemas: &mut Schemas, request: &mut RequestMetadata) {
        T::schemas(schemas);
        request.body = Some(
            utoipa::openapi::request_body::RequestBodyBuilder::new()
                .required(Some(utoipa::openapi::Required::True))
                .content(
                    "application/json",
                    utoipa::openapi::ContentBuilder::new()
                        .schema(Some(T::schema()))
                        .build(),
                )
                .build(),
        );
    }
}
impl<T: utoipa::IntoParams + ToSchema> RequestArgument for axum::extract::Query<T> {
    fn collect(schemas: &mut Schemas, request: &mut RequestMetadata) {
        T::schemas(schemas);
        request.parameters.extend(T::into_params(|| {
            Some(utoipa::openapi::path::ParameterIn::Query)
        }));
    }
}
impl<T: RequestArgument> RequestArgument for Option<T> {
    fn collect(schemas: &mut Schemas, request: &mut RequestMetadata) {
        let mut optional = RequestMetadata::default();
        T::collect(schemas, &mut optional);
        request.parameters.extend(optional.parameters);
        if let Some(mut body) = optional.body {
            body.required = Some(utoipa::openapi::Required::False);
            request.body = Some(body);
        }
    }
}
impl<T> RequestArgument for axum::extract::State<T> {
    fn collect(_: &mut Schemas, _: &mut RequestMetadata) {}
}
impl<T> RequestArgument for axum::extract::Path<T> {
    fn collect(_: &mut Schemas, _: &mut RequestMetadata) {}
}
macro_rules! native_arguments {
    ($($ty:ty),* $(,)?) => { $(impl RequestArgument for $ty {
        fn collect(_: &mut Schemas, _: &mut RequestMetadata) {}
    })* };
}
native_arguments!(
    axum::http::HeaderMap,
    axum_extra::extract::cookie::CookieJar,
    bytes::Bytes,
    axum::body::Body,
    axum::extract::Multipart,
    axum::extract::Request
);

macro_rules! handler {
    ($($arg:ident),*) => {
        impl<F, Fut, R, $($arg,)*> HandlerResponse<($($arg,)*)> for F
        where F: Fn($($arg),*) -> Fut, Fut: Future<Output = R>, R: TypedResponse,
            $($arg: RequestArgument,)* {
            fn json_schema(&self, schemas: &mut Schemas) -> Option<RefOr<Schema>> { R::json_schema(schemas) }
            fn request(&self, _schemas: &mut Schemas) -> RequestMetadata {
                #[allow(unused_mut)]
                let mut request = RequestMetadata::default();
                $( $arg::collect(_schemas, &mut request); )*
                request
            }
        }
    };
}
handler!();
handler!(A);
handler!(A, B);
handler!(A, B, C);
handler!(A, B, C, D);
handler!(A, B, C, D, E);
handler!(A, B, C, D, E, F1);
handler!(A, B, C, D, E, F1, G);
handler!(A, B, C, D, E, F1, G, H);

pub(crate) fn refine<S, F, Args>(
    router: &mut UtoipaMethodRouter<S>,
    handler: &F,
    metadata: UtoipaMethodRouter<S>,
) where
    F: HandlerResponse<Args>,
{
    let request = handler.request(&mut router.0);
    let schema = handler.json_schema(&mut router.0);
    for (path, item) in metadata.1.paths {
        let target = router
            .1
            .paths
            .get_mut(&path)
            .expect("registered handler path exists");
        for (source, target) in [
            (item.get, &mut target.get),
            (item.post, &mut target.post),
            (item.put, &mut target.put),
            (item.patch, &mut target.patch),
            (item.delete, &mut target.delete),
        ] {
            if source.is_none() {
                continue;
            }
            let operation = target.as_mut().expect("registered handler method exists");
            let mut parameters = operation.parameters.take().unwrap_or_default();
            parameters.retain(|parameter| {
                !matches!(parameter, RefOr::T(parameter)
                if parameter.parameter_in == utoipa::openapi::path::ParameterIn::Query)
            });
            parameters.extend(request.parameters.iter().cloned().map(Into::into));
            operation.parameters = (!parameters.is_empty()).then_some(parameters);
            if let Some(body) = &request.body {
                operation.request_body = Some(body.clone().into());
            }
            let Some(schema) = &schema else {
                continue;
            };
            for (status, response) in &mut operation.responses.responses {
                if status.starts_with('2')
                    && let RefOr::T(response) = response
                {
                    response.content.insert(
                        "application/json".to_owned(),
                        utoipa::openapi::ContentBuilder::new()
                            .schema(Some(schema.clone()))
                            .build()
                            .into(),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json,
        body::{Body, to_bytes},
        http::Request,
    };
    use serde::{Deserialize, Serialize};
    use tower::ServiceExt;

    #[derive(utoipa::ToSchema)]
    #[expect(dead_code, reason = "intentionally stale response annotation")]
    struct Outdated {
        old_field: String,
    }
    #[derive(Serialize, utoipa::ToSchema)]
    struct Actual {
        new_field: String,
    }
    #[derive(Deserialize, utoipa::ToSchema)]
    struct Input {
        text: String,
    }
    #[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
    #[into_params(parameter_in = Query)]
    struct FirstQuery {
        page: Option<u64>,
    }
    #[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
    #[into_params(parameter_in = Query)]
    struct SecondQuery {
        search: Option<String>,
    }

    #[utoipa::path(post, path = "/typed", request_body = Outdated, params(("stale" = String, Query)), responses((status = 200, body = Outdated)))]
    async fn handler(
        axum::extract::Query(first): axum::extract::Query<FirstQuery>,
        axum::extract::Query(second): axum::extract::Query<SecondQuery>,
        Json(input): Json<Input>,
    ) -> Json<Actual> {
        let _ = (first.page, second.search);
        Json(Actual {
            new_field: input.text,
        })
    }

    #[tokio::test]
    async fn handler_types_override_stale_annotations_and_match_real_http_json() {
        let router = OpenApiRouter::new().api_routes(crate::openapi::routes!(handler));
        let spec = serde_json::to_value(router.get_openapi()).unwrap();
        let schema = &spec["paths"]["/typed"]["post"]["responses"]["200"]["content"]["application/json"]
            ["schema"];
        assert!(schema["properties"]["new_field"].is_object());
        assert!(schema["properties"].get("old_field").is_none());
        assert!(
            spec["paths"]["/typed"]["post"]["requestBody"]["content"]["application/json"]["schema"]
                ["properties"]["text"]
                .is_object()
        );
        let names: Vec<_> = spec["paths"]["/typed"]["post"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["page", "search"]);
        let router: axum::Router = router.into();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/typed")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"text":"from the actual DTO"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"new_field":"from the actual DTO"})
        );
    }

    #[utoipa::path(post, path = "/optional", responses((status = 200, body = Actual)))]
    async fn optional(input: Option<Json<Input>>) -> Json<Actual> {
        Json(Actual {
            new_field: input.map(|Json(input)| input.text).unwrap_or_default(),
        })
    }

    #[test]
    fn optional_json_is_an_optional_body_without_claiming_null_is_valid() {
        let router: OpenApiRouter =
            OpenApiRouter::new().api_routes(crate::openapi::routes!(optional));
        let spec = serde_json::to_value(router.get_openapi()).unwrap();
        let request = &spec["paths"]["/optional"]["post"]["requestBody"];
        assert_eq!(request["required"], false);
        assert_eq!(
            request["content"]["application/json"]["schema"]["type"],
            "object"
        );
    }
}
