//! Branch and tag protection rules.
//!
//! Rules are loaded once per push into a [`RefGuard`], which is evaluated
//! synchronously inside the receive-pack worker before any ref is written.
//! Every ref write path (HTTP and SSH receive-pack and in-app commits) asks
//! the same guard, so a rule cannot be bypassed by switching transports.

use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Permission, RepositoryState};
use crate::{
    entity::{repository, repository_protection_rule, repository_protection_rule_user, user},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE, validate_slug},
};

const MAX_PATTERN_BYTES: usize = 255;
const MAX_RULES_PER_REPOSITORY: usize = 100;
const MAX_ALLOWED_USERS: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuleKind {
    Branch,
    Tag,
}

impl RuleKind {
    fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "branch" => Ok(Self::Branch),
            "tag" => Ok(Self::Tag),
            _ => Err(ApiError::bad_request(
                "Protection rule kind must be branch or tag.",
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::Tag => "tag",
        }
    }
}

/// The result of checking one ref update against the protection rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RefCheck {
    Allowed,
    Rejected(String),
    /// The update is allowed only if the new commit descends from the old
    /// one. Ancestry needs the incoming objects, so the caller resolves it.
    RequiresFastForward(String),
}

#[derive(Clone, Debug)]
struct GuardRule {
    kind: RuleKind,
    pattern: String,
    block_force_push: bool,
    block_deletion: bool,
    block_update: bool,
    restrict_pushes: bool,
    admins_bypass: bool,
    allowed_users: HashSet<Uuid>,
}

impl GuardRule {
    fn from_model(model: repository_protection_rule::Model, allowed_users: HashSet<Uuid>) -> Self {
        Self {
            kind: RuleKind::parse(&model.kind).unwrap_or(RuleKind::Branch),
            pattern: model.pattern,
            block_force_push: model.block_force_push,
            block_deletion: model.block_deletion,
            block_update: model.block_update,
            restrict_pushes: model.restrict_pushes,
            admins_bypass: model.admins_bypass,
            allowed_users,
        }
    }
}

/// A snapshot of a repository's protection rules and the pushing actor.
#[derive(Clone, Debug, Default)]
pub(crate) struct RefGuard {
    rules: Vec<GuardRule>,
    actor: Option<Uuid>,
    actor_is_admin: bool,
}

impl RefGuard {
    /// Checks one ref update. `old_is_null` means the ref is being created and
    /// `new_is_null` means it is being deleted.
    pub(crate) fn check(&self, name: &str, old_is_null: bool, new_is_null: bool) -> RefCheck {
        let (kind, short) = if let Some(short) = name.strip_prefix("refs/heads/") {
            (RuleKind::Branch, short)
        } else if let Some(short) = name.strip_prefix("refs/tags/") {
            (RuleKind::Tag, short)
        } else {
            return RefCheck::Allowed;
        };
        let mut fast_forward = None;
        for rule in self
            .rules
            .iter()
            .filter(|rule| rule.kind == kind && glob_match(&rule.pattern, short))
        {
            let label = format!("protected {} `{short}`", kind.as_str());
            let exempt = rule.admins_bypass && self.actor_is_admin;
            if rule.restrict_pushes
                && !exempt
                && !self
                    .actor
                    .is_some_and(|actor| rule.allowed_users.contains(&actor))
            {
                return RefCheck::Rejected(format!(
                    "{label}: you are not allowed to push to this ref"
                ));
            }
            if new_is_null && rule.block_deletion {
                return RefCheck::Rejected(format!("{label}: deletion is not allowed"));
            }
            match kind {
                RuleKind::Tag if !old_is_null && rule.block_update => {
                    return RefCheck::Rejected(format!(
                        "{label}: existing tags cannot be moved or deleted"
                    ));
                }
                RuleKind::Branch if !old_is_null && !new_is_null && rule.block_force_push => {
                    fast_forward = Some(format!("{label}: force-push is not allowed"));
                }
                _ => {}
            }
        }
        fast_forward.map_or(RefCheck::Allowed, RefCheck::RequiresFastForward)
    }

    #[cfg(test)]
    pub(crate) fn for_tests(
        rules: Vec<repository_protection_rule::Model>,
        actor: Option<Uuid>,
        actor_is_admin: bool,
        allowed_users: &[Uuid],
    ) -> Self {
        Self {
            rules: rules
                .into_iter()
                .map(|rule| GuardRule::from_model(rule, allowed_users.iter().copied().collect()))
                .collect(),
            actor,
            actor_is_admin,
        }
    }
}

/// Loads the guard for a push by `actor`. Deploy keys push without a user
/// and are therefore never exempt or on an allow list.
pub(crate) async fn load_guard(
    state: &RepositoryState,
    repository: &repository::Model,
    actor: Option<Uuid>,
) -> Result<RefGuard, ApiError> {
    let database = state.identity().database();
    let rules = repository_protection_rule::Entity::find()
        .filter(repository_protection_rule::Column::RepositoryId.eq(repository.id))
        .all(database)
        .await?;
    if rules.is_empty() {
        return Ok(RefGuard::default());
    }
    let mut users = allowed_users(database, rules.iter().map(|rule| rule.id)).await?;
    let actor_is_admin = match actor {
        Some(actor) => {
            state
                .can_access(repository, Some(actor), Permission::Manage)
                .await?
        }
        None => false,
    };
    Ok(RefGuard {
        rules: rules
            .into_iter()
            .map(|rule| {
                let allowed = users
                    .remove(&rule.id)
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                GuardRule::from_model(rule, allowed)
            })
            .collect(),
        actor,
        actor_is_admin,
    })
}

async fn allowed_users<C: ConnectionTrait>(
    database: &C,
    rule_ids: impl Iterator<Item = Uuid>,
) -> Result<HashMap<Uuid, Vec<Uuid>>, ApiError> {
    let rows = repository_protection_rule_user::Entity::find()
        .filter(repository_protection_rule_user::Column::RuleId.is_in(rule_ids))
        .all(database)
        .await?;
    let mut users: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for row in rows {
        users.entry(row.rule_id).or_default().push(row.user_id);
    }
    Ok(users)
}

/// Matches a short ref name against a rule pattern. `*` and `?` stay within
/// one path component, `**` crosses `/`, and everything else is literal.
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    #[derive(Clone, Copy)]
    enum Token {
        Literal(u8),
        Any,
        AnyComponent,
        One,
    }
    let bytes = pattern.as_bytes();
    let mut tokens = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'*' if bytes.get(index + 1) == Some(&b'*') => {
                tokens.push(Token::Any);
                index += 2;
                continue;
            }
            b'*' => tokens.push(Token::AnyComponent),
            b'?' => tokens.push(Token::One),
            byte => tokens.push(Token::Literal(byte)),
        }
        index += 1;
    }
    let name = name.as_bytes();
    // matches[j] is true when tokens[i..] matches name[j..]; computed from the
    // last token backwards so the whole check is O(pattern * name).
    let mut next = vec![false; name.len() + 1];
    next[name.len()] = true;
    for token in tokens.iter().rev() {
        let mut current = vec![false; name.len() + 1];
        for j in (0..=name.len()).rev() {
            current[j] = match *token {
                Token::Literal(byte) => j < name.len() && name[j] == byte && next[j + 1],
                Token::One => j < name.len() && name[j] != b'/' && next[j + 1],
                Token::AnyComponent => {
                    next[j] || (j < name.len() && name[j] != b'/' && current[j + 1])
                }
                Token::Any => next[j] || (j < name.len() && current[j + 1]),
            };
        }
        next = current;
    }
    next[0]
}

fn validate_pattern(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    let value = value
        .strip_prefix("refs/heads/")
        .or_else(|| value.strip_prefix("refs/tags/"))
        .unwrap_or(value);
    if value.is_empty()
        || value.len() > MAX_PATTERN_BYTES
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || value
            .chars()
            .any(|character| character.is_control() || character == ' ')
    {
        return Err(ApiError::bad_request(
            "Protection patterns must be a branch or tag name or a glob such as release/* up to 255 characters.",
        ));
    }
    Ok(value.to_owned())
}

#[derive(Serialize)]
pub struct ProtectionRuleResponse {
    id: Uuid,
    kind: &'static str,
    pattern: String,
    block_force_push: bool,
    block_deletion: bool,
    block_update: bool,
    restrict_pushes: bool,
    admins_bypass: bool,
    allowed_users: Vec<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct CreateProtectionRuleRequest {
    kind: String,
    pattern: String,
    block_force_push: Option<bool>,
    block_deletion: Option<bool>,
    block_update: Option<bool>,
    restrict_pushes: Option<bool>,
    admins_bypass: Option<bool>,
    #[serde(default)]
    allowed_users: Vec<String>,
}

#[derive(Deserialize)]
pub struct UpdateProtectionRuleRequest {
    pattern: Option<String>,
    block_force_push: Option<bool>,
    block_deletion: Option<bool>,
    block_update: Option<bool>,
    restrict_pushes: Option<bool>,
    admins_bypass: Option<bool>,
    allowed_users: Option<Vec<String>>,
}

pub async fn list_rules(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<ProtectionRuleResponse>>, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    let database = state.identity().database();
    let rules = repository_protection_rule::Entity::find()
        .filter(repository_protection_rule::Column::RepositoryId.eq(repository.id))
        .order_by_asc(repository_protection_rule::Column::Kind)
        .order_by_asc(repository_protection_rule::Column::Pattern)
        .all(database)
        .await?;
    rule_responses(database, rules).await.map(Json)
}

pub async fn create_rule(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateProtectionRuleRequest>,
) -> Result<impl IntoResponse, ApiError> {
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
    let kind = RuleKind::parse(&request.kind)?;
    let pattern = validate_pattern(&request.pattern)?;
    let database = state.identity().database();
    let existing = repository_protection_rule::Entity::find()
        .filter(repository_protection_rule::Column::RepositoryId.eq(repository.id))
        .all(database)
        .await?;
    if existing.len() >= MAX_RULES_PER_REPOSITORY {
        return Err(ApiError::bad_request(
            "A repository can have at most 100 protection rules.",
        ));
    }
    if existing
        .iter()
        .any(|rule| rule.kind == kind.as_str() && rule.pattern == pattern)
    {
        return Err(ApiError::conflict(
            "A protection rule for that pattern already exists.",
        ));
    }
    let users = resolve_users(database, &request.allowed_users).await?;
    let now = Utc::now();
    let transaction = database.begin().await?;
    let rule = repository_protection_rule::ActiveModel {
        id: Set(Uuid::new_v4()),
        repository_id: Set(repository.id),
        kind: Set(kind.as_str().to_owned()),
        pattern: Set(pattern.clone()),
        block_force_push: Set(request.block_force_push.unwrap_or(kind == RuleKind::Branch)),
        block_deletion: Set(request.block_deletion.unwrap_or(true)),
        block_update: Set(request.block_update.unwrap_or(kind == RuleKind::Tag)),
        restrict_pushes: Set(request.restrict_pushes.unwrap_or(false)),
        admins_bypass: Set(request.admins_bypass.unwrap_or(true)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    replace_users(&transaction, rule.id, &users).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.protection.create",
            Some(format!("{namespace}/{name}:{}:{pattern}", kind.as_str())),
        )
        .await?;
    transaction.commit().await?;
    let response = rule_responses(database, vec![rule])
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("created protection rule vanished"))?;
    Ok((StatusCode::CREATED, Json(response)))
}

pub async fn update_rule(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateProtectionRuleRequest>,
) -> Result<Json<ProtectionRuleResponse>, ApiError> {
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
    let database = state.identity().database();
    let rule = find_rule(database, repository.id, id).await?;
    let pattern = request
        .pattern
        .as_deref()
        .map(validate_pattern)
        .transpose()?;
    if let Some(pattern) = pattern.as_ref().filter(|pattern| **pattern != rule.pattern)
        && repository_protection_rule::Entity::find()
            .filter(repository_protection_rule::Column::RepositoryId.eq(repository.id))
            .filter(repository_protection_rule::Column::Kind.eq(&rule.kind))
            .filter(repository_protection_rule::Column::Pattern.eq(pattern))
            .one(database)
            .await?
            .is_some()
    {
        return Err(ApiError::conflict(
            "A protection rule for that pattern already exists.",
        ));
    }
    let users = match &request.allowed_users {
        Some(usernames) => Some(resolve_users(database, usernames).await?),
        None => None,
    };
    let transaction = database.begin().await?;
    let target = format!("{namespace}/{name}:{}:{}", rule.kind, rule.pattern);
    let mut active: repository_protection_rule::ActiveModel = rule.into();
    if let Some(pattern) = pattern {
        active.pattern = Set(pattern);
    }
    if let Some(value) = request.block_force_push {
        active.block_force_push = Set(value);
    }
    if let Some(value) = request.block_deletion {
        active.block_deletion = Set(value);
    }
    if let Some(value) = request.block_update {
        active.block_update = Set(value);
    }
    if let Some(value) = request.restrict_pushes {
        active.restrict_pushes = Set(value);
    }
    if let Some(value) = request.admins_bypass {
        active.admins_bypass = Set(value);
    }
    active.updated_at = Set(Utc::now());
    let rule = active.update(&transaction).await?;
    if let Some(users) = users {
        replace_users(&transaction, rule.id, &users).await?;
    }
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.protection.update",
            Some(target),
        )
        .await?;
    transaction.commit().await?;
    rule_responses(database, vec![rule])
        .await?
        .pop()
        .map(Json)
        .ok_or_else(|| ApiError::internal("updated protection rule vanished"))
}

pub async fn delete_rule(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
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
    let database = state.identity().database();
    let rule = find_rule(database, repository.id, id).await?;
    let transaction = database.begin().await?;
    repository_protection_rule::Entity::delete_by_id(rule.id)
        .exec(&transaction)
        .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.protection.delete",
            Some(format!("{namespace}/{name}:{}:{}", rule.kind, rule.pattern)),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn find_rule<C: ConnectionTrait>(
    database: &C,
    repository_id: Uuid,
    id: Uuid,
) -> Result<repository_protection_rule::Model, ApiError> {
    repository_protection_rule::Entity::find_by_id(id)
        .filter(repository_protection_rule::Column::RepositoryId.eq(repository_id))
        .one(database)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn resolve_users<C: ConnectionTrait>(
    database: &C,
    usernames: &[String],
) -> Result<Vec<Uuid>, ApiError> {
    if usernames.len() > MAX_ALLOWED_USERS {
        return Err(ApiError::bad_request(
            "A protection rule can allow at most 100 users.",
        ));
    }
    let mut ids = Vec::with_capacity(usernames.len());
    for username in usernames {
        let username = validate_slug(username, "Username")?;
        let account = user::Entity::find()
            .filter(user::Column::Username.eq(&username))
            .one(database)
            .await?
            .ok_or_else(|| ApiError::bad_request(format!("User `{username}` does not exist.")))?;
        if !ids.contains(&account.id) {
            ids.push(account.id);
        }
    }
    Ok(ids)
}

async fn replace_users<C: ConnectionTrait>(
    database: &C,
    rule_id: Uuid,
    users: &[Uuid],
) -> Result<(), ApiError> {
    repository_protection_rule_user::Entity::delete_many()
        .filter(repository_protection_rule_user::Column::RuleId.eq(rule_id))
        .exec(database)
        .await?;
    for user_id in users {
        repository_protection_rule_user::ActiveModel {
            rule_id: Set(rule_id),
            user_id: Set(*user_id),
        }
        .insert(database)
        .await?;
    }
    Ok(())
}

async fn rule_responses<C: ConnectionTrait>(
    database: &C,
    rules: Vec<repository_protection_rule::Model>,
) -> Result<Vec<ProtectionRuleResponse>, ApiError> {
    let mut users = allowed_users(database, rules.iter().map(|rule| rule.id)).await?;
    let user_ids = users.values().flatten().copied().collect::<HashSet<_>>();
    let usernames = user::Entity::find()
        .filter(user::Column::Id.is_in(user_ids))
        .all(database)
        .await?
        .into_iter()
        .map(|account| (account.id, account.username))
        .collect::<HashMap<_, _>>();
    Ok(rules
        .into_iter()
        .map(|rule| {
            let mut allowed_users = users
                .remove(&rule.id)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|id| usernames.get(&id).cloned())
                .collect::<Vec<_>>();
            allowed_users.sort();
            ProtectionRuleResponse {
                id: rule.id,
                kind: RuleKind::parse(&rule.kind)
                    .unwrap_or(RuleKind::Branch)
                    .as_str(),
                pattern: rule.pattern,
                block_force_push: rule.block_force_push,
                block_deletion: rule.block_deletion,
                block_update: rule.block_update,
                restrict_pushes: rule.restrict_pushes,
                admins_bypass: rule.admins_bypass,
                allowed_users,
                created_at: rule.created_at,
                updated_at: rule.updated_at,
            }
        })
        .collect())
}

#[cfg(test)]
pub(crate) fn test_rule(kind: &str, pattern: &str) -> repository_protection_rule::Model {
    let now = Utc::now();
    repository_protection_rule::Model {
        id: Uuid::new_v4(),
        repository_id: Uuid::new_v4(),
        kind: kind.to_owned(),
        pattern: pattern.to_owned(),
        block_force_push: kind == "branch",
        block_deletion: true,
        block_update: kind == "tag",
        restrict_pushes: false,
        admins_bypass: true,
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_patterns_respect_path_components() {
        assert!(glob_match("main", "main"));
        assert!(!glob_match("main", "main2"));
        assert!(glob_match("release/*", "release/1.0"));
        assert!(!glob_match("release/*", "release/1.0/hotfix"));
        assert!(glob_match("release/**", "release/1.0/hotfix"));
        assert!(glob_match("v*", "v1.2.3"));
        assert!(!glob_match("v*", "x1"));
        assert!(glob_match("v?", "v1"));
        assert!(!glob_match("v?", "v12"));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("*", "feature/x"));
        assert!(glob_match("**", "feature/x"));
        assert!(glob_match("*-stable", "2.0-stable"));
    }

    #[test]
    fn branch_rules_block_deletion_and_request_fast_forward_checks() {
        let guard = RefGuard::for_tests(vec![test_rule("branch", "main")], None, false, &[]);
        assert_eq!(
            guard.check("refs/heads/main", false, true),
            RefCheck::Rejected("protected branch `main`: deletion is not allowed".into())
        );
        assert!(matches!(
            guard.check("refs/heads/main", false, false),
            RefCheck::RequiresFastForward(_)
        ));
        assert_eq!(
            guard.check("refs/heads/main", true, false),
            RefCheck::Allowed
        );
        assert_eq!(
            guard.check("refs/heads/feature", false, true),
            RefCheck::Allowed
        );
        assert_eq!(
            guard.check("refs/notes/main", false, true),
            RefCheck::Allowed
        );
    }

    #[test]
    fn tag_rules_block_moving_existing_tags_but_allow_new_ones() {
        let guard = RefGuard::for_tests(vec![test_rule("tag", "v*")], None, false, &[]);
        assert_eq!(
            guard.check("refs/tags/v1.0", true, false),
            RefCheck::Allowed
        );
        assert!(matches!(
            guard.check("refs/tags/v1.0", false, false),
            RefCheck::Rejected(_)
        ));
        assert!(matches!(
            guard.check("refs/tags/v1.0", false, true),
            RefCheck::Rejected(_)
        ));
        assert_eq!(
            guard.check("refs/tags/nightly", false, true),
            RefCheck::Allowed
        );
    }

    #[test]
    fn push_restrictions_allow_listed_users_and_optionally_admins() {
        let allowed = Uuid::new_v4();
        let other = Uuid::new_v4();
        let mut rule = test_rule("branch", "main");
        rule.restrict_pushes = true;
        rule.block_force_push = false;
        let check = |rule: &repository_protection_rule::Model, actor, admin| {
            RefGuard::for_tests(vec![rule.clone()], actor, admin, &[allowed]).check(
                "refs/heads/main",
                false,
                false,
            )
        };
        assert_eq!(check(&rule, Some(allowed), false), RefCheck::Allowed);
        assert!(matches!(
            check(&rule, Some(other), false),
            RefCheck::Rejected(_)
        ));
        assert!(matches!(check(&rule, None, false), RefCheck::Rejected(_)));
        assert_eq!(check(&rule, Some(other), true), RefCheck::Allowed);
        rule.admins_bypass = false;
        assert!(matches!(
            check(&rule, Some(other), true),
            RefCheck::Rejected(_)
        ));
    }
}
