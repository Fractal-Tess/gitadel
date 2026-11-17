//! Persisted repository cache entries with the existing TTL and payload format.
use gitadel_db::entity::repository_cache_entry;
use gitadel_identity::IdentityError;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    sea_query::OnConflict,
};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
use uuid::Uuid;
#[derive(Clone)]
pub struct RepositoryCache {
    writer: DatabaseConnection,
    reader: DatabaseConnection,
}
impl RepositoryCache {
    pub fn new(writer: DatabaseConnection, reader: DatabaseConnection) -> Self {
        Self { writer, reader }
    }
    pub async fn cached_value<T>(
        &self,
        repository_id: Uuid,
        kind: &str,
        cache_key: &str,
    ) -> Result<Option<T>, IdentityError>
    where
        T: DeserializeOwned,
    {
        let id = (repository_id, kind.to_owned(), cache_key.to_owned());
        let Some(entry) = repository_cache_entry::Entity::find_by_id(id.clone())
            .one(&self.reader)
            .await?
        else {
            return Ok(None);
        };
        if entry.expires_at <= chrono::Utc::now() {
            repository_cache_entry::Entity::delete_by_id(id)
                .exec(&self.writer)
                .await?;
            return Ok(None);
        }
        match serde_json::from_str(&entry.payload_json) {
            Ok(value) => Ok(Some(value)),
            Err(error) => {
                tracing::warn!(
                    %error,
                    %repository_id,
                    cache_kind = kind,
                    "discarding invalid repository cache entry"
                );
                repository_cache_entry::Entity::delete_by_id(id)
                    .exec(&self.writer)
                    .await?;
                Ok(None)
            }
        }
    }

    pub async fn cache_value<T>(
        &self,
        repository_id: Uuid,
        kind: &str,
        cache_key: &str,
        value: &T,
        lifetime: Duration,
    ) -> Result<(), IdentityError>
    where
        T: Serialize + ?Sized,
    {
        let now = chrono::Utc::now();
        let expires_at =
            now + chrono::Duration::from_std(lifetime).map_err(IdentityError::internal)?;
        let entry = repository_cache_entry::ActiveModel {
            repository_id: Set(repository_id),
            kind: Set(kind.to_owned()),
            cache_key: Set(cache_key.to_owned()),
            payload_json: Set(serde_json::to_string(value).map_err(IdentityError::internal)?),
            expires_at: Set(expires_at),
            created_at: Set(now),
        };
        repository_cache_entry::Entity::insert(entry)
            .on_conflict(
                OnConflict::columns([
                    repository_cache_entry::Column::RepositoryId,
                    repository_cache_entry::Column::Kind,
                    repository_cache_entry::Column::CacheKey,
                ])
                .update_columns([
                    repository_cache_entry::Column::PayloadJson,
                    repository_cache_entry::Column::ExpiresAt,
                    repository_cache_entry::Column::CreatedAt,
                ])
                .to_owned(),
            )
            .exec(&self.writer)
            .await?;
        repository_cache_entry::Entity::delete_many()
            .filter(repository_cache_entry::Column::ExpiresAt.lte(now))
            .exec(&self.writer)
            .await?;
        Ok(())
    }
}
