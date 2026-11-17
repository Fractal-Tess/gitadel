use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_WRITE};

pub const MAX_AVATAR_REQUEST_BYTES: usize = 6 * 1024 * 1024;

#[utoipa::path(
    get, path = "/users/{user_id}/avatar",
    operation_id = "get_users_user_id_avatar",
    summary = "Download a user's avatar",
    tag = "Account",
    params(
        ("user_id" = Uuid, Path),
    ),
    security(),
    responses(
        (status = 200, description = "Success", body = crate::openapi::Binary, content_type = "image/png"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn public_avatar(
    State(state): State<IdentityState>,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    Ok(avatar_response(
        domain::public_avatar(&state.account_service(), user_id).await?,
    ))
}

#[utoipa::path(
    get, path = "/organizations/{slug}/avatar",
    operation_id = "get_organizations_slug_avatar",
    summary = "Download an organization's avatar",
    tag = "Organizations",
    params(
        ("slug" = String, Path),
    ),
    security(),
    responses(
        (status = 200, description = "Success", body = crate::openapi::Binary, content_type = "image/png"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn public_organization_avatar(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
) -> Result<Response, ApiError> {
    Ok(avatar_response(
        domain::public_organization_avatar(&state.account_service(), slug).await?,
    ))
}

fn avatar_response(content: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(content));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=0, must-revalidate"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateAvatarRequest {
    image_base64: String,
}

#[utoipa::path(
    put, path = "/me/avatar",
    operation_id = "put_me_avatar",
    summary = "Upload an avatar",
    tag = "Account",

    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_avatar(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateAvatarRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::update_avatar(&state.account_service(), &actor, request.image_base64).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put, path = "/organizations/{slug}/avatar",
    operation_id = "put_organizations_slug_avatar",
    summary = "Upload an organization avatar",
    tag = "Organizations",

    params(
        ("slug" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_organization_avatar(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateAvatarRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::update_organization_avatar(
        &state.account_service(),
        &actor,
        slug,
        request.image_base64,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete, path = "/organizations/{slug}/avatar",
    operation_id = "delete_organizations_slug_avatar",
    summary = "Remove an organization avatar",
    tag = "Organizations",
    params(
        ("slug" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_organization_avatar(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::delete_organization_avatar(&state.account_service(), &actor, slug).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete, path = "/me/avatar",
    operation_id = "delete_me_avatar",
    summary = "Remove the avatar",
    tag = "Account",
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_avatar(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::delete_avatar(&state.account_service(), &actor).await?;
    Ok(StatusCode::NO_CONTENT)
}

use gitadel_identity::avatar as domain;
