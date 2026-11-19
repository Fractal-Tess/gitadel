use axum::{
    Json,
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    Permission, RepositoryState, notifications, render_markdown, resources::total_count_header,
    webhook_events,
};
use crate::{
    entity::{issue_comment, repository, repository_issue, user},
    identity::{ApiError, SCOPE_WRITE},
};

#[derive(Clone, Serialize, ToSchema)]
pub struct IssueUserResponse {
    id: Uuid,
    username: String,
    avatar_updated_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Clone, Serialize, ToSchema)]
pub struct IssueLabelResponse {
    id: Uuid,
    name: String,
    color: String,
    description: String,
    external_url: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ExternalIssueAuthorResponse {
    username: String,
    profile_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct IssueResponse {
    id: Uuid,
    number: i64,
    title: String,
    body: String,
    rendered_body: String,
    state: String,
    author: IssueUserResponse,
    assignee: Option<IssueUserResponse>,
    labels: Vec<IssueLabelResponse>,
    comment_count: u64,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    closed_at: Option<chrono::DateTime<Utc>>,
    external_url: Option<String>,
    external_author: Option<ExternalIssueAuthorResponse>,
    can_edit: bool,
    can_manage: bool,
}

#[derive(Serialize, ToSchema)]
pub struct IssueAttachmentResponse {
    id: Uuid,
    name: String,
    content_type: String,
    size_bytes: i64,
    created_at: chrono::DateTime<Utc>,
    url: String,
}

#[derive(Serialize, ToSchema)]
pub struct RenderedMarkdownResponse {
    rendered_html: String,
}

#[derive(Serialize, ToSchema)]
pub struct IssueCommentResponse {
    id: Uuid,
    body: String,
    rendered_body: String,
    author: IssueUserResponse,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    external_url: Option<String>,
    external_author: Option<ExternalIssueAuthorResponse>,
    can_edit: bool,
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListIssuesQuery {
    state: Option<String>,
    q: Option<String>,
    page: Option<u64>,
    per_page: Option<u64>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateIssueRequest {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    label_ids: Vec<Uuid>,
    #[serde(default)]
    attachment_ids: Vec<Uuid>,
    assignee: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateIssueRequest {
    title: Option<String>,
    body: Option<String>,
    state: Option<String>,
    assignee: Option<String>,
    label_ids: Option<Vec<Uuid>>,
    attachment_ids: Option<Vec<Uuid>>,
}

#[derive(Deserialize, ToSchema)]
pub struct CommentRequest {
    body: String,
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UploadAttachmentQuery {
    name: String,
}

#[derive(Deserialize, ToSchema)]
pub struct PreviewMarkdownRequest {
    markdown: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateLabelRequest {
    name: String,
    color: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateLabelRequest {
    name: Option<String>,
    color: Option<String>,
    description: Option<String>,
}

#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/issues",
    operation_id = "get_repositories_namespace_name_issues",
    summary = "List the repository's issues",
    tag = "Issues",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ListIssuesQuery,
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = Vec<crate::repository::issues::IssueResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_issues(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    Query(query): Query<ListIssuesQuery>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let (repository, _viewer_id, _) =
        issue_context(&state, &headers, &jar, &namespace, &name).await?;
    let (total, issues) = service(&state)
        .list_issues(
            repository.id,
            &crate::repository::principal(&headers, &jar)?,
            query.into(),
        )
        .await?;
    Ok((
        total_count_header(total),
        Json(
            issues
                .into_iter()
                .map(Into::into)
                .collect::<Vec<IssueResponse>>(),
        ),
    ))
}
#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/issues/{number}",
    operation_id = "get_repositories_namespace_name_issues_number",
    summary = "Get an issue",
    tag = "Issues",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("number" = i64, Path),
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = IssueResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_issue(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<IssueResponse>, ApiError> {
    let (repository, _viewer_id, _) =
        issue_context(&state, &headers, &jar, &namespace, &name).await?;
    Ok(Json(
        service(&state)
            .get_issue(
                repository.id,
                &crate::repository::principal(&headers, &jar)?,
                number,
            )
            .await?
            .into(),
    ))
}
#[utoipa::path(
    post, path = "/repositories/{namespace}/{name}/issues",
    operation_id = "post_repositories_namespace_name_issues",
    summary = "Create an issue",
    tag = "Issues",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 201, description = "Success", body = crate::repository::issues::IssueResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_issue(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateIssueRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Read,
            SCOPE_WRITE,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(IssueResponse::from(
            service(&state)
                .create_issue(
                    repository.id,
                    &crate::repository::principal(&headers, &jar)?,
                    request.into(),
                )
                .await?,
        )),
    ))
}
#[utoipa::path(
    patch, path = "/repositories/{namespace}/{name}/issues/{number}",
    operation_id = "patch_repositories_namespace_name_issues_number",
    summary = "Update an issue",
    tag = "Issues",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("number" = i64, Path),
    ),
    responses(
        (status = 200, description = "Success", body = IssueResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_issue(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateIssueRequest>,
) -> Result<Json<IssueResponse>, ApiError> {
    let _actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let repository = state.find(&namespace, &name).await?;
    state
        .authorize(&repository, Some(_actor.user.id), Permission::Read)
        .await?;
    Ok(Json(
        service(&state)
            .update_issue(
                repository.id,
                &crate::repository::principal(&headers, &jar)?,
                number,
                request.into(),
            )
            .await?
            .into(),
    ))
}
#[utoipa::path(
    delete, path = "/repositories/{namespace}/{name}/issues/{number}",
    operation_id = "delete_repositories_namespace_name_issues_number",
    summary = "Delete an issue",
    tag = "Issues",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("number" = i64, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_issue(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    service(&state)
        .delete_issue(
            repository.id,
            &crate::repository::principal(&headers, &jar)?,
            number,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/issues/{number}/comments",
    operation_id = "get_repositories_namespace_name_issues_number_comments",
    summary = "List an issue's comments",
    tag = "Issues",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("number" = i64, Path),
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = Vec<IssueCommentResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_comments(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<IssueCommentResponse>>, ApiError> {
    let (repository, _viewer_id, _) =
        issue_context(&state, &headers, &jar, &namespace, &name).await?;
    Ok(Json(
        service(&state)
            .list_comments(
                repository.id,
                &crate::repository::principal(&headers, &jar)?,
                number,
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<IssueCommentResponse>>(),
    ))
}
#[utoipa::path(
    post, path = "/repositories/{namespace}/{name}/issues/{number}/comments",
    operation_id = "post_repositories_namespace_name_issues_number_comments",
    summary = "Comment on an issue",
    tag = "Issues",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("number" = i64, Path),
    ),
    responses(
        (status = 201, description = "Success", body = crate::repository::issues::IssueCommentResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_comment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CommentRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Read,
            SCOPE_WRITE,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(IssueCommentResponse::from(
            service(&state)
                .create_comment(
                    repository.id,
                    &crate::repository::principal(&headers, &jar)?,
                    number,
                    request.into(),
                )
                .await?,
        )),
    ))
}
#[utoipa::path(
    patch, path = "/repositories/{namespace}/{name}/issues/{number}/comments/{id}",
    operation_id = "patch_repositories_namespace_name_issues_number_comments_id",
    summary = "Edit an issue comment",
    tag = "Issues",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("number" = i64, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = IssueCommentResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_comment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number, id)): AxumPath<(String, String, i64, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CommentRequest>,
) -> Result<Json<IssueCommentResponse>, ApiError> {
    let _actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let repository = state.find(&namespace, &name).await?;
    state
        .authorize(&repository, Some(_actor.user.id), Permission::Read)
        .await?;
    Ok(Json(
        service(&state)
            .update_comment(
                repository.id,
                &crate::repository::principal(&headers, &jar)?,
                number,
                id,
                request.into(),
            )
            .await?
            .into(),
    ))
}
#[utoipa::path(
    delete, path = "/repositories/{namespace}/{name}/issues/{number}/comments/{id}",
    operation_id = "delete_repositories_namespace_name_issues_number_comments_id",
    summary = "Delete an issue comment",
    tag = "Issues",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("number" = i64, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_comment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number, id)): AxumPath<(String, String, i64, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let _actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let repository = state.find(&namespace, &name).await?;
    state
        .authorize(&repository, Some(_actor.user.id), Permission::Read)
        .await?;
    service(&state)
        .delete_comment(
            repository.id,
            &crate::repository::principal(&headers, &jar)?,
            number,
            id,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/issue-labels",
    operation_id = "get_repositories_namespace_name_issue_labels",
    summary = "List the repository's issue labels",
    tag = "Issues",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = Vec<IssueLabelResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_labels(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<IssueLabelResponse>>, ApiError> {
    let (repository, _viewer_id, _) =
        issue_context(&state, &headers, &jar, &namespace, &name).await?;
    Ok(Json(
        service(&state)
            .list_labels(
                repository.id,
                &crate::repository::principal(&headers, &jar)?,
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<IssueLabelResponse>>(),
    ))
}
#[utoipa::path(
    post, path = "/repositories/{namespace}/{name}/issue-labels",
    operation_id = "post_repositories_namespace_name_issue_labels",
    summary = "Create an issue label",
    tag = "Issues",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 201, description = "Success", body = crate::repository::issues::IssueLabelResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_label(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateLabelRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Write,
            SCOPE_WRITE,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(IssueLabelResponse::from(
            service(&state)
                .create_label(
                    repository.id,
                    &crate::repository::principal(&headers, &jar)?,
                    request.into(),
                )
                .await?,
        )),
    ))
}
#[utoipa::path(
    patch, path = "/repositories/{namespace}/{name}/issue-labels/{id}",
    operation_id = "patch_repositories_namespace_name_issue_labels_id",
    summary = "Update an issue label",
    tag = "Issues",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = IssueLabelResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_label(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateLabelRequest>,
) -> Result<Json<IssueLabelResponse>, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Write,
            SCOPE_WRITE,
        )
        .await?;
    Ok(Json(
        service(&state)
            .update_label(
                repository.id,
                &crate::repository::principal(&headers, &jar)?,
                id,
                request.into(),
            )
            .await?
            .into(),
    ))
}
#[utoipa::path(
    delete, path = "/repositories/{namespace}/{name}/issue-labels/{id}",
    operation_id = "delete_repositories_namespace_name_issue_labels_id",
    summary = "Delete an issue label",
    tag = "Issues",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_label(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Write,
            SCOPE_WRITE,
        )
        .await?;
    service(&state)
        .delete_label(
            repository.id,
            &crate::repository::principal(&headers, &jar)?,
            id,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/assignable-users",
    operation_id = "get_repositories_namespace_name_assignable_users",
    summary = "List users who can be assigned to issues",
    tag = "Collaborators",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = Vec<IssueUserResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_assignable_users(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<IssueUserResponse>>, ApiError> {
    let (repository, _viewer_id, _) =
        issue_context(&state, &headers, &jar, &namespace, &name).await?;
    Ok(Json(
        service(&state)
            .list_assignable_users(
                repository.id,
                &crate::repository::principal(&headers, &jar)?,
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<IssueUserResponse>>(),
    ))
}
#[utoipa::path(
    post, path = "/repositories/{namespace}/{name}/markdown-preview",
    operation_id = "post_repositories_namespace_name_markdown_preview",
    summary = "Render Markdown as HTML",
    tag = "Repository content",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = RenderedMarkdownResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn preview_markdown(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<PreviewMarkdownRequest>,
) -> Result<Json<RenderedMarkdownResponse>, ApiError> {
    issue_context(&state, &headers, &jar, &namespace, &name).await?;
    Ok(Json(RenderedMarkdownResponse {
        rendered_html: render_markdown(&request.markdown),
    }))
}

#[utoipa::path(
    put, path = "/repositories/{namespace}/{name}/issue-attachments",
    operation_id = "put_repositories_namespace_name_issue_attachments",
    summary = "Upload an issue attachment",
    tag = "Issues",
    request_body(content = crate::openapi::Binary, content_type = "application/octet-stream"),
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        UploadAttachmentQuery,
    ),
    responses(
        (status = 201, description = "Success", body = crate::repository::issues::IssueAttachmentResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn upload_attachment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    Query(query): Query<UploadAttachmentQuery>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Body,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Read,
            SCOPE_WRITE,
        )
        .await?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    Ok((
        StatusCode::CREATED,
        Json(IssueAttachmentResponse::from(
            service(&state)
                .upload_attachment(
                    repository.id,
                    &crate::repository::principal(&headers, &jar)?,
                    query.into(),
                    content_type,
                    body.into_data_stream(),
                )
                .await?,
        )),
    ))
}
#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/issue-attachments/{attachment_id}",
    operation_id = "get_repositories_namespace_name_issue_attachments_attachment_id",
    summary = "Download an issue attachment",
    tag = "Issues",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("attachment_id" = Uuid, Path),
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = crate::openapi::Binary, content_type = "application/octet-stream"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn download_attachment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, attachment_id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, ApiError> {
    let (repository, _) = state
        .readable_repository(&headers, &jar, &namespace, &name)
        .await?;
    let (attachment, file) = service(&state)
        .download_attachment(
            repository.id,
            &crate::repository::principal(&headers, &jar)?,
            attachment_id,
        )
        .await?;
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    let inline = matches!(
        attachment.content_type.as_str(),
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "image/bmp"
            | "image/x-icon"
    );
    let content_type = if inline {
        HeaderValue::from_str(&attachment.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
    } else {
        HeaderValue::from_static("application/octet-stream")
    };
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    let safe_name = ascii_download_name(&attachment.name);
    let disposition = if inline { "inline" } else { "attachment" };
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("{disposition}; filename=\"{safe_name}\""))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&attachment.size_bytes.to_string()).map_err(ApiError::internal)?,
    );
    Ok(response)
}
#[utoipa::path(
    delete, path = "/repositories/{namespace}/{name}/issue-attachments/{attachment_id}",
    operation_id = "delete_repositories_namespace_name_issue_attachments_attachment_id",
    summary = "Delete an issue attachment",
    tag = "Issues",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("attachment_id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_attachment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, attachment_id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Read,
            SCOPE_WRITE,
        )
        .await?;
    service(&state)
        .delete_attachment(
            repository.id,
            &crate::repository::principal(&headers, &jar)?,
            attachment_id,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn issue_context(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
) -> Result<(repository::Model, Option<Uuid>, bool), ApiError> {
    let (repository, viewer_id) = state
        .readable_repository(headers, jar, namespace, name)
        .await?;
    let can_write = state
        .can_access(&repository, viewer_id, Permission::Write)
        .await?;
    Ok((repository, viewer_id, can_write))
}

fn ascii_download_name(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if value.is_empty() {
        "download".to_owned()
    } else {
        value
    }
}

fn service(state: &RepositoryState) -> gitadel_issues::issues::IssueService<'_> {
    gitadel_issues::issues::IssueService::new(
        state.identity().database(),
        state.lfs_root.as_ref(),
        std::sync::Arc::new(LegacyIssueEffects(state.clone())),
        gitadel_core::audit::AuditContext {
            remote_address: crate::identity::remote_address(),
        },
    )
}
struct LegacyIssueEffects(RepositoryState);
#[async_trait::async_trait]
impl gitadel_issues::issue_effects::IssueEffects for LegacyIssueEffects {
    async fn issue(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
        action: gitadel_issues::issue_effects::IssueAction,
        extra: serde_json::Value,
    ) {
        webhook_events::issue(&self.0, repository, actor, issue, action, extra).await;
    }
    async fn comment(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
        comment: &issue_comment::Model,
        action: gitadel_issues::issue_effects::ChangeAction,
        previous_body: Option<&str>,
    ) {
        webhook_events::issue_comment(
            &self.0,
            repository,
            actor,
            issue,
            comment,
            action,
            previous_body,
        )
        .await;
    }
    async fn prepare_deleted(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
    ) -> Option<serde_json::Value> {
        webhook_events::prepare_issue_deleted(&self.0, repository, actor, issue).await
    }
    async fn send_deleted(&self, repository_id: Uuid, payload: Option<serde_json::Value>) {
        webhook_events::send_prepared(&self.0, repository_id, "issues", payload).await;
    }
    fn opened(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
    ) {
        notifications::issue_opened(&self.0, repository, actor, issue);
    }
    fn assigned(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
    ) {
        notifications::issue_assigned(&self.0, repository, actor, issue);
    }
    fn commented(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
        issue: &repository_issue::Model,
        comment: &issue_comment::Model,
    ) {
        notifications::issue_commented(&self.0, repository, actor, issue, comment);
    }
}
pub(super) async fn cleanup_expired_attachments(state: &RepositoryState) -> Result<(), ApiError> {
    Ok(gitadel_issues::issues::cleanup_expired_attachments(&service(state)).await?)
}

impl From<gitadel_issues::issues::IssueUserResponse> for IssueUserResponse {
    fn from(value: gitadel_issues::issues::IssueUserResponse) -> Self {
        Self {
            id: value.id,
            username: value.username,
            avatar_updated_at: value.avatar_updated_at,
        }
    }
}

impl From<gitadel_issues::issues::IssueLabelResponse> for IssueLabelResponse {
    fn from(value: gitadel_issues::issues::IssueLabelResponse) -> Self {
        Self {
            id: value.id,
            name: value.name,
            color: value.color,
            description: value.description,
            external_url: value.external_url,
        }
    }
}

impl From<gitadel_issues::issues::ExternalIssueAuthorResponse> for ExternalIssueAuthorResponse {
    fn from(value: gitadel_issues::issues::ExternalIssueAuthorResponse) -> Self {
        Self {
            username: value.username,
            profile_url: value.profile_url,
        }
    }
}

impl From<gitadel_issues::issues::IssueResponse> for IssueResponse {
    fn from(value: gitadel_issues::issues::IssueResponse) -> Self {
        Self {
            id: value.id,
            number: value.number,
            title: value.title,
            body: value.body,
            rendered_body: value.rendered_body,
            state: value.state,
            author: value.author.into(),
            assignee: value.assignee.map(Into::into),
            labels: value.labels.into_iter().map(Into::into).collect(),
            comment_count: value.comment_count,
            created_at: value.created_at,
            updated_at: value.updated_at,
            closed_at: value.closed_at,
            external_url: value.external_url,
            external_author: value.external_author.map(Into::into),
            can_edit: value.can_edit,
            can_manage: value.can_manage,
        }
    }
}

impl From<gitadel_issues::issues::IssueAttachmentResponse> for IssueAttachmentResponse {
    fn from(value: gitadel_issues::issues::IssueAttachmentResponse) -> Self {
        Self {
            id: value.id,
            name: value.name,
            content_type: value.content_type,
            size_bytes: value.size_bytes,
            created_at: value.created_at,
            url: value.url,
        }
    }
}

impl From<gitadel_issues::issues::RenderedMarkdownResponse> for RenderedMarkdownResponse {
    fn from(value: gitadel_issues::issues::RenderedMarkdownResponse) -> Self {
        Self {
            rendered_html: value.rendered_html,
        }
    }
}

impl From<gitadel_issues::issues::IssueCommentResponse> for IssueCommentResponse {
    fn from(value: gitadel_issues::issues::IssueCommentResponse) -> Self {
        Self {
            id: value.id,
            body: value.body,
            rendered_body: value.rendered_body,
            author: value.author.into(),
            created_at: value.created_at,
            updated_at: value.updated_at,
            external_url: value.external_url,
            external_author: value.external_author.map(Into::into),
            can_edit: value.can_edit,
        }
    }
}

impl From<ListIssuesQuery> for gitadel_issues::issues::ListIssuesQuery {
    fn from(value: ListIssuesQuery) -> Self {
        Self {
            state: value.state,
            q: value.q,
            page: value.page,
            per_page: value.per_page,
        }
    }
}

impl From<CreateIssueRequest> for gitadel_issues::issues::CreateIssueRequest {
    fn from(value: CreateIssueRequest) -> Self {
        Self {
            title: value.title,
            body: value.body,
            label_ids: value.label_ids,
            attachment_ids: value.attachment_ids,
            assignee: value.assignee,
        }
    }
}

impl From<UpdateIssueRequest> for gitadel_issues::issues::UpdateIssueRequest {
    fn from(value: UpdateIssueRequest) -> Self {
        Self {
            title: value.title,
            body: value.body,
            state: value.state,
            assignee: value.assignee,
            label_ids: value.label_ids,
            attachment_ids: value.attachment_ids,
        }
    }
}

impl From<CommentRequest> for gitadel_issues::issues::CommentRequest {
    fn from(value: CommentRequest) -> Self {
        Self { body: value.body }
    }
}

impl From<UploadAttachmentQuery> for gitadel_issues::issues::UploadAttachmentQuery {
    fn from(value: UploadAttachmentQuery) -> Self {
        Self { name: value.name }
    }
}

impl From<PreviewMarkdownRequest> for gitadel_issues::issues::PreviewMarkdownRequest {
    fn from(value: PreviewMarkdownRequest) -> Self {
        Self {
            markdown: value.markdown,
        }
    }
}

impl From<CreateLabelRequest> for gitadel_issues::issues::CreateLabelRequest {
    fn from(value: CreateLabelRequest) -> Self {
        Self {
            name: value.name,
            color: value.color,
            description: value.description,
        }
    }
}

impl From<UpdateLabelRequest> for gitadel_issues::issues::UpdateLabelRequest {
    fn from(value: UpdateLabelRequest) -> Self {
        Self {
            name: value.name,
            color: value.color,
            description: value.description,
        }
    }
}

pub(super) use gitadel_issues::issues::ImportedLabel;

// Consumed by the repository import metadata worker.
pub(super) async fn upsert_imported_label(
    state: &RepositoryState,
    repository_id: Uuid,
    label: ImportedLabel,
) -> Result<(), ApiError> {
    Ok(
        gitadel_issues::issues::upsert_imported_label(&service(state), repository_id, label)
            .await?,
    )
}

impl From<gitadel_issues::issues::IssueError> for ApiError {
    fn from(error: gitadel_issues::issues::IssueError) -> Self {
        match error {
            gitadel_issues::issues::IssueError::Identity(error) => error.into(),
            gitadel_issues::issues::IssueError::Quota(message) => {
                ApiError::too_many_requests(message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        extract::{Path as AxumPath, Query, State},
        http::HeaderMap,
        response::IntoResponse as _,
    };
    use axum_extra::extract::cookie::CookieJar;
    use chrono::{Duration, Utc};
    use sea_orm::{ActiveModelTrait, Set};
    use serde_json::Value;
    use uuid::Uuid;

    use super::{ListIssuesQuery, list_issues};
    use crate::{
        entity::{issue_comment, issue_label, issue_label_assignment, repository_issue},
        repository::resources::test_support::{TestInstance, instance},
    };

    async fn insert_issue(
        instance: &TestInstance,
        repository_id: Uuid,
        number: i64,
        title: &str,
        state: &str,
    ) -> repository_issue::Model {
        // Higher numbers are more recently updated, so lists lead with them.
        let now = Utc::now() + Duration::seconds(number);
        repository_issue::ActiveModel {
            id: Set(Uuid::new_v4()),
            repository_id: Set(repository_id),
            number: Set(number),
            author_user_id: Set(instance.owner.id),
            title: Set(title.to_owned()),
            body: Set(format!("Body of **{title}**")),
            state: Set(state.to_owned()),
            assignee_user_id: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            closed_at: Set(None),
            external_source: Set(None),
            external_id: Set(None),
            external_url: Set(None),
            external_author: Set(None),
            external_author_url: Set(None),
            external_updated_at: Set(None),
        }
        .insert(&instance.database)
        .await
        .unwrap()
    }

    /// The `X-Total-Count` header and the JSON array a list request returns.
    async fn list(
        instance: &TestInstance,
        state: Option<&str>,
        q: Option<&str>,
        page: Option<u64>,
        per_page: Option<u64>,
    ) -> (String, Vec<Value>) {
        let response = list_issues(
            State(instance.state.clone()),
            AxumPath(("alice".to_owned(), "project".to_owned())),
            Query(ListIssuesQuery {
                state: state.map(str::to_owned),
                q: q.map(str::to_owned),
                page,
                per_page,
            }),
            HeaderMap::new(),
            CookieJar::new(),
        )
        .await
        .unwrap()
        .into_response();
        let total = response.headers()["x-total-count"]
            .to_str()
            .unwrap()
            .to_owned();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (total, serde_json::from_slice(&body).unwrap())
    }

    fn numbers(issues: &[Value]) -> Vec<i64> {
        issues
            .iter()
            .map(|issue| issue["number"].as_i64().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn issue_list_pages_and_reports_the_total() {
        let instance = instance().await;
        let repository = instance.repository("project").await;
        for number in 1..=5 {
            let state = if number == 2 { "closed" } else { "open" };
            let title = if number % 2 == 1 { "odd crash" } else { "even" };
            insert_issue(&instance, repository.id, number, title, state).await;
        }

        // The body stays a bare array, and defaults return everything here.
        let (total, issues) = list(&instance, None, None, None, None).await;
        assert_eq!(total, "5");
        assert_eq!(numbers(&issues), [5, 4, 3, 2, 1]);

        let (total, issues) = list(&instance, None, None, Some(1), Some(2)).await;
        assert_eq!(total, "5");
        assert_eq!(numbers(&issues), [5, 4]);
        let (_, issues) = list(&instance, None, None, Some(3), Some(2)).await;
        assert_eq!(numbers(&issues), [1]);
        let (total, issues) = list(&instance, None, None, Some(4), Some(2)).await;
        assert_eq!(total, "5");
        assert!(issues.is_empty());

        // Filters narrow the total as well as the page.
        let (total, issues) = list(&instance, Some("open"), None, Some(2), Some(3)).await;
        assert_eq!(total, "4");
        assert_eq!(numbers(&issues), [1]);
        let (total, issues) = list(&instance, None, Some("crash"), Some(1), Some(2)).await;
        assert_eq!(total, "3");
        assert_eq!(numbers(&issues), [5, 3]);
        let (total, issues) = list(&instance, Some("closed"), Some("crash"), None, None).await;
        assert_eq!(total, "0");
        assert!(issues.is_empty());
    }

    #[tokio::test]
    async fn issue_list_batches_users_labels_and_comment_counts() {
        let instance = instance().await;
        let repository = instance.repository("project").await;
        let plain = insert_issue(&instance, repository.id, 1, "plain", "open").await;
        let busy = insert_issue(&instance, repository.id, 2, "busy", "open").await;
        let mut assigned: repository_issue::ActiveModel = busy.clone().into();
        assigned.assignee_user_id = Set(Some(instance.owner.id));
        assigned.update(&instance.database).await.unwrap();
        for name in ["zeta", "alpha"] {
            let label = issue_label::ActiveModel {
                id: Set(Uuid::new_v4()),
                repository_id: Set(repository.id),
                name: Set(name.to_owned()),
                color: Set("ff0000".to_owned()),
                description: Set(String::new()),
                external_source: Set(None),
                external_instance_url: Set(None),
                external_id: Set(None),
                external_url: Set(None),
                external_updated_at: Set(None),
                created_at: Set(Utc::now()),
            }
            .insert(&instance.database)
            .await
            .unwrap();
            issue_label_assignment::ActiveModel {
                issue_id: Set(busy.id),
                label_id: Set(label.id),
            }
            .insert(&instance.database)
            .await
            .unwrap();
        }
        for (issue, comments) in [(&busy, 3), (&plain, 0)] {
            for _ in 0..comments {
                issue_comment::ActiveModel {
                    id: Set(Uuid::new_v4()),
                    issue_id: Set(issue.id),
                    author_user_id: Set(instance.owner.id),
                    body: Set("comment".to_owned()),
                    created_at: Set(Utc::now()),
                    updated_at: Set(Utc::now()),
                    external_source: Set(None),
                    external_id: Set(None),
                    external_url: Set(None),
                    external_author: Set(None),
                    external_author_url: Set(None),
                    external_updated_at: Set(None),
                }
                .insert(&instance.database)
                .await
                .unwrap();
            }
        }

        let (_, issues) = list(&instance, None, None, None, None).await;
        let [busy, plain] = issues.as_slice() else {
            panic!("expected two issues, got {issues:?}");
        };
        assert_eq!(busy["number"], 2);
        assert_eq!(busy["author"]["username"], "alice");
        assert_eq!(busy["assignee"]["username"], "alice");
        assert_eq!(busy["comment_count"], 3);
        let labels = busy["labels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|label| label["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["alpha", "zeta"]);
        assert_eq!(
            busy["rendered_body"],
            "<p>Body of <strong>busy</strong></p>\n"
        );

        assert_eq!(plain["number"], 1);
        assert_eq!(plain["assignee"], Value::Null);
        assert_eq!(plain["comment_count"], 0);
        assert_eq!(plain["labels"], Value::Array(Vec::new()));
    }
}
