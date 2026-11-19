//! Repository-visible registry catalog pages.
use gitadel_identity::{IdentityError, permissions::RepositoryPrincipal};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};
use uuid::Uuid;
/// Reauthenticate the credential with repository-read scope before catalog access.
pub async fn catalog_page(
    database: &DatabaseConnection,
    principal: &RepositoryPrincipal,
    last: &str,
    size: usize,
) -> Result<(Vec<String>, bool), IdentityError> {
    let actor = principal
        .optional_user(database, gitadel_identity::SCOPE_REPOSITORY_READ)
        .await?
        .map(|user| user.id);
    Ok(catalog_page_for_actor(database, actor, last, size).await?)
}
/// One page of image names the actor may pull, in name order after `last`,
/// and whether more follow.
///
/// The catalog is reachable without credentials, so its cost must not grow
/// with the number of repositories: readability, the `last` cursor, ordering,
/// and the page limit are all applied by one statement over the images that
/// exist, rather than by an access query and an image listing per repository.
///
/// Readability uses the same identity policy as repository operations.
pub async fn catalog_page_for_actor(
    database: &DatabaseConnection,
    actor: Option<Uuid>,
    last: &str,
    size: usize,
) -> Result<(Vec<String>, bool), DbErr> {
    let readable = gitadel_identity::permissions::repository_policy_sql(
        "r",
        "?1",
        actor.is_some(),
        gitadel_identity::permissions::RepositoryPermission::Read,
    );
    let sql = format!(
        "SELECT image FROM (\
             SELECT r.namespace || '/' || r.name || \
                    CASE WHEN i.name = '' THEN '' ELSE '/' || i.name END AS image \
             FROM registry_images i JOIN repositories r ON r.id = i.repository_id \
             WHERE {readable}\
         ) WHERE image > ?2 ORDER BY image LIMIT ?3"
    );
    let limit = size as u64 + 1;
    let mut names = Vec::with_capacity(size + 1);
    let mut cursor = last.to_owned();
    loop {
        let rows = database
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                &sql,
                [actor.into(), cursor.as_str().into(), limit.into()],
            ))
            .await?;
        let fetched = rows.len() as u64;
        for row in rows {
            cursor = row.try_get::<String>("", "image")?;
            // Rows written by an older layout may not name a pullable image.
            if names.len() <= size
                && (cursor.contains('/') && crate::valid_image_path_components(&cursor))
            {
                names.push(cursor.clone());
            }
        }
        if names.len() > size || fetched < limit {
            break;
        }
    }
    let more = names.len() > size;
    names.truncate(size);
    Ok((names, more))
}
