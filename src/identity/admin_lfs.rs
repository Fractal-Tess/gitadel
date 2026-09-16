use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use sea_orm::{ConnectionTrait, DbBackend, QueryResult, Statement, Value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, require_admin};

const DEFAULT_LIMIT: u64 = 10;
const MAX_LIMIT: u64 = 100;

#[derive(Debug, Deserialize)]
pub struct LfsRepositoryUsageQuery {
    pub search: Option<String>,
    pub owner: Option<String>,
    pub owner_type: Option<String>,
    pub min_bytes: Option<u64>,
    pub max_bytes: Option<u64>,
    pub sort: Option<String>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct LfsRepositoryUsage {
    pub repository_id: Uuid,
    pub repository_name: String,
    pub owner_name: String,
    pub owner_type: String,
    pub object_count: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct LfsRepositoryUsageResponse {
    pub repositories: Vec<LfsRepositoryUsage>,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
}

pub async fn list_lfs_repository_usage(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<LfsRepositoryUsageQuery>,
) -> Result<Json<LfsRepositoryUsageResponse>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(ApiError::bad_request(format!(
            "The LFS usage page size must be between 1 and {MAX_LIMIT}."
        )));
    }
    if let (Some(minimum), Some(maximum)) = (query.min_bytes, query.max_bytes)
        && minimum > maximum
    {
        return Err(ApiError::bad_request(
            "The minimum LFS usage cannot exceed the maximum.",
        ));
    }
    let owner_type = query.owner_type.as_deref().unwrap_or("");
    if !owner_type.is_empty() && !matches!(owner_type, "user" | "organization") {
        return Err(ApiError::bad_request(
            "The LFS owner type must be user or organization.",
        ));
    }
    let sort = query.sort.as_deref().unwrap_or("bytes_desc");
    if !matches!(sort, "bytes_desc" | "bytes_asc" | "name") {
        return Err(ApiError::bad_request(
            "The LFS usage sort must be bytes_desc, bytes_asc, or name.",
        ));
    }

    let backend = state.database().get_database_backend();
    let mut values = Vec::new();
    let mut next_placeholder = 1usize;
    let mut where_sql = String::from("r.deleted_at IS NULL");
    let mut having_sql = String::from("SUM(lo.size) > 0");
    let placeholder = |backend: DbBackend, index: usize| match backend {
        DbBackend::Postgres => format!("${index}"),
        _ => format!("?{index}"),
    };
    let add_text_filter = |where_sql: &mut String,
                           values: &mut Vec<Value>,
                           next_placeholder: &mut usize,
                           sql: &str,
                           value: String| {
        let marker = placeholder(backend, *next_placeholder);
        *next_placeholder += 1;
        where_sql.push_str(sql.replace("$PLACEHOLDER", &marker).as_str());
        values.push(value.into());
    };

    if let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        add_text_filter(
            &mut where_sql,
            &mut values,
            &mut next_placeholder,
            " AND LOWER(r.name) LIKE LOWER('%' || $PLACEHOLDER || '%')",
            search.to_owned(),
        );
    }
    if let Some(owner) = query
        .owner
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        add_text_filter(
            &mut where_sql,
            &mut values,
            &mut next_placeholder,
            " AND (LOWER(u.username) LIKE LOWER('%' || $PLACEHOLDER || '%') OR LOWER(o.slug) LIKE LOWER('%' || $PLACEHOLDER || '%') OR LOWER(o.display_name) LIKE LOWER('%' || $PLACEHOLDER || '%'))",
            owner.to_owned(),
        );
    }
    if !owner_type.is_empty() {
        add_text_filter(
            &mut where_sql,
            &mut values,
            &mut next_placeholder,
            " AND n.kind = $PLACEHOLDER",
            owner_type.to_owned(),
        );
    }
    if let Some(minimum) = query.min_bytes {
        let marker = placeholder(backend, next_placeholder);
        next_placeholder += 1;
        having_sql.push_str(&format!(" AND SUM(lo.size) >= {marker}"));
        values.push(
            i64::try_from(minimum)
                .map_err(|_| ApiError::bad_request("The minimum LFS usage is too large."))?
                .into(),
        );
    }
    if let Some(maximum) = query.max_bytes {
        let marker = placeholder(backend, next_placeholder);
        next_placeholder += 1;
        having_sql.push_str(&format!(" AND SUM(lo.size) <= {marker}"));
        values.push(
            i64::try_from(maximum)
                .map_err(|_| ApiError::bad_request("The maximum LFS usage is too large."))?
                .into(),
        );
    }

    // One row per repository keeps shared OIDs from being double-counted inside
    // a repository while still showing each repository's logical ownership.
    let grouped = format!(
        "SELECT r.id AS repository_id, r.name AS repository_name, CASE WHEN n.kind = 'organization' THEN o.slug ELSE u.username END AS owner_name, n.kind AS owner_type, COUNT(lo.oid) AS object_count, SUM(lo.size) AS total_bytes FROM lfs_objects lo JOIN repositories r ON r.id = lo.repository_id JOIN namespaces n ON n.slug = r.namespace LEFT JOIN users u ON u.id = n.user_id LEFT JOIN organizations o ON o.id = n.organization_id WHERE {where_sql} GROUP BY r.id, r.name, n.kind, u.username, o.slug HAVING {having_sql}"
    );
    let total_sql = format!("SELECT COUNT(*) AS total FROM ({grouped}) usage");
    let total_row = state
        .database()
        .query_one_raw(Statement::from_sql_and_values(
            backend,
            total_sql,
            values.clone(),
        ))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("LFS usage count returned no row"))?;
    let total = checked_u64(&total_row, "total")?;

    let order_sql = match sort {
        "bytes_asc" => "total_bytes ASC, repository_name ASC, repository_id ASC",
        "name" => "repository_name ASC, repository_id ASC",
        _ => "total_bytes DESC, repository_name ASC, repository_id ASC",
    };
    let limit_marker = placeholder(backend, next_placeholder);
    next_placeholder += 1;
    let offset_marker = placeholder(backend, next_placeholder);
    values.push(
        i64::try_from(limit)
            .map_err(|_| ApiError::bad_request("The page size is too large."))?
            .into(),
    );
    values.push(
        i64::try_from(query.offset.unwrap_or(0))
            .map_err(|_| ApiError::bad_request("The page offset is too large."))?
            .into(),
    );
    let rows_sql = format!(
        "SELECT repository_id, repository_name, owner_name, owner_type, object_count, total_bytes FROM ({grouped}) usage ORDER BY {order_sql} LIMIT {limit_marker} OFFSET {offset_marker}"
    );
    let rows = state
        .database()
        .query_all_raw(Statement::from_sql_and_values(backend, rows_sql, values))
        .await
        .map_err(ApiError::internal)?;
    let repositories = rows
        .into_iter()
        .map(parse_usage)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(LfsRepositoryUsageResponse {
        repositories,
        total,
        limit,
        offset: query.offset.unwrap_or(0),
    }))
}

fn checked_u64(row: &QueryResult, column: &str) -> Result<u64, ApiError> {
    let value = row.try_get::<i64>("", column).map_err(ApiError::internal)?;
    u64::try_from(value).map_err(ApiError::internal)
}

fn parse_usage(row: QueryResult) -> Result<LfsRepositoryUsage, ApiError> {
    Ok(LfsRepositoryUsage {
        repository_id: row
            .try_get("", "repository_id")
            .map_err(ApiError::internal)?,
        repository_name: row
            .try_get("", "repository_name")
            .map_err(ApiError::internal)?,
        owner_name: row.try_get("", "owner_name").map_err(ApiError::internal)?,
        owner_type: row.try_get("", "owner_type").map_err(ApiError::internal)?,
        object_count: checked_u64(&row, "object_count")?,
        total_bytes: checked_u64(&row, "total_bytes")?,
    })
}
