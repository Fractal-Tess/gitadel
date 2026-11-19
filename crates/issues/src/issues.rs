//! Issue, comment, label and attachment operations.
use crate::issue_effects::{self as webhook_events, ChangeAction, IssueAction, IssueEffects};
use chrono::{Duration, Utc};
use futures_util::TryStreamExt as _;
use gitadel_core::{
    audit::AuditContext, filesystem::create_private_directory_async, markdown::render_markdown,
};
use gitadel_db::entity::{
    issue_attachment, issue_comment, issue_label, issue_label_assignment, namespace,
    organization_member, repository, repository_collaborator, repository_issue, user,
};
use gitadel_identity::permissions::RepositoryPrincipal;
use gitadel_identity::permissions::{
    RepositoryPermission as Permission, authorize_repository, can_access_repository,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ExprTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::Arc,
};
use tokio::{fs, io::AsyncWriteExt as _};
use uuid::Uuid;
const IN_QUERY_CHUNK: usize = 500;
struct PageWindow {
    offset: u64,
    limit: u64,
}
impl PageWindow {
    fn new(page: Option<u64>, per_page: Option<u64>) -> Self {
        let limit = per_page.unwrap_or(100).clamp(1, 100);
        Self {
            offset: std::cmp::Ord::max(page.unwrap_or(1), 1)
                .saturating_sub(1)
                .saturating_mul(limit),
            limit,
        }
    }
}
pub struct IssueService<'a> {
    pub(crate) database: &'a sea_orm::DatabaseConnection,
    lfs_root: &'a std::path::Path,
    pub(crate) effects: Arc<dyn IssueEffects>,
    audit: AuditContext,
}
impl<'a> IssueService<'a> {
    pub fn new(
        database: &'a sea_orm::DatabaseConnection,
        lfs_root: &'a std::path::Path,
        effects: Arc<dyn IssueEffects>,
        audit: AuditContext,
    ) -> Self {
        Self {
            database,
            lfs_root,
            effects,
            audit,
        }
    }
    fn lfs_repository_path(&self, repository: &repository::Model) -> PathBuf {
        self.lfs_root.join(repository.storage_key.to_string())
    }
    async fn can_access(
        &self,
        repository: &repository::Model,
        actor: Option<Uuid>,
        permission: Permission,
    ) -> Result<bool, IssueError> {
        Ok(can_access_repository(self.database, repository.id, actor, permission).await?)
    }
    pub async fn list_issues(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        query: ListIssuesQuery,
    ) -> Result<(u64, Vec<IssueResponse>), IssueError> {
        let viewer_id = principal
            .optional_user(self.database, gitadel_identity::SCOPE_READ)
            .await?
            .map(|user| user.id);
        let state = self;
        let repository =
            authorize_repository(state.database, repository_id, viewer_id, Permission::Read)
                .await?;
        let can_write = state
            .can_access(&repository, viewer_id, Permission::Write)
            .await?;
        let window = PageWindow::new(query.page, query.per_page);
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
        let total = select.clone().count(state.database).await?;
        let issues = select
            .order_by_desc(repository_issue::Column::UpdatedAt)
            // Issues touched in the same instant keep one order across pages.
            .order_by_desc(repository_issue::Column::Number)
            .offset(window.offset)
            .limit(window.limit)
            .all(state.database)
            .await?;
        Ok((
            total,
            issue_responses(state, issues, viewer_id, can_write).await?,
        ))
    }
    pub async fn get_issue(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        number: i64,
    ) -> Result<IssueResponse, IssueError> {
        let viewer_id = principal
            .optional_user(self.database, gitadel_identity::SCOPE_READ)
            .await?
            .map(|user| user.id);
        let state = self;
        let repository =
            authorize_repository(state.database, repository_id, viewer_id, Permission::Read)
                .await?;
        let can_write = state
            .can_access(&repository, viewer_id, Permission::Write)
            .await?;
        let issue = find_issue(state, repository.id, number).await?;
        issue_response(state, issue, viewer_id, can_write).await
    }
    pub async fn create_issue(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        request: CreateIssueRequest,
    ) -> Result<IssueResponse, IssueError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Read,
        )
        .await?;
        let namespace = &repository.namespace;
        let name = &repository.name;
        let can_write = state
            .can_access(&repository, Some(actor.user.id), Permission::Write)
            .await?;
        if (!request.label_ids.is_empty() || request.assignee.is_some()) && !can_write {
            return Err(IssueError::forbidden(
                "Write access is required to assign issues or apply labels.",
            ));
        }
        let title = validate_issue_title(&request.title)?;
        let body = validate_issue_body(request.body)?;
        let assignee_user_id = if let Some(username) = request.assignee.as_deref() {
            assignee_id(state, &repository, username).await?
        } else {
            None
        };
        validate_label_ids(state, repository.id, &request.label_ids).await?;
        let transaction = state.database.begin().await?;
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
            .ok_or_else(IssueError::not_found)?
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
            state,
            &transaction,
            repository.id,
            issue.id,
            &request.attachment_ids,
            actor.user.id,
            can_write,
        )
        .await?;
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            "repository.issue.create",
            Some(format!("{namespace}/{name}/{}", issue.number)),
            state.audit,
        )
        .await?;
        transaction.commit().await?;
        webhook_events::issue(
            state,
            &repository,
            &actor.user,
            &issue,
            IssueAction::Opened,
            serde_json::json!({}),
        )
        .await;
        state.effects.opened(&repository, &actor.user, &issue);
        issue_response(state, issue, Some(actor.user.id), can_write).await
    }
    pub async fn update_issue(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        number: i64,
        request: UpdateIssueRequest,
    ) -> Result<IssueResponse, IssueError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Read,
        )
        .await?;
        let namespace = &repository.namespace;
        let name = &repository.name;
        let can_write = state
            .can_access(&repository, Some(actor.user.id), Permission::Write)
            .await?;
        let stored = find_issue(state, repository.id, number).await?;
        if stored.external_source.is_some() {
            return Err(IssueError::forbidden(
                "Issues imported from an upstream forge are read-only.",
            ));
        }
        if stored.author_user_id != actor.user.id && !can_write {
            return Err(IssueError::not_found());
        }
        if (request.assignee.is_some() || request.label_ids.is_some()) && !can_write {
            return Err(IssueError::forbidden(
                "Write access is required to assign issues or change labels.",
            ));
        }
        let label_ids = request.label_ids;
        let previous_labels = if let Some(ids) = label_ids.as_deref() {
            validate_label_ids(state, repository.id, ids).await?;
            Some(assigned_label_ids(state, stored.id).await?)
        } else {
            None
        };
        let previous = stored.clone();
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
            issue.assignee_user_id = Set(assignee_id(state, &repository, &username).await?);
        }
        issue.updated_at = Set(Utc::now());
        let transaction = state.database.begin().await?;
        let issue = issue.update(&transaction).await?;
        if let Some(ids) = label_ids.as_deref() {
            replace_issue_labels_on(&transaction, issue.id, ids).await?;
        }
        if let Some(ids) = request.attachment_ids.as_deref() {
            bind_issue_attachments(
                state,
                &transaction,
                repository.id,
                issue.id,
                ids,
                actor.user.id,
                can_write,
            )
            .await?;
        }
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            "repository.issue.update",
            Some(format!("{namespace}/{name}/{number}")),
            state.audit,
        )
        .await?;
        transaction.commit().await?;
        emit_issue_update_events(
            state,
            &repository,
            &actor.user,
            &previous,
            &issue,
            previous_labels.as_deref().zip(label_ids.as_deref()),
        )
        .await;
        issue_response(state, issue, Some(actor.user.id), can_write).await
    }
    pub async fn delete_issue(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        number: i64,
    ) -> Result<(), IssueError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Manage,
        )
        .await?;
        let namespace = &repository.namespace;
        let name = &repository.name;
        let issue = find_issue(state, repository.id, number).await?;
        if issue.external_source.is_some() {
            return Err(IssueError::forbidden(
                "Issues imported from an upstream forge are read-only.",
            ));
        }
        let attachments = issue_attachment::Entity::find()
            .filter(issue_attachment::Column::IssueId.eq(issue.id))
            .all(state.database)
            .await?;
        let deleted_event =
            webhook_events::prepare_issue_deleted(state, &repository, &actor.user, &issue).await;
        let transaction = state.database.begin().await?;
        repository_issue::Entity::delete_by_id(issue.id)
            .exec(&transaction)
            .await?;
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            "repository.issue.delete",
            Some(format!("{namespace}/{name}/{number}")),
            state.audit,
        )
        .await?;
        transaction.commit().await?;
        webhook_events::send_prepared(state, repository.id, "issues", deleted_event).await;
        let directory = issue_attachment_directory(state, &repository);
        for attachment in attachments {
            let _ = fs::remove_file(directory.join(attachment.id.to_string())).await;
        }
        Ok(())
    }
    pub async fn list_comments(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        number: i64,
    ) -> Result<Vec<IssueCommentResponse>, IssueError> {
        let viewer_id = principal
            .optional_user(self.database, gitadel_identity::SCOPE_READ)
            .await?
            .map(|user| user.id);
        let state = self;
        let repository =
            authorize_repository(state.database, repository_id, viewer_id, Permission::Read)
                .await?;
        let can_write = state
            .can_access(&repository, viewer_id, Permission::Write)
            .await?;
        let issue = find_issue(state, repository.id, number).await?;
        let comments = issue_comment::Entity::find()
            .filter(issue_comment::Column::IssueId.eq(issue.id))
            .order_by_asc(issue_comment::Column::CreatedAt)
            .all(state.database)
            .await?;
        comment_responses(state, comments, viewer_id, can_write).await
    }
    pub async fn create_comment(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        number: i64,
        request: CommentRequest,
    ) -> Result<IssueCommentResponse, IssueError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Read,
        )
        .await?;
        let namespace = &repository.namespace;
        let name = &repository.name;
        let issue = find_issue(state, repository.id, number).await?;
        if issue.external_source.is_some() {
            return Err(IssueError::forbidden(
                "Comments on imported upstream issues are read-only.",
            ));
        }
        let body = validate_comment_body(request.body)?;
        let now = Utc::now();
        let transaction = state.database.begin().await?;
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
        let issue = active_issue.update(&transaction).await?;
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            "repository.issue.comment.create",
            Some(format!("{namespace}/{name}/{number}/{}", comment.id)),
            state.audit,
        )
        .await?;
        transaction.commit().await?;
        webhook_events::issue_comment(
            state,
            &repository,
            &actor.user,
            &issue,
            &comment,
            ChangeAction::Created,
            None,
        )
        .await;
        state
            .effects
            .commented(&repository, &actor.user, &issue, &comment);
        comment_response(state, comment, Some(actor.user.id), false).await
    }
    pub async fn update_comment(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        number: i64,
        id: Uuid,
        request: CommentRequest,
    ) -> Result<IssueCommentResponse, IssueError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Read,
        )
        .await?;
        let can_write = state
            .can_access(&repository, Some(actor.user.id), Permission::Write)
            .await?;
        let issue = find_issue(state, repository.id, number).await?;
        let stored = find_comment(state, issue.id, id).await?;
        if stored.external_source.is_some() {
            return Err(IssueError::forbidden(
                "Comments imported from an upstream forge are read-only.",
            ));
        }
        if stored.author_user_id != actor.user.id && !can_write {
            return Err(IssueError::not_found());
        }
        let previous_body = stored.body.clone();
        let mut comment: issue_comment::ActiveModel = stored.into();
        comment.body = Set(validate_comment_body(request.body)?);
        comment.updated_at = Set(Utc::now());
        let comment = comment.update(state.database).await?;
        if comment.body != previous_body {
            webhook_events::issue_comment(
                state,
                &repository,
                &actor.user,
                &issue,
                &comment,
                ChangeAction::Edited,
                Some(&previous_body),
            )
            .await;
        }
        comment_response(state, comment, Some(actor.user.id), can_write).await
    }
    pub async fn delete_comment(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        number: i64,
        id: Uuid,
    ) -> Result<(), IssueError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Read,
        )
        .await?;
        let can_write = state
            .can_access(&repository, Some(actor.user.id), Permission::Write)
            .await?;
        let issue = find_issue(state, repository.id, number).await?;
        let comment = find_comment(state, issue.id, id).await?;
        if comment.external_source.is_some() {
            return Err(IssueError::forbidden(
                "Comments imported from an upstream forge are read-only.",
            ));
        }
        if comment.author_user_id != actor.user.id && !can_write {
            return Err(IssueError::not_found());
        }
        issue_comment::Entity::delete_by_id(comment.id)
            .exec(state.database)
            .await?;
        webhook_events::issue_comment(
            state,
            &repository,
            &actor.user,
            &issue,
            &comment,
            ChangeAction::Deleted,
            None,
        )
        .await;
        Ok(())
    }
    pub async fn list_labels(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
    ) -> Result<Vec<IssueLabelResponse>, IssueError> {
        let viewer_id = principal
            .optional_user(self.database, gitadel_identity::SCOPE_READ)
            .await?
            .map(|user| user.id);
        let state = self;
        let repository =
            authorize_repository(state.database, repository_id, viewer_id, Permission::Read)
                .await?;
        let labels = issue_label::Entity::find()
            .filter(issue_label::Column::RepositoryId.eq(repository.id))
            .order_by_asc(issue_label::Column::Name)
            .all(state.database)
            .await?;
        Ok(labels.into_iter().map(label_response).collect())
    }
    pub async fn create_label(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        request: CreateLabelRequest,
    ) -> Result<IssueLabelResponse, IssueError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Write,
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
        .insert(state.database)
        .await?;
        Ok(label_response(label))
    }
    pub async fn update_label(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        id: Uuid,
        request: UpdateLabelRequest,
    ) -> Result<IssueLabelResponse, IssueError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Write,
        )
        .await?;
        let stored = find_label(state, repository.id, id).await?;
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
        Ok(label_response(label.update(state.database).await?))
    }
    pub async fn delete_label(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        id: Uuid,
    ) -> Result<(), IssueError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Write,
        )
        .await?;
        let label = find_label(state, repository.id, id).await?;
        issue_label::Entity::delete_by_id(label.id)
            .exec(state.database)
            .await?;
        Ok(())
    }
    pub async fn list_assignable_users(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
    ) -> Result<Vec<IssueUserResponse>, IssueError> {
        let viewer_id = principal
            .optional_user(self.database, gitadel_identity::SCOPE_READ)
            .await?
            .map(|user| user.id);
        let state = self;
        let repository =
            authorize_repository(state.database, repository_id, viewer_id, Permission::Read)
                .await?;
        let database = state.database;
        let namespace = namespace::Entity::find_by_id(&repository.namespace)
            .one(database)
            .await?
            .ok_or_else(IssueError::not_found)?;
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
        Ok(users)
    }
    pub async fn upload_attachment<S, E>(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        query: UploadAttachmentQuery,
        content_type: String,
        mut stream: S,
    ) -> Result<IssueAttachmentResponse, IssueError>
    where
        S: futures_util::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Read,
        )
        .await?;
        let namespace = &repository.namespace;
        let name = &repository.name;
        let attachment_name = validate_attachment_name(&query.name)?;
        let content_type = content_type.chars().take(255).collect::<String>();
        let attachment_id = Uuid::new_v4();
        let directory = issue_attachment_directory(state, &repository);
        create_private_directory_async(&directory)
            .await
            .map_err(IssueError::internal)?;
        let path = directory.join(attachment_id.to_string());
        let temporary_path = directory.join(format!(".{attachment_id}.upload"));
        let mut output = fs::File::create(&temporary_path)
            .await
            .map_err(IssueError::internal)?;
        let mut size = 0_u64;
        while let Some(chunk) = stream.try_next().await.map_err(IssueError::internal)? {
            size = size.saturating_add(chunk.len() as u64);
            if size > MAX_ISSUE_ATTACHMENT_BYTES {
                drop(output);
                let _ = fs::remove_file(&temporary_path).await;
                return Err(IssueError::bad_request(
                    "Issue attachments cannot exceed 10 MiB.",
                ));
            }
            output
                .write_all(&chunk)
                .await
                .map_err(IssueError::internal)?;
        }
        output.flush().await.map_err(IssueError::internal)?;
        output.sync_all().await.map_err(IssueError::internal)?;
        drop(output);
        let transaction = state.database.begin().await?;
        let persistence = async {
            enforce_attachment_quotas(
                &transaction,
                repository.id,
                actor.user.id,
                i64::try_from(size).map_err(IssueError::internal)?,
            )
            .await?;
            fs::rename(&temporary_path, &path)
                .await
                .map_err(IssueError::internal)?;
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
            gitadel_db::audit::record(
                &transaction,
                Some(actor.user.id),
                "repository.issue.attachment.upload",
                Some(format!("{namespace}/{name}/{}", attachment.id)),
                state.audit,
            )
            .await?;
            transaction.commit().await?;
            Ok::<_, IssueError>(attachment)
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
        Ok(attachment_response(&repository, attachment))
    }
    pub async fn download_attachment(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        attachment_id: Uuid,
    ) -> Result<(issue_attachment::Model, fs::File), IssueError> {
        let viewer_id = principal
            .optional_user(self.database, gitadel_identity::SCOPE_READ)
            .await?
            .map(|user| user.id);
        let state = self;
        let repository =
            authorize_repository(state.database, repository_id, viewer_id, Permission::Read)
                .await?;
        let attachment = issue_attachment::Entity::find_by_id(attachment_id)
            .filter(issue_attachment::Column::RepositoryId.eq(repository.id))
            .one(state.database)
            .await?
            .ok_or_else(IssueError::not_found)?;
        let file = fs::File::open(
            issue_attachment_directory(state, &repository).join(attachment.id.to_string()),
        )
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                IssueError::not_found()
            } else {
                IssueError::internal(error)
            }
        })?;
        Ok((attachment, file))
    }
    pub async fn delete_attachment(
        &self,
        repository_id: Uuid,
        principal: &RepositoryPrincipal,
        attachment_id: Uuid,
    ) -> Result<(), IssueError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let state = self;
        let repository = authorize_repository(
            state.database,
            repository_id,
            Some(actor.user.id),
            Permission::Read,
        )
        .await?;
        let namespace = &repository.namespace;
        let name = &repository.name;
        let attachment = issue_attachment::Entity::find_by_id(attachment_id)
            .filter(issue_attachment::Column::RepositoryId.eq(repository.id))
            .one(state.database)
            .await?
            .ok_or_else(IssueError::not_found)?;
        let can_write = state
            .can_access(&repository, Some(actor.user.id), Permission::Write)
            .await?;
        if attachment.uploader_user_id != actor.user.id && !can_write {
            return Err(IssueError::not_found());
        }
        let path = issue_attachment_directory(state, &repository).join(attachment.id.to_string());
        let trash = path.with_extension(format!("delete-{}", Uuid::new_v4().simple()));
        let moved = match fs::rename(&path, &trash).await {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(IssueError::internal(error)),
        };
        let transaction = state.database.begin().await?;
        let persistence = async {
            issue_attachment::Entity::delete_by_id(attachment.id)
                .exec(&transaction)
                .await?;
            gitadel_db::audit::record(
                &transaction,
                Some(actor.user.id),
                "repository.issue.attachment.delete",
                Some(format!("{namespace}/{name}/{}", attachment.id)),
                state.audit,
            )
            .await?;
            transaction.commit().await?;
            Ok::<(), IssueError>(())
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
        Ok(())
    }
}
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

#[derive(Clone)]
pub struct IssueUserResponse {
    pub id: Uuid,
    pub username: String,
    pub avatar_updated_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Clone)]
pub struct IssueLabelResponse {
    pub id: Uuid,
    pub name: String,
    pub color: String,
    pub description: String,
    pub external_url: Option<String>,
}

#[derive(Clone)]
pub struct ExternalIssueAuthorResponse {
    pub username: String,
    pub profile_url: String,
}

#[derive(Clone)]
pub struct IssueResponse {
    pub id: Uuid,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub rendered_body: String,
    pub state: String,
    pub author: IssueUserResponse,
    pub assignee: Option<IssueUserResponse>,
    pub labels: Vec<IssueLabelResponse>,
    pub comment_count: u64,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
    pub closed_at: Option<chrono::DateTime<Utc>>,
    pub external_url: Option<String>,
    pub external_author: Option<ExternalIssueAuthorResponse>,
    pub can_edit: bool,
    pub can_manage: bool,
}

#[derive(Clone)]
pub struct IssueAttachmentResponse {
    pub id: Uuid,
    pub name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub created_at: chrono::DateTime<Utc>,
    pub url: String,
}

#[derive(Clone)]
pub struct RenderedMarkdownResponse {
    pub rendered_html: String,
}

#[derive(Clone)]
pub struct IssueCommentResponse {
    pub id: Uuid,
    pub body: String,
    pub rendered_body: String,
    pub author: IssueUserResponse,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
    pub external_url: Option<String>,
    pub external_author: Option<ExternalIssueAuthorResponse>,
    pub can_edit: bool,
}

#[derive(Clone)]
pub struct ListIssuesQuery {
    pub state: Option<String>,
    pub q: Option<String>,
    pub page: Option<u64>,
    pub per_page: Option<u64>,
}

#[derive(Clone)]
pub struct CreateIssueRequest {
    pub title: String,
    pub body: String,
    pub label_ids: Vec<Uuid>,
    pub attachment_ids: Vec<Uuid>,
    pub assignee: Option<String>,
}

#[derive(Clone)]
pub struct UpdateIssueRequest {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<String>,
    pub assignee: Option<String>,
    pub label_ids: Option<Vec<Uuid>>,
    pub attachment_ids: Option<Vec<Uuid>>,
}

#[derive(Clone)]
pub struct CommentRequest {
    pub body: String,
}

#[derive(Clone)]
pub struct UploadAttachmentQuery {
    pub name: String,
}

#[derive(Clone)]
pub struct PreviewMarkdownRequest {
    pub markdown: String,
}

#[derive(Clone)]
pub struct CreateLabelRequest {
    pub name: String,
    pub color: String,
    pub description: String,
}

#[derive(Clone)]
pub struct UpdateLabelRequest {
    pub name: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
}

async fn enforce_attachment_quotas<C>(
    database: &C,
    repository_id: Uuid,
    uploader_user_id: Uuid,
    incoming_bytes: i64,
) -> Result<(), IssueError>
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
) -> Result<AttachmentUsage, IssueError>
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
        .ok_or_else(|| IssueError::internal("attachment usage query returned no row"))
}

fn ensure_attachment_quota(
    usage: AttachmentUsage,
    incoming_bytes: i64,
    max_count: i64,
    max_bytes: i64,
    message: &'static str,
) -> Result<(), IssueError> {
    if usage.count.saturating_add(1) > max_count
        || usage
            .bytes
            .unwrap_or_default()
            .saturating_add(incoming_bytes)
            > max_bytes
    {
        return Err(IssueError::too_many_requests(message));
    }
    Ok(())
}

async fn assigned_label_ids(
    state: &IssueService<'_>,
    issue_id: Uuid,
) -> Result<Vec<Uuid>, IssueError> {
    Ok(issue_label_assignment::Entity::find()
        .filter(issue_label_assignment::Column::IssueId.eq(issue_id))
        .all(state.database)
        .await?
        .into_iter()
        .map(|assignment| assignment.label_id)
        .collect())
}

/// Translate one issue update into the GitHub-style `issues` actions it implies.
async fn emit_issue_update_events(
    state: &IssueService<'_>,
    repository: &repository::Model,
    actor: &user::Model,
    previous: &repository_issue::Model,
    issue: &repository_issue::Model,
    labels: Option<(&[Uuid], &[Uuid])>,
) {
    let mut changes = serde_json::Map::new();
    if previous.title != issue.title {
        changes.insert(
            "title".to_owned(),
            serde_json::json!({ "from": previous.title }),
        );
    }
    if previous.body != issue.body {
        changes.insert(
            "body".to_owned(),
            serde_json::json!({ "from": previous.body }),
        );
    }
    if !changes.is_empty() {
        webhook_events::issue(
            state,
            repository,
            actor,
            issue,
            IssueAction::Edited,
            serde_json::json!({ "changes": changes }),
        )
        .await;
    }
    if previous.state != issue.state {
        let action = if issue.state == "closed" {
            IssueAction::Closed
        } else {
            IssueAction::Reopened
        };
        webhook_events::issue(
            state,
            repository,
            actor,
            issue,
            action,
            serde_json::json!({}),
        )
        .await;
    }
    if previous.assignee_user_id != issue.assignee_user_id {
        if let Some(old) = previous.assignee_user_id {
            emit_assignment(
                state,
                repository,
                actor,
                issue,
                IssueAction::Unassigned,
                old,
            )
            .await;
        }
        if let Some(new) = issue.assignee_user_id {
            emit_assignment(state, repository, actor, issue, IssueAction::Assigned, new).await;
            state.effects.assigned(repository, actor, issue);
        }
    }
    let Some((before, after)) = labels else {
        return;
    };
    let changed = before
        .iter()
        .filter(|id| !after.contains(id))
        .map(|id| (IssueAction::Unlabeled, *id))
        .chain(
            after
                .iter()
                .filter(|id| !before.contains(id))
                .map(|id| (IssueAction::Labeled, *id)),
        )
        .collect::<Vec<_>>();
    for (action, label_id) in changed {
        let label = match issue_label::Entity::find_by_id(label_id)
            .one(state.database)
            .await
        {
            Ok(Some(label)) => label,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%error, %label_id, "could not load label for webhook event");
                continue;
            }
        };
        webhook_events::issue(
            state,
            repository,
            actor,
            issue,
            action,
            serde_json::json!({ "label": webhook_events::label_payload(&label) }),
        )
        .await;
    }
}

async fn emit_assignment(
    state: &IssueService<'_>,
    repository: &repository::Model,
    actor: &user::Model,
    issue: &repository_issue::Model,
    action: IssueAction,
    assignee_id: Uuid,
) {
    let assignee = user::Entity::find_by_id(assignee_id)
        .one(state.database)
        .await
        .ok()
        .flatten();
    let extra = serde_json::json!({
        "assignee": assignee.as_ref().map(crate::issue_effects::user_payload),
    });
    webhook_events::issue(state, repository, actor, issue, action, extra).await;
}

async fn bind_issue_attachments<C>(
    _state: &IssueService<'_>,
    transaction: &C,
    repository_id: Uuid,
    issue_id: Uuid,
    ids: &[Uuid],
    actor_id: Uuid,
    can_write: bool,
) -> Result<(), IssueError>
where
    C: sea_orm::ConnectionTrait,
{
    let unique_ids = ids.iter().copied().collect::<BTreeSet<_>>();
    if unique_ids.len() != ids.len() {
        return Err(IssueError::bad_request(
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
                IssueError::bad_request("Every attachment must belong to this repository.")
            })?;
        if attachment.issue_id.is_some_and(|bound| bound != issue_id)
            || (attachment.uploader_user_id != actor_id && !can_write)
        {
            return Err(IssueError::bad_request(
                "The attachment cannot be added to this issue.",
            ));
        }
        if attachment.issue_id.is_none() {
            if attached >= MAX_ATTACHMENTS_PER_ISSUE {
                return Err(IssueError::bad_request(format!(
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

fn issue_attachment_directory(state: &IssueService<'_>, repository: &repository::Model) -> PathBuf {
    state
        .lfs_repository_path(repository)
        .join("issue-attachments")
}

pub async fn cleanup_expired_attachments(state: &IssueService<'_>) -> Result<(), IssueError> {
    let cutoff = Utc::now() - UNBOUND_ATTACHMENT_LIFETIME;
    let attachments = issue_attachment::Entity::find()
        .filter(issue_attachment::Column::IssueId.is_null())
        .filter(issue_attachment::Column::CreatedAt.lt(cutoff))
        .order_by_asc(issue_attachment::Column::CreatedAt)
        .limit(ATTACHMENT_CLEANUP_BATCH_SIZE)
        .all(state.database)
        .await?;
    for attachment in attachments {
        let Some(repository) = repository::Entity::find_by_id(attachment.repository_id)
            .one(state.database)
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
            .exec(state.database)
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

fn validate_attachment_name(value: &str) -> Result<String, IssueError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 255
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(IssueError::bad_request(
            "The attachment file name is not valid.",
        ));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod quota_tests {
    use super::*;
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

        assert!(matches!(error, IssueError::Quota(_)));
    }
}

async fn find_issue(
    state: &IssueService<'_>,
    repository_id: Uuid,
    number: i64,
) -> Result<repository_issue::Model, IssueError> {
    repository_issue::Entity::find()
        .filter(repository_issue::Column::RepositoryId.eq(repository_id))
        .filter(repository_issue::Column::Number.eq(number))
        .one(state.database)
        .await?
        .ok_or_else(IssueError::not_found)
}

async fn find_comment(
    state: &IssueService<'_>,
    issue_id: Uuid,
    id: Uuid,
) -> Result<issue_comment::Model, IssueError> {
    issue_comment::Entity::find_by_id(id)
        .filter(issue_comment::Column::IssueId.eq(issue_id))
        .one(state.database)
        .await?
        .ok_or_else(IssueError::not_found)
}

async fn find_label(
    state: &IssueService<'_>,
    repository_id: Uuid,
    id: Uuid,
) -> Result<issue_label::Model, IssueError> {
    issue_label::Entity::find_by_id(id)
        .filter(issue_label::Column::RepositoryId.eq(repository_id))
        .one(state.database)
        .await?
        .ok_or_else(IssueError::not_found)
}

async fn issue_responses(
    state: &IssueService<'_>,
    issues: Vec<repository_issue::Model>,
    viewer_id: Option<Uuid>,
    can_write: bool,
) -> Result<Vec<IssueResponse>, IssueError> {
    if issues.is_empty() {
        return Ok(Vec::new());
    }
    let database = state.database;
    let issue_ids = issues.iter().map(|issue| issue.id).collect::<Vec<_>>();
    // One query per related table for the whole page, however long it is.
    let user_ids = issues
        .iter()
        .flat_map(|issue| [Some(issue.author_user_id), issue.assignee_user_id])
        .flatten()
        .collect();
    let users = issue_users(state, user_ids).await?;
    let mut assignments = Vec::new();
    let mut comment_counts = HashMap::with_capacity(issues.len());
    for ids in issue_ids.chunks(IN_QUERY_CHUNK) {
        assignments.extend(
            issue_label_assignment::Entity::find()
                .filter(issue_label_assignment::Column::IssueId.is_in(ids.iter().copied()))
                .all(database)
                .await?,
        );
        comment_counts.extend(
            issue_comment::Entity::find()
                .select_only()
                .column(issue_comment::Column::IssueId)
                .column_as(issue_comment::Column::Id.count(), "comments")
                .filter(issue_comment::Column::IssueId.is_in(ids.iter().copied()))
                .group_by(issue_comment::Column::IssueId)
                .into_tuple::<(Uuid, i64)>()
                .all(database)
                .await?,
        );
    }
    let label_ids = assignments
        .iter()
        .map(|assignment| assignment.label_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut labels = HashMap::with_capacity(label_ids.len());
    for ids in label_ids.chunks(IN_QUERY_CHUNK) {
        labels.extend(
            issue_label::Entity::find()
                .filter(issue_label::Column::Id.is_in(ids.iter().copied()))
                .all(database)
                .await?
                .into_iter()
                .map(|label| (label.id, label_response(label))),
        );
    }
    let mut issue_labels = HashMap::<Uuid, Vec<IssueLabelResponse>>::new();
    for assignment in assignments {
        if let Some(label) = labels.get(&assignment.label_id) {
            issue_labels
                .entry(assignment.issue_id)
                .or_default()
                .push(label.clone());
        }
    }

    issues
        .into_iter()
        .map(|issue| {
            let author = users
                .get(&issue.author_user_id)
                .cloned()
                .ok_or_else(IssueError::not_found)?;
            let assignee = issue
                .assignee_user_id
                .map(|id| users.get(&id).cloned().ok_or_else(IssueError::not_found))
                .transpose()?;
            let mut labels = issue_labels.remove(&issue.id).unwrap_or_default();
            labels.sort_unstable_by(|left, right| left.name.cmp(&right.name));
            let comment_count = comment_counts
                .get(&issue.id)
                .map_or(0, |count| u64::try_from(*count).unwrap_or_default());
            Ok(build_issue_response(
                issue,
                author,
                assignee,
                labels,
                comment_count,
                viewer_id,
                can_write,
            ))
        })
        .collect()
}

async fn issue_response(
    state: &IssueService<'_>,
    issue: repository_issue::Model,
    viewer_id: Option<Uuid>,
    can_write: bool,
) -> Result<IssueResponse, IssueError> {
    issue_responses(state, vec![issue], viewer_id, can_write)
        .await?
        .pop()
        .ok_or_else(|| IssueError::internal("issue response was not built"))
}

fn build_issue_response(
    issue: repository_issue::Model,
    author: IssueUserResponse,
    assignee: Option<IssueUserResponse>,
    labels: Vec<IssueLabelResponse>,
    comment_count: u64,
    viewer_id: Option<Uuid>,
    can_write: bool,
) -> IssueResponse {
    let external_author = issue
        .external_author
        .as_ref()
        .zip(issue.external_author_url.as_ref())
        .map(|(username, profile_url)| ExternalIssueAuthorResponse {
            username: username.clone(),
            profile_url: profile_url.clone(),
        });
    let imported = issue.external_source.is_some();
    IssueResponse {
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
    }
}

async fn comment_responses(
    state: &IssueService<'_>,
    comments: Vec<issue_comment::Model>,
    viewer_id: Option<Uuid>,
    can_write: bool,
) -> Result<Vec<IssueCommentResponse>, IssueError> {
    let user_ids = comments
        .iter()
        .map(|comment| comment.author_user_id)
        .collect();
    let users = issue_users(state, user_ids).await?;
    comments
        .into_iter()
        .map(|comment| {
            let author = users
                .get(&comment.author_user_id)
                .cloned()
                .ok_or_else(IssueError::not_found)?;
            Ok(build_comment_response(
                comment, author, viewer_id, can_write,
            ))
        })
        .collect()
}

async fn comment_response(
    state: &IssueService<'_>,
    comment: issue_comment::Model,
    viewer_id: Option<Uuid>,
    can_write: bool,
) -> Result<IssueCommentResponse, IssueError> {
    comment_responses(state, vec![comment], viewer_id, can_write)
        .await?
        .pop()
        .ok_or_else(|| IssueError::internal("comment response was not built"))
}

fn build_comment_response(
    comment: issue_comment::Model,
    author: IssueUserResponse,
    viewer_id: Option<Uuid>,
    can_write: bool,
) -> IssueCommentResponse {
    let external_author = comment
        .external_author
        .as_ref()
        .zip(comment.external_author_url.as_ref())
        .map(|(username, profile_url)| ExternalIssueAuthorResponse {
            username: username.clone(),
            profile_url: profile_url.clone(),
        });
    let imported = comment.external_source.is_some();
    IssueCommentResponse {
        id: comment.id,
        rendered_body: render_markdown(&comment.body),
        body: comment.body,
        author,
        created_at: comment.created_at,
        updated_at: comment.updated_at,
        external_url: comment.external_url,
        external_author,
        can_edit: !imported && (viewer_id == Some(comment.author_user_id) || can_write),
    }
}

/// The accounts behind `ids`, loaded together rather than one query each.
async fn issue_users(
    state: &IssueService<'_>,
    ids: BTreeSet<Uuid>,
) -> Result<HashMap<Uuid, IssueUserResponse>, IssueError> {
    let ids = ids.into_iter().collect::<Vec<_>>();
    let mut users = HashMap::with_capacity(ids.len());
    for ids in ids.chunks(IN_QUERY_CHUNK) {
        users.extend(
            user::Entity::find()
                .filter(user::Column::Id.is_in(ids.iter().copied()))
                .all(state.database)
                .await?
                .into_iter()
                .map(|account| {
                    (
                        account.id,
                        IssueUserResponse {
                            id: account.id,
                            username: account.username,
                            avatar_updated_at: account.avatar_updated_at,
                        },
                    )
                }),
        );
    }
    Ok(users)
}

async fn assignee_id(
    state: &IssueService<'_>,
    repository: &repository::Model,
    username: &str,
) -> Result<Option<Uuid>, IssueError> {
    let username = username.trim();
    if username.is_empty() {
        return Ok(None);
    }
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(username))
        .one(state.database)
        .await?
        .ok_or_else(|| IssueError::bad_request("The selected assignee does not exist."))?;
    if !state
        .can_access(repository, Some(account.id), Permission::Write)
        .await?
    {
        return Err(IssueError::bad_request(
            "The assignee must have write access to this repository.",
        ));
    }
    Ok(Some(account.id))
}

async fn validate_label_ids(
    state: &IssueService<'_>,
    repository_id: Uuid,
    ids: &[Uuid],
) -> Result<(), IssueError> {
    for id in ids {
        find_label(state, repository_id, *id)
            .await
            .map_err(|_| IssueError::bad_request("Every label must belong to this repository."))?;
    }
    Ok(())
}

async fn replace_issue_labels_on<C>(
    database: &C,
    issue_id: Uuid,
    label_ids: &[Uuid],
) -> Result<(), IssueError>
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
pub struct ImportedLabel {
    pub external_source: String,
    pub external_instance_url: String,
    pub external_id: String,
    pub external_url: Option<String>,
    pub name: String,
    pub color: String,
    pub description: String,
    pub external_updated_at: Option<chrono::DateTime<Utc>>,
}

// Consumed by the repository import metadata worker.
pub async fn upsert_imported_label(
    state: &IssueService<'_>,
    repository_id: Uuid,
    label: ImportedLabel,
) -> Result<(), IssueError> {
    let database = state.database;
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

fn validate_issue_title(value: &str) -> Result<String, IssueError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_ISSUE_TITLE_LENGTH {
        return Err(IssueError::bad_request(
            "Issue titles must be between 1 and 255 characters.",
        ));
    }
    Ok(value.to_owned())
}

fn validate_issue_body(value: String) -> Result<String, IssueError> {
    if value.len() > MAX_ISSUE_BODY_LENGTH {
        return Err(IssueError::bad_request(
            "Issue descriptions must be at most 1,000,000 characters.",
        ));
    }
    Ok(value)
}

fn validate_comment_body(value: String) -> Result<String, IssueError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_ISSUE_BODY_LENGTH {
        return Err(IssueError::bad_request(
            "Comments must be between 1 and 1,000,000 characters.",
        ));
    }
    Ok(value.to_owned())
}

fn validate_issue_state(value: &str) -> Result<(), IssueError> {
    if matches!(value, "open" | "closed") {
        Ok(())
    } else {
        Err(IssueError::bad_request(
            "Issue state must be open or closed.",
        ))
    }
}

fn validate_label_name(value: &str) -> Result<String, IssueError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_LABEL_NAME_LENGTH {
        return Err(IssueError::bad_request(
            "Label names must be between 1 and 64 characters.",
        ));
    }
    Ok(value.to_owned())
}

fn validate_label_color(value: &str) -> Result<String, IssueError> {
    let value = value.trim().trim_start_matches('#').to_ascii_lowercase();
    if value.len() != 6 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(IssueError::bad_request(
            "Label colors must be six hexadecimal digits.",
        ));
    }
    Ok(value)
}

fn validate_label_description(value: String) -> Result<String, IssueError> {
    let value = value.trim();
    if value.len() > 255 {
        return Err(IssueError::bad_request(
            "Label descriptions must be at most 255 characters.",
        ));
    }
    Ok(value.to_owned())
}

/// Issue failures retain quota exhaustion separately from invalid input.
#[derive(Debug)]
pub enum IssueError {
    Identity(gitadel_identity::IdentityError),
    Quota(&'static str),
}
impl IssueError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self::Identity(gitadel_identity::IdentityError::bad_request(message))
    }
    fn forbidden(message: impl Into<String>) -> Self {
        Self::Identity(gitadel_identity::IdentityError::forbidden(message))
    }
    fn not_found() -> Self {
        Self::Identity(gitadel_identity::IdentityError::not_found())
    }
    fn internal(error: impl std::fmt::Display) -> Self {
        Self::Identity(gitadel_identity::IdentityError::internal(error))
    }
    fn too_many_requests(message: &'static str) -> Self {
        Self::Quota(message)
    }
}
impl From<gitadel_identity::IdentityError> for IssueError {
    fn from(error: gitadel_identity::IdentityError) -> Self {
        Self::Identity(error)
    }
}
impl From<sea_orm::DbErr> for IssueError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Identity(error.into())
    }
}
impl std::fmt::Display for IssueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity(error) => error.fmt(f),
            Self::Quota(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for IssueError {}

#[cfg(test)]
mod direct_boundaries_tests {
    use super::*;
    use gitadel_db::test_support;

    struct NoEffects;
    #[async_trait::async_trait]
    impl IssueEffects for NoEffects {
        async fn issue(
            &self,
            _: &repository::Model,
            _: &user::Model,
            _: &repository_issue::Model,
            _: IssueAction,
            _: serde_json::Value,
        ) {
        }
        async fn comment(
            &self,
            _: &repository::Model,
            _: &user::Model,
            _: &repository_issue::Model,
            _: &issue_comment::Model,
            _: ChangeAction,
            _: Option<&str>,
        ) {
        }
        async fn prepare_deleted(
            &self,
            _: &repository::Model,
            _: &user::Model,
            _: &repository_issue::Model,
        ) -> Option<serde_json::Value> {
            None
        }
        async fn send_deleted(&self, _: Uuid, _: Option<serde_json::Value>) {}
        fn opened(&self, _: &repository::Model, _: &user::Model, _: &repository_issue::Model) {}
        fn assigned(&self, _: &repository::Model, _: &user::Model, _: &repository_issue::Model) {}
        fn commented(
            &self,
            _: &repository::Model,
            _: &user::Model,
            _: &repository_issue::Model,
            _: &issue_comment::Model,
        ) {
        }
    }

    #[tokio::test]
    async fn reader_can_open_and_edit_own_issue_but_cannot_manage_labels() {
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "owner").await;
        let reader = test_support::owner(&database, "reader").await;
        let project = test_support::repository(&database, &owner, "project").await;
        let mut active: repository::ActiveModel = project.clone().into();
        active.visibility = Set("public".to_owned());
        active.update(&database).await.unwrap();
        let raw = gitadel_identity::create_session_on(&database, reader.id, Duration::hours(1))
            .await
            .unwrap();
        let principal = RepositoryPrincipal::session(raw);
        let service = IssueService::new(
            &database,
            std::path::Path::new("unused"),
            Arc::new(NoEffects),
            AuditContext::default(),
        );
        let issue = service
            .create_issue(
                project.id,
                &principal,
                CreateIssueRequest {
                    title: "Question".to_owned(),
                    body: "Details".to_owned(),
                    label_ids: vec![],
                    attachment_ids: vec![],
                    assignee: None,
                },
            )
            .await
            .unwrap();
        assert!(issue.can_edit);
        assert!(!issue.can_manage);
        let changed = service
            .update_issue(
                project.id,
                &principal,
                issue.number,
                UpdateIssueRequest {
                    title: Some("Updated".to_owned()),
                    body: None,
                    state: None,
                    assignee: None,
                    label_ids: None,
                    attachment_ids: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(changed.title, "Updated");
        assert!(matches!(
            service
                .create_label(
                    project.id,
                    &principal,
                    CreateLabelRequest {
                        name: "bug".to_owned(),
                        color: "ff0000".to_owned(),
                        description: String::new()
                    }
                )
                .await,
            Err(IssueError::Identity(
                gitadel_identity::IdentityError::NotFound
            ))
        ));
        assert!(matches!(
            service
                .create_issue(
                    project.id,
                    &RepositoryPrincipal::anonymous(),
                    CreateIssueRequest {
                        title: "Anonymous".to_owned(),
                        body: String::new(),
                        label_ids: vec![],
                        attachment_ids: vec![],
                        assignee: None
                    }
                )
                .await,
            Err(IssueError::Identity(
                gitadel_identity::IdentityError::Unauthorized
            ))
        ));
    }
}
