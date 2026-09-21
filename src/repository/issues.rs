use std::{collections::BTreeSet, path::PathBuf};

use axum::{
    Json,
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{Duration, Utc};
use futures_util::TryStreamExt as _;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ExprTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use tokio::{fs, io::AsyncWriteExt as _};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use super::{Permission, RepositoryState, render_markdown};
use crate::{
    entity::{
        issue_attachment, issue_comment, issue_label, issue_label_assignment, namespace,
        organization_member, repository, repository_collaborator, repository_issue, user,
    },
    filesystem::create_private_directory_async,
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

const MAX_ISSUE_TITLE_LENGTH: usize = 255;
const MAX_ISSUE_BODY_LENGTH: usize = 1_000_000;
const MAX_LABEL_NAME_LENGTH: usize = 64;
const MAX_ISSUE_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;
const MAX_ATTACHMENTS_PER_ISSUE: u64 = 20;
const MAX_USER_ATTACHMENT_BYTES: i64 = 256 * 1024 * 1024;
const MAX_USER_ATTACHMENTS: i64 = 256;
const MAX_USER_UNBOUND_ATTACHMENT_BYTES: i64 = 64 * 1024 * 1024;
const MAX_USER_UNBOUND_ATTACHMENTS: i64 = 16;
const MAX_REPOSITORY_ATTACHMENT_BYTES: i64 = 2 * 1024 * 1024 * 1024;
const MAX_REPOSITORY_ATTACHMENTS: i64 = 2_048;
const MAX_INSTANCE_ATTACHMENT_BYTES: i64 = 20 * 1024 * 1024 * 1024;
const MAX_INSTANCE_ATTACHMENTS: i64 = 20_000;
const UNBOUND_ATTACHMENT_LIFETIME: Duration = Duration::hours(1);
const ATTACHMENT_CLEANUP_BATCH_SIZE: u64 = 100;

#[derive(FromQueryResult)]
struct AttachmentUsage {
    count: i64,
    bytes: Option<i64>,
}

#[derive(Clone, Serialize)]
pub struct IssueUserResponse {
    id: Uuid,
    username: String,
    avatar_updated_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Clone, Serialize)]
pub struct IssueLabelResponse {
    id: Uuid,
    name: String,
    color: String,
    description: String,
    external_url: Option<String>,
}

#[derive(Serialize)]
pub struct ExternalIssueAuthorResponse {
    username: String,
    profile_url: String,
}

#[derive(Serialize)]
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

#[derive(Serialize)]
pub struct IssueAttachmentResponse {
    id: Uuid,
    name: String,
    content_type: String,
    size_bytes: i64,
    created_at: chrono::DateTime<Utc>,
    url: String,
}

#[derive(Serialize)]
pub struct RenderedMarkdownResponse {
    rendered_html: String,
}

#[derive(Serialize)]
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

#[derive(Deserialize)]
pub struct ListIssuesQuery {
    state: Option<String>,
    q: Option<String>,
}

#[derive(Deserialize)]
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

#[derive(Deserialize)]
pub struct UpdateIssueRequest {
    title: Option<String>,
    body: Option<String>,
    state: Option<String>,
    assignee: Option<String>,
    label_ids: Option<Vec<Uuid>>,
    attachment_ids: Option<Vec<Uuid>>,
}

#[derive(Deserialize)]
pub struct CommentRequest {
    body: String,
}

#[derive(Deserialize)]
pub struct UploadAttachmentQuery {
    name: String,
}

#[derive(Deserialize)]
pub struct PreviewMarkdownRequest {
    markdown: String,
}

#[derive(Deserialize)]
pub struct CreateLabelRequest {
    name: String,
    color: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
pub struct UpdateLabelRequest {
    name: Option<String>,
    color: Option<String>,
    description: Option<String>,
}

pub async fn list_issues(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    Query(query): Query<ListIssuesQuery>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<IssueResponse>>, ApiError> {
    let (repository, viewer_id, can_write) =
        issue_context(&state, &headers, &jar, &namespace, &name).await?;
    let mut select = repository_issue::Entity::find()
        .filter(repository_issue::Column::RepositoryId.eq(repository.id));
    if let Some(issue_state) = query.state.as_deref() {
        validate_issue_state(issue_state)?;
        select = select.filter(repository_issue::Column::State.eq(issue_state));
    }
    if let Some(query) = query
        .q
        .map(|query| query.trim().to_owned())
        .filter(|query| !query.is_empty())
    {
        select = select.filter(
            repository_issue::Column::Title
                .contains(&query)
                .or(repository_issue::Column::Body.contains(&query)),
        );
    }
    let issues = select
        .order_by_desc(repository_issue::Column::UpdatedAt)
        .all(state.identity().database())
        .await?;
    Ok(Json(
        issue_responses(&state, issues, viewer_id, can_write).await?,
    ))
}

pub async fn get_issue(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<IssueResponse>, ApiError> {
    let (repository, viewer_id, can_write) =
        issue_context(&state, &headers, &jar, &namespace, &name).await?;
    let issue = find_issue(&state, repository.id, number).await?;
    Ok(Json(
        issue_response(&state, issue, viewer_id, can_write).await?,
    ))
}

pub async fn create_issue(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateIssueRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Read,
            SCOPE_WRITE,
        )
        .await?;
    let can_write = state
        .can_access(&repository, Some(actor.user.id), Permission::Write)
        .await?;
    if (!request.label_ids.is_empty() || request.assignee.is_some()) && !can_write {
        return Err(ApiError::forbidden(
            "Write access is required to assign issues or apply labels.",
        ));
    }
    let title = validate_issue_title(&request.title)?;
    let body = validate_issue_body(request.body)?;
    let assignee_user_id = if let Some(username) = request.assignee.as_deref() {
        assignee_id(&state, &repository, username).await?
    } else {
        None
    };
    validate_label_ids(&state, repository.id, &request.label_ids).await?;
    let transaction = state.identity().database().begin().await?;
    repository::Entity::update_many()
        .col_expr(
            repository::Column::IssueCounter,
            sea_orm::sea_query::Expr::col(repository::Column::IssueCounter).add(1),
        )
        .filter(repository::Column::Id.eq(repository.id))
        .exec(&transaction)
        .await?;
    let number = repository::Entity::find_by_id(repository.id)
        .one(&transaction)
        .await?
        .ok_or_else(ApiError::not_found)?
        .issue_counter;
    let now = Utc::now();
    let issue = repository_issue::ActiveModel {
        id: Set(Uuid::new_v4()),
        repository_id: Set(repository.id),
        number: Set(number),
        author_user_id: Set(actor.user.id),
        title: Set(title),
        body: Set(body),
        state: Set("open".to_owned()),
        assignee_user_id: Set(assignee_user_id),
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
    .insert(&transaction)
    .await?;
    replace_issue_labels_on(&transaction, issue.id, &request.label_ids).await?;
    bind_issue_attachments(
        &state,
        &transaction,
        repository.id,
        issue.id,
        &request.attachment_ids,
        actor.user.id,
        can_write,
    )
    .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.issue.create",
            Some(format!("{namespace}/{name}/{}", issue.number)),
        )
        .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(issue_response(&state, issue, Some(actor.user.id), can_write).await?),
    ))
}

pub async fn update_issue(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateIssueRequest>,
) -> Result<Json<IssueResponse>, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let repository = state.find(&namespace, &name).await?;
    state
        .authorize(&repository, Some(actor.user.id), Permission::Read)
        .await?;
    let can_write = state
        .can_access(&repository, Some(actor.user.id), Permission::Write)
        .await?;
    let stored = find_issue(&state, repository.id, number).await?;
    if stored.external_source.is_some() {
        return Err(ApiError::forbidden(
            "Issues imported from an upstream forge are read-only.",
        ));
    }
    if stored.author_user_id != actor.user.id && !can_write {
        return Err(ApiError::not_found());
    }
    if (request.assignee.is_some() || request.label_ids.is_some()) && !can_write {
        return Err(ApiError::forbidden(
            "Write access is required to assign issues or change labels.",
        ));
    }
    let label_ids = request.label_ids;
    if let Some(ids) = label_ids.as_deref() {
        validate_label_ids(&state, repository.id, ids).await?;
    }
    let mut issue: repository_issue::ActiveModel = stored.into();
    if let Some(title) = request.title {
        issue.title = Set(validate_issue_title(&title)?);
    }
    if let Some(body) = request.body {
        issue.body = Set(validate_issue_body(body)?);
    }
    if let Some(issue_state) = request.state {
        validate_issue_state(&issue_state)?;
        issue.closed_at = Set((issue_state == "closed").then(Utc::now));
        issue.state = Set(issue_state);
    }
    if let Some(username) = request.assignee {
        issue.assignee_user_id = Set(assignee_id(&state, &repository, &username).await?);
    }
    issue.updated_at = Set(Utc::now());
    let transaction = state.identity().database().begin().await?;
    let issue = issue.update(&transaction).await?;
    if let Some(ids) = label_ids.as_deref() {
        replace_issue_labels_on(&transaction, issue.id, ids).await?;
    }
    if let Some(ids) = request.attachment_ids.as_deref() {
        bind_issue_attachments(
            &state,
            &transaction,
            repository.id,
            issue.id,
            ids,
            actor.user.id,
            can_write,
        )
        .await?;
    }
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.issue.update",
            Some(format!("{namespace}/{name}/{number}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(
        issue_response(&state, issue, Some(actor.user.id), can_write).await?,
    ))
}

pub async fn delete_issue(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let issue = find_issue(&state, repository.id, number).await?;
    if issue.external_source.is_some() {
        return Err(ApiError::forbidden(
            "Issues imported from an upstream forge are read-only.",
        ));
    }
    let attachments = issue_attachment::Entity::find()
        .filter(issue_attachment::Column::IssueId.eq(issue.id))
        .all(state.identity().database())
        .await?;
    let transaction = state.identity().database().begin().await?;
    repository_issue::Entity::delete_by_id(issue.id)
        .exec(&transaction)
        .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.issue.delete",
            Some(format!("{namespace}/{name}/{number}")),
        )
        .await?;
    transaction.commit().await?;
    let directory = issue_attachment_directory(&state, &repository);
    for attachment in attachments {
        let _ = fs::remove_file(directory.join(attachment.id.to_string())).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_comments(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<IssueCommentResponse>>, ApiError> {
    let (repository, viewer_id, can_write) =
        issue_context(&state, &headers, &jar, &namespace, &name).await?;
    let issue = find_issue(&state, repository.id, number).await?;
    let comments = issue_comment::Entity::find()
        .filter(issue_comment::Column::IssueId.eq(issue.id))
        .order_by_asc(issue_comment::Column::CreatedAt)
        .all(state.identity().database())
        .await?;
    Ok(Json(
        comment_responses(&state, comments, viewer_id, can_write).await?,
    ))
}

pub async fn create_comment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CommentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Read,
            SCOPE_WRITE,
        )
        .await?;
    let issue = find_issue(&state, repository.id, number).await?;
    if issue.external_source.is_some() {
        return Err(ApiError::forbidden(
            "Comments on imported upstream issues are read-only.",
        ));
    }
    let body = validate_comment_body(request.body)?;
    let now = Utc::now();
    let transaction = state.identity().database().begin().await?;
    let comment = issue_comment::ActiveModel {
        id: Set(Uuid::new_v4()),
        issue_id: Set(issue.id),
        author_user_id: Set(actor.user.id),
        body: Set(body),
        created_at: Set(now),
        updated_at: Set(now),
        external_source: Set(None),
        external_id: Set(None),
        external_url: Set(None),
        external_author: Set(None),
        external_author_url: Set(None),
        external_updated_at: Set(None),
    }
    .insert(&transaction)
    .await?;
    let mut active_issue: repository_issue::ActiveModel = issue.into();
    active_issue.updated_at = Set(now);
    active_issue.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.issue.comment.create",
            Some(format!("{namespace}/{name}/{number}/{}", comment.id)),
        )
        .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(comment_response(&state, comment, Some(actor.user.id), false).await?),
    ))
}

pub async fn update_comment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number, id)): AxumPath<(String, String, i64, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CommentRequest>,
) -> Result<Json<IssueCommentResponse>, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let repository = state.find(&namespace, &name).await?;
    state
        .authorize(&repository, Some(actor.user.id), Permission::Read)
        .await?;
    let can_write = state
        .can_access(&repository, Some(actor.user.id), Permission::Write)
        .await?;
    let issue = find_issue(&state, repository.id, number).await?;
    let stored = find_comment(&state, issue.id, id).await?;
    if stored.external_source.is_some() {
        return Err(ApiError::forbidden(
            "Comments imported from an upstream forge are read-only.",
        ));
    }
    if stored.author_user_id != actor.user.id && !can_write {
        return Err(ApiError::not_found());
    }
    let mut comment: issue_comment::ActiveModel = stored.into();
    comment.body = Set(validate_comment_body(request.body)?);
    comment.updated_at = Set(Utc::now());
    let comment = comment.update(state.identity().database()).await?;
    Ok(Json(
        comment_response(&state, comment, Some(actor.user.id), can_write).await?,
    ))
}

pub async fn delete_comment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, number, id)): AxumPath<(String, String, i64, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let repository = state.find(&namespace, &name).await?;
    state
        .authorize(&repository, Some(actor.user.id), Permission::Read)
        .await?;
    let can_write = state
        .can_access(&repository, Some(actor.user.id), Permission::Write)
        .await?;
    let issue = find_issue(&state, repository.id, number).await?;
    let comment = find_comment(&state, issue.id, id).await?;
    if comment.external_source.is_some() {
        return Err(ApiError::forbidden(
            "Comments imported from an upstream forge are read-only.",
        ));
    }
    if comment.author_user_id != actor.user.id && !can_write {
        return Err(ApiError::not_found());
    }
    issue_comment::Entity::delete_by_id(comment.id)
        .exec(state.identity().database())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_labels(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<IssueLabelResponse>>, ApiError> {
    let (repository, _, _) = issue_context(&state, &headers, &jar, &namespace, &name).await?;
    let labels = issue_label::Entity::find()
        .filter(issue_label::Column::RepositoryId.eq(repository.id))
        .order_by_asc(issue_label::Column::Name)
        .all(state.identity().database())
        .await?;
    Ok(Json(labels.into_iter().map(label_response).collect()))
}

pub async fn create_label(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateLabelRequest>,
) -> Result<impl IntoResponse, ApiError> {
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
    let label = issue_label::ActiveModel {
        id: Set(Uuid::new_v4()),
        repository_id: Set(repository.id),
        name: Set(validate_label_name(&request.name)?),
        color: Set(validate_label_color(&request.color)?),
        description: Set(validate_label_description(request.description)?),
        external_source: Set(None),
        external_instance_url: Set(None),
        external_id: Set(None),
        external_url: Set(None),
        external_updated_at: Set(None),
        created_at: Set(Utc::now()),
    }
    .insert(state.identity().database())
    .await?;
    Ok((StatusCode::CREATED, Json(label_response(label))))
}

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
    let stored = find_label(&state, repository.id, id).await?;
    let mut label: issue_label::ActiveModel = stored.into();
    if let Some(name) = request.name {
        label.name = Set(validate_label_name(&name)?);
    }
    if let Some(color) = request.color {
        label.color = Set(validate_label_color(&color)?);
    }
    if let Some(description) = request.description {
        label.description = Set(validate_label_description(description)?);
    }
    Ok(Json(label_response(
        label.update(state.identity().database()).await?,
    )))
}

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
    let label = find_label(&state, repository.id, id).await?;
    issue_label::Entity::delete_by_id(label.id)
        .exec(state.identity().database())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_assignable_users(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<IssueUserResponse>>, ApiError> {
    let (repository, _, _) = issue_context(&state, &headers, &jar, &namespace, &name).await?;
    let database = state.identity().database();
    let namespace = namespace::Entity::find_by_id(&repository.namespace)
        .one(database)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut user_ids = BTreeSet::new();
    match namespace.kind.as_str() {
        "user" => {
            if let Some(owner_id) = namespace.user_id {
                user_ids.insert(owner_id);
            }
            let collaborators = repository_collaborator::Entity::find()
                .filter(repository_collaborator::Column::RepositoryId.eq(repository.id))
                .filter(repository_collaborator::Column::Role.eq("write"))
                .all(database)
                .await?;
            user_ids.extend(collaborators.into_iter().map(|item| item.user_id));
        }
        "organization" => {
            if let Some(organization_id) = namespace.organization_id {
                let members = organization_member::Entity::find()
                    .filter(organization_member::Column::OrganizationId.eq(organization_id))
                    .all(database)
                    .await?;
                user_ids.extend(members.into_iter().map(|member| member.user_id));
            }
        }
        _ => {}
    }
    let mut users = Vec::with_capacity(user_ids.len());
    for id in user_ids {
        if let Some(account) = user::Entity::find_by_id(id).one(database).await? {
            users.push(IssueUserResponse {
                id: account.id,
                username: account.username,
                avatar_updated_at: account.avatar_updated_at,
            });
        }
    }
    users.sort_unstable_by(|left, right| left.username.cmp(&right.username));
    Ok(Json(users))
}

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

pub async fn upload_attachment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    Query(query): Query<UploadAttachmentQuery>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Body,
) -> Result<impl IntoResponse, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Read,
            SCOPE_WRITE,
        )
        .await?;
    let attachment_name = validate_attachment_name(&query.name)?;
    let attachment_id = Uuid::new_v4();
    let directory = issue_attachment_directory(&state, &repository);
    create_private_directory_async(&directory)
        .await
        .map_err(ApiError::internal)?;
    let path = directory.join(attachment_id.to_string());
    let temporary_path = directory.join(format!(".{attachment_id}.upload"));
    let mut output = fs::File::create(&temporary_path)
        .await
        .map_err(ApiError::internal)?;
    let mut stream = body.into_data_stream();
    let mut size = 0_u64;
    while let Some(chunk) = stream.try_next().await.map_err(ApiError::internal)? {
        size = size.saturating_add(chunk.len() as u64);
        if size > MAX_ISSUE_ATTACHMENT_BYTES {
            drop(output);
            let _ = fs::remove_file(&temporary_path).await;
            return Err(ApiError::bad_request(
                "Issue attachments cannot exceed 10 MiB.",
            ));
        }
        output.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    output.flush().await.map_err(ApiError::internal)?;
    output.sync_all().await.map_err(ApiError::internal)?;
    drop(output);
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .chars()
        .take(255)
        .collect::<String>();
    let transaction = state.identity().database().begin().await?;
    let persistence = async {
        enforce_attachment_quotas(
            &transaction,
            repository.id,
            actor.user.id,
            i64::try_from(size).map_err(ApiError::internal)?,
        )
        .await?;
        fs::rename(&temporary_path, &path)
            .await
            .map_err(ApiError::internal)?;
        let attachment = issue_attachment::ActiveModel {
            id: Set(attachment_id),
            repository_id: Set(repository.id),
            issue_id: Set(None),
            uploader_user_id: Set(actor.user.id),
            name: Set(attachment_name),
            content_type: Set(content_type),
            size_bytes: Set(size as i64),
            created_at: Set(Utc::now()),
        }
        .insert(&transaction)
        .await?;
        state
            .identity()
            .audit_on(
                &transaction,
                Some(actor.user.id),
                "repository.issue.attachment.upload",
                Some(format!("{namespace}/{name}/{}", attachment.id)),
            )
            .await?;
        transaction.commit().await?;
        Ok::<_, ApiError>(attachment)
    }
    .await;
    let attachment = match persistence {
        Ok(attachment) => attachment,
        Err(error) => {
            let _ = fs::remove_file(&temporary_path).await;
            let _ = fs::remove_file(&path).await;
            return Err(error);
        }
    };
    Ok((
        StatusCode::CREATED,
        Json(attachment_response(&repository, attachment)),
    ))
}

async fn enforce_attachment_quotas<C>(
    database: &C,
    repository_id: Uuid,
    uploader_user_id: Uuid,
    incoming_bytes: i64,
) -> Result<(), ApiError>
where
    C: sea_orm::ConnectionTrait,
{
    let unbound = attachment_usage(database, None, Some(uploader_user_id), true).await?;
    ensure_attachment_quota(
        unbound,
        incoming_bytes,
        MAX_USER_UNBOUND_ATTACHMENTS,
        MAX_USER_UNBOUND_ATTACHMENT_BYTES,
        "You have too many unattached issue uploads. Attach or delete an upload before retrying.",
    )?;
    let user = attachment_usage(database, None, Some(uploader_user_id), false).await?;
    ensure_attachment_quota(
        user,
        incoming_bytes,
        MAX_USER_ATTACHMENTS,
        MAX_USER_ATTACHMENT_BYTES,
        "Your issue attachment storage quota is full.",
    )?;
    let repository = attachment_usage(database, Some(repository_id), None, false).await?;
    ensure_attachment_quota(
        repository,
        incoming_bytes,
        MAX_REPOSITORY_ATTACHMENTS,
        MAX_REPOSITORY_ATTACHMENT_BYTES,
        "This repository's issue attachment storage quota is full.",
    )?;
    let instance = attachment_usage(database, None, None, false).await?;
    ensure_attachment_quota(
        instance,
        incoming_bytes,
        MAX_INSTANCE_ATTACHMENTS,
        MAX_INSTANCE_ATTACHMENT_BYTES,
        "The instance issue attachment storage quota is full.",
    )
}

async fn attachment_usage<C>(
    database: &C,
    repository_id: Option<Uuid>,
    uploader_user_id: Option<Uuid>,
    unbound_only: bool,
) -> Result<AttachmentUsage, ApiError>
where
    C: sea_orm::ConnectionTrait,
{
    let mut query = issue_attachment::Entity::find()
        .select_only()
        .column_as(issue_attachment::Column::Id.count(), "count")
        .column_as(issue_attachment::Column::SizeBytes.sum(), "bytes");
    if let Some(repository_id) = repository_id {
        query = query.filter(issue_attachment::Column::RepositoryId.eq(repository_id));
    }
    if let Some(uploader_user_id) = uploader_user_id {
        query = query.filter(issue_attachment::Column::UploaderUserId.eq(uploader_user_id));
    }
    if unbound_only {
        query = query.filter(issue_attachment::Column::IssueId.is_null());
    }
    query
        .into_model::<AttachmentUsage>()
        .one(database)
        .await?
        .ok_or_else(|| ApiError::internal("attachment usage query returned no row"))
}

fn ensure_attachment_quota(
    usage: AttachmentUsage,
    incoming_bytes: i64,
    max_count: i64,
    max_bytes: i64,
    message: &'static str,
) -> Result<(), ApiError> {
    if usage.count.saturating_add(1) > max_count
        || usage
            .bytes
            .unwrap_or_default()
            .saturating_add(incoming_bytes)
            > max_bytes
    {
        return Err(ApiError::too_many_requests(message));
    }
    Ok(())
}

pub async fn download_attachment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, attachment_id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, ApiError> {
    let repository = readable_issue_repository(&state, &headers, &jar, &namespace, &name).await?;
    let attachment = issue_attachment::Entity::find_by_id(attachment_id)
        .filter(issue_attachment::Column::RepositoryId.eq(repository.id))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let file = fs::File::open(
        issue_attachment_directory(&state, &repository).join(attachment.id.to_string()),
    )
    .await
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found()
        } else {
            ApiError::internal(error)
        }
    })?;
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

pub async fn delete_attachment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, attachment_id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Read,
            SCOPE_WRITE,
        )
        .await?;
    let attachment = issue_attachment::Entity::find_by_id(attachment_id)
        .filter(issue_attachment::Column::RepositoryId.eq(repository.id))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let can_write = state
        .can_access(&repository, Some(actor.user.id), Permission::Write)
        .await?;
    if attachment.uploader_user_id != actor.user.id && !can_write {
        return Err(ApiError::not_found());
    }
    let path = issue_attachment_directory(&state, &repository).join(attachment.id.to_string());
    let trash = path.with_extension(format!("delete-{}", Uuid::new_v4().simple()));
    let moved = match fs::rename(&path, &trash).await {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(ApiError::internal(error)),
    };
    let transaction = state.identity().database().begin().await?;
    let persistence = async {
        issue_attachment::Entity::delete_by_id(attachment.id)
            .exec(&transaction)
            .await?;
        state
            .identity()
            .audit_on(
                &transaction,
                Some(actor.user.id),
                "repository.issue.attachment.delete",
                Some(format!("{namespace}/{name}/{}", attachment.id)),
            )
            .await?;
        transaction.commit().await?;
        Ok::<(), ApiError>(())
    }
    .await;
    if let Err(error) = persistence {
        if moved {
            let _ = fs::rename(&trash, &path).await;
        }
        return Err(error);
    }
    if moved {
        let _ = fs::remove_file(trash).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn issue_context(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
) -> Result<(repository::Model, Option<Uuid>, bool), ApiError> {
    let repository = state.find(namespace, name).await?;
    let viewer_id = state
        .identity()
        .optional_user(headers, jar, SCOPE_READ)
        .await?
        .map(|viewer| viewer.id);
    state
        .authorize(&repository, viewer_id, Permission::Read)
        .await?;
    let can_write = state
        .can_access(&repository, viewer_id, Permission::Write)
        .await?;
    Ok((repository, viewer_id, can_write))
}

async fn readable_issue_repository(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
) -> Result<repository::Model, ApiError> {
    let repository = state.find(namespace, name).await?;
    let user_id = state
        .identity()
        .optional_user(headers, jar, SCOPE_READ)
        .await?
        .map(|actor| actor.id);
    state
        .authorize(&repository, user_id, Permission::Read)
        .await?;
    Ok(repository)
}

async fn bind_issue_attachments<C>(
    _state: &RepositoryState,
    transaction: &C,
    repository_id: Uuid,
    issue_id: Uuid,
    ids: &[Uuid],
    actor_id: Uuid,
    can_write: bool,
) -> Result<(), ApiError>
where
    C: sea_orm::ConnectionTrait,
{
    let unique_ids = ids.iter().copied().collect::<BTreeSet<_>>();
    if unique_ids.len() != ids.len() {
        return Err(ApiError::bad_request(
            "Issue attachments cannot contain duplicate IDs.",
        ));
    }
    let mut attached = issue_attachment::Entity::find()
        .filter(issue_attachment::Column::IssueId.eq(issue_id))
        .count(transaction)
        .await?;
    for id in unique_ids {
        let attachment = issue_attachment::Entity::find_by_id(id)
            .filter(issue_attachment::Column::RepositoryId.eq(repository_id))
            .one(transaction)
            .await?
            .ok_or_else(|| {
                ApiError::bad_request("Every attachment must belong to this repository.")
            })?;
        if attachment.issue_id.is_some_and(|bound| bound != issue_id)
            || (attachment.uploader_user_id != actor_id && !can_write)
        {
            return Err(ApiError::bad_request(
                "The attachment cannot be added to this issue.",
            ));
        }
        if attachment.issue_id.is_none() {
            if attached >= MAX_ATTACHMENTS_PER_ISSUE {
                return Err(ApiError::bad_request(format!(
                    "Issues cannot contain more than {MAX_ATTACHMENTS_PER_ISSUE} attachments.",
                )));
            }
            attached = attached.saturating_add(1);
        }
        let mut active: issue_attachment::ActiveModel = attachment.into();
        active.issue_id = Set(Some(issue_id));
        active.update(transaction).await?;
    }
    Ok(())
}

fn issue_attachment_directory(state: &RepositoryState, repository: &repository::Model) -> PathBuf {
    state
        .lfs_repository_path(repository)
        .join("issue-attachments")
}

pub(super) async fn cleanup_expired_attachments(state: &RepositoryState) -> Result<(), ApiError> {
    let cutoff = Utc::now() - UNBOUND_ATTACHMENT_LIFETIME;
    let attachments = issue_attachment::Entity::find()
        .filter(issue_attachment::Column::IssueId.is_null())
        .filter(issue_attachment::Column::CreatedAt.lt(cutoff))
        .order_by_asc(issue_attachment::Column::CreatedAt)
        .limit(ATTACHMENT_CLEANUP_BATCH_SIZE)
        .all(state.identity().database())
        .await?;
    for attachment in attachments {
        let Some(repository) = repository::Entity::find_by_id(attachment.repository_id)
            .one(state.identity().database())
            .await?
        else {
            continue;
        };
        let path = issue_attachment_directory(state, &repository).join(attachment.id.to_string());
        let trash = path.with_extension(format!("expire-{}", Uuid::new_v4().simple()));
        let moved = match fs::rename(&path, &trash).await {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                tracing::warn!(%error, attachment_id = %attachment.id, "could not stage expired issue attachment for deletion");
                continue;
            }
        };
        let deleted = issue_attachment::Entity::delete_many()
            .filter(issue_attachment::Column::Id.eq(attachment.id))
            .filter(issue_attachment::Column::IssueId.is_null())
            .filter(issue_attachment::Column::CreatedAt.lt(cutoff))
            .exec(state.identity().database())
            .await;
        match deleted {
            Ok(result) if result.rows_affected == 1 => {
                if moved {
                    let _ = fs::remove_file(&trash).await;
                }
            }
            Ok(_) => {
                if moved {
                    let _ = fs::rename(&trash, &path).await;
                }
            }
            Err(error) => {
                if moved {
                    let _ = fs::rename(&trash, &path).await;
                }
                return Err(error.into());
            }
        }
    }
    Ok(())
}
fn attachment_response(
    repository: &repository::Model,
    attachment: issue_attachment::Model,
) -> IssueAttachmentResponse {
    IssueAttachmentResponse {
        id: attachment.id,
        url: format!(
            "/api/v1/repositories/{}/{}/issue-attachments/{}",
            repository.namespace, repository.name, attachment.id
        ),
        name: attachment.name,
        content_type: attachment.content_type,
        size_bytes: attachment.size_bytes,
        created_at: attachment.created_at,
    }
}

fn validate_attachment_name(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 255
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(
            "The attachment file name is not valid.",
        ));
    }
    Ok(value.to_owned())
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

async fn find_issue(
    state: &RepositoryState,
    repository_id: Uuid,
    number: i64,
) -> Result<repository_issue::Model, ApiError> {
    repository_issue::Entity::find()
        .filter(repository_issue::Column::RepositoryId.eq(repository_id))
        .filter(repository_issue::Column::Number.eq(number))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn find_comment(
    state: &RepositoryState,
    issue_id: Uuid,
    id: Uuid,
) -> Result<issue_comment::Model, ApiError> {
    issue_comment::Entity::find_by_id(id)
        .filter(issue_comment::Column::IssueId.eq(issue_id))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn find_label(
    state: &RepositoryState,
    repository_id: Uuid,
    id: Uuid,
) -> Result<issue_label::Model, ApiError> {
    issue_label::Entity::find_by_id(id)
        .filter(issue_label::Column::RepositoryId.eq(repository_id))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn issue_responses(
    state: &RepositoryState,
    issues: Vec<repository_issue::Model>,
    viewer_id: Option<Uuid>,
    can_write: bool,
) -> Result<Vec<IssueResponse>, ApiError> {
    let mut responses = Vec::with_capacity(issues.len());
    for issue in issues {
        responses.push(issue_response(state, issue, viewer_id, can_write).await?);
    }
    Ok(responses)
}

async fn issue_response(
    state: &RepositoryState,
    issue: repository_issue::Model,
    viewer_id: Option<Uuid>,
    can_write: bool,
) -> Result<IssueResponse, ApiError> {
    let author = issue_user(state, issue.author_user_id).await?;
    let assignee = if let Some(id) = issue.assignee_user_id {
        Some(issue_user(state, id).await?)
    } else {
        None
    };
    let assignments = issue_label_assignment::Entity::find()
        .filter(issue_label_assignment::Column::IssueId.eq(issue.id))
        .all(state.identity().database())
        .await?;
    let mut labels = Vec::with_capacity(assignments.len());
    for assignment in assignments {
        if let Some(label) = issue_label::Entity::find_by_id(assignment.label_id)
            .one(state.identity().database())
            .await?
        {
            labels.push(label_response(label));
        }
    }
    labels.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    let comment_count = issue_comment::Entity::find()
        .filter(issue_comment::Column::IssueId.eq(issue.id))
        .count(state.identity().database())
        .await?;
    let external_author = issue
        .external_author
        .as_ref()
        .zip(issue.external_author_url.as_ref())
        .map(|(username, profile_url)| ExternalIssueAuthorResponse {
            username: username.clone(),
            profile_url: profile_url.clone(),
        });
    let imported = issue.external_source.is_some();
    Ok(IssueResponse {
        id: issue.id,
        number: issue.number,
        title: issue.title,
        rendered_body: render_markdown(&issue.body),
        body: issue.body,
        state: issue.state,
        author,
        assignee,
        labels,
        comment_count,
        created_at: issue.created_at,
        updated_at: issue.updated_at,
        closed_at: issue.closed_at,
        external_url: issue.external_url,
        external_author,
        can_edit: !imported && (viewer_id == Some(issue.author_user_id) || can_write),
        can_manage: !imported && can_write,
    })
}

async fn comment_responses(
    state: &RepositoryState,
    comments: Vec<issue_comment::Model>,
    viewer_id: Option<Uuid>,
    can_write: bool,
) -> Result<Vec<IssueCommentResponse>, ApiError> {
    let mut responses = Vec::with_capacity(comments.len());
    for comment in comments {
        responses.push(comment_response(state, comment, viewer_id, can_write).await?);
    }
    Ok(responses)
}

async fn comment_response(
    state: &RepositoryState,
    comment: issue_comment::Model,
    viewer_id: Option<Uuid>,
    can_write: bool,
) -> Result<IssueCommentResponse, ApiError> {
    let external_author = comment
        .external_author
        .as_ref()
        .zip(comment.external_author_url.as_ref())
        .map(|(username, profile_url)| ExternalIssueAuthorResponse {
            username: username.clone(),
            profile_url: profile_url.clone(),
        });
    let imported = comment.external_source.is_some();
    Ok(IssueCommentResponse {
        id: comment.id,
        rendered_body: render_markdown(&comment.body),
        body: comment.body,
        author: issue_user(state, comment.author_user_id).await?,
        created_at: comment.created_at,
        updated_at: comment.updated_at,
        external_url: comment.external_url,
        external_author,
        can_edit: !imported && (viewer_id == Some(comment.author_user_id) || can_write),
    })
}

async fn issue_user(state: &RepositoryState, id: Uuid) -> Result<IssueUserResponse, ApiError> {
    user::Entity::find_by_id(id)
        .one(state.identity().database())
        .await?
        .map(|account| IssueUserResponse {
            id: account.id,
            username: account.username,
            avatar_updated_at: account.avatar_updated_at,
        })
        .ok_or_else(ApiError::not_found)
}

async fn assignee_id(
    state: &RepositoryState,
    repository: &repository::Model,
    username: &str,
) -> Result<Option<Uuid>, ApiError> {
    let username = username.trim();
    if username.is_empty() {
        return Ok(None);
    }
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(username))
        .one(state.identity().database())
        .await?
        .ok_or_else(|| ApiError::bad_request("The selected assignee does not exist."))?;
    if !state
        .can_access(repository, Some(account.id), Permission::Write)
        .await?
    {
        return Err(ApiError::bad_request(
            "The assignee must have write access to this repository.",
        ));
    }
    Ok(Some(account.id))
}

async fn validate_label_ids(
    state: &RepositoryState,
    repository_id: Uuid,
    ids: &[Uuid],
) -> Result<(), ApiError> {
    for id in ids {
        find_label(state, repository_id, *id)
            .await
            .map_err(|_| ApiError::bad_request("Every label must belong to this repository."))?;
    }
    Ok(())
}

async fn replace_issue_labels_on<C>(
    database: &C,
    issue_id: Uuid,
    label_ids: &[Uuid],
) -> Result<(), ApiError>
where
    C: sea_orm::ConnectionTrait,
{
    issue_label_assignment::Entity::delete_many()
        .filter(issue_label_assignment::Column::IssueId.eq(issue_id))
        .exec(database)
        .await?;
    for label_id in label_ids {
        issue_label_assignment::ActiveModel {
            issue_id: Set(issue_id),
            label_id: Set(*label_id),
        }
        .insert(database)
        .await?;
    }
    Ok(())
}

fn label_response(label: issue_label::Model) -> IssueLabelResponse {
    IssueLabelResponse {
        id: label.id,
        name: label.name,
        color: label.color,
        description: label.description,
        external_url: label.external_url,
    }
}

// Consumed by the repository import metadata worker.
pub(super) struct ImportedLabel {
    pub(super) external_source: String,
    pub(super) external_instance_url: String,
    pub(super) external_id: String,
    pub(super) external_url: Option<String>,
    pub(super) name: String,
    pub(super) color: String,
    pub(super) description: String,
    pub(super) external_updated_at: Option<chrono::DateTime<Utc>>,
}

// Consumed by the repository import metadata worker.
pub(super) async fn upsert_imported_label(
    state: &super::RepositoryState,
    repository_id: Uuid,
    label: ImportedLabel,
) -> Result<(), crate::identity::ApiError> {
    let database = state.identity().database();
    if let Some(current) = issue_label::Entity::find()
        .filter(issue_label::Column::RepositoryId.eq(repository_id))
        .filter(issue_label::Column::ExternalSource.eq(&label.external_source))
        .filter(issue_label::Column::ExternalInstanceUrl.eq(&label.external_instance_url))
        .filter(issue_label::Column::ExternalId.eq(&label.external_id))
        .one(database)
        .await?
    {
        let mut active: issue_label::ActiveModel = current.into();
        active.name = Set(label.name);
        active.color = Set(label.color);
        active.description = Set(label.description);
        active.external_url = Set(label.external_url);
        active.external_updated_at = Set(label.external_updated_at);
        active.update(database).await?;
    } else {
        issue_label::ActiveModel {
            id: Set(Uuid::new_v4()),
            repository_id: Set(repository_id),
            name: Set(label.name),
            color: Set(label.color),
            description: Set(label.description),
            external_source: Set(Some(label.external_source)),
            external_instance_url: Set(Some(label.external_instance_url)),
            external_id: Set(Some(label.external_id)),
            external_url: Set(label.external_url),
            external_updated_at: Set(label.external_updated_at),
            created_at: Set(Utc::now()),
        }
        .insert(database)
        .await?;
    }
    Ok(())
}

fn validate_issue_title(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_ISSUE_TITLE_LENGTH {
        return Err(ApiError::bad_request(
            "Issue titles must be between 1 and 255 characters.",
        ));
    }
    Ok(value.to_owned())
}

fn validate_issue_body(value: String) -> Result<String, ApiError> {
    if value.len() > MAX_ISSUE_BODY_LENGTH {
        return Err(ApiError::bad_request(
            "Issue descriptions must be at most 1,000,000 characters.",
        ));
    }
    Ok(value)
}

fn validate_comment_body(value: String) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_ISSUE_BODY_LENGTH {
        return Err(ApiError::bad_request(
            "Comments must be between 1 and 1,000,000 characters.",
        ));
    }
    Ok(value.to_owned())
}

fn validate_issue_state(value: &str) -> Result<(), ApiError> {
    if matches!(value, "open" | "closed") {
        Ok(())
    } else {
        Err(ApiError::bad_request("Issue state must be open or closed."))
    }
}

fn validate_label_name(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_LABEL_NAME_LENGTH {
        return Err(ApiError::bad_request(
            "Label names must be between 1 and 64 characters.",
        ));
    }
    Ok(value.to_owned())
}

fn validate_label_color(value: &str) -> Result<String, ApiError> {
    let value = value.trim().trim_start_matches('#').to_ascii_lowercase();
    if value.len() != 6 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request(
            "Label colors must be six hexadecimal digits.",
        ));
    }
    Ok(value)
}

fn validate_label_description(value: String) -> Result<String, ApiError> {
    let value = value.trim();
    if value.len() > 255 {
        return Err(ApiError::bad_request(
            "Label descriptions must be at most 255 characters.",
        ));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use axum::{http::StatusCode, response::IntoResponse as _};

    use super::{AttachmentUsage, ensure_attachment_quota};

    #[test]
    fn attachment_quota_accepts_the_exact_boundary() {
        let result = ensure_attachment_quota(
            AttachmentUsage {
                count: 2,
                bytes: Some(90),
            },
            10,
            3,
            100,
            "quota",
        );

        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn attachment_quota_rejects_an_excess_object() {
        let error = ensure_attachment_quota(
            AttachmentUsage {
                count: 3,
                bytes: Some(90),
            },
            1,
            3,
            100,
            "quota",
        )
        .expect_err("reject attachment count above quota");

        assert_eq!(
            error.into_response().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
}
