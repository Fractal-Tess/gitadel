//! Persistent import recovery.

use chrono::Utc;
use gitadel_db::entity::{repository, repository_import, repository_import_item};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, IntoActiveModel,
    QueryFilter, Set,
};

pub async fn recover_interrupted(database: &DatabaseConnection) -> Result<(), DbErr> {
    let now = Utc::now();
    repository_import_item::Entity::update_many()
        .col_expr(
            repository_import_item::Column::State,
            sea_orm::sea_query::Expr::value("credentials_required"),
        )
        .col_expr(
            repository_import_item::Column::LastError,
            sea_orm::sea_query::Expr::value(
                "Gitadel restarted. Reconnect the source credential to resume.",
            ),
        )
        .col_expr(
            repository_import_item::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(repository_import_item::Column::State.is_in(["queued", "cloning", "metadata"]))
        .exec(database)
        .await?;
    repository_import::Entity::update_many()
        .col_expr(
            repository_import::Column::State,
            sea_orm::sea_query::Expr::value("credentials_required"),
        )
        .col_expr(
            repository_import::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(repository_import::Column::State.is_in(["queued", "running"]))
        .exec(database)
        .await?;
    Ok(())
}

/// Forge adapters supply the clone and provider metadata operations.
#[async_trait::async_trait]
pub trait ImportEffects: Send + Sync {
    async fn clone_repository(
        &self,
        import: &repository_import::Model,
        item: &repository_import_item::Model,
    ) -> Result<repository::Model, String>;
    async fn import_metadata(
        &self,
        import: &repository_import::Model,
        item: &repository_import_item::Model,
        repository: &repository::Model,
    ) -> Result<Option<String>, String>;
}

fn database_failure(error: DbErr) -> String {
    tracing::error!(%error, "import database operation failed");
    "The request could not be completed.".to_owned()
}

pub async fn run_items(
    database: &DatabaseConnection,
    auth: &gitadel_identity::runtime::AuthService<'_>,
    mut import: repository_import::Model,
    items: Vec<repository_import_item::Model>,
    effects: &dyn ImportEffects,
) {
    import.state = "running".to_owned();
    import.updated_at = Utc::now();
    let mut active = import.clone().into_active_model();
    active.reset(repository_import::Column::State);
    active.reset(repository_import::Column::UpdatedAt);
    if let Err(error) = active.update(database).await {
        tracing::error!(import_id = %import.id, %error, "could not mark repository import as running");
    }
    for queued_item in items {
        let Ok(Some(mut item)) = repository_import_item::Entity::find_by_id(queued_item.id)
            .one(database)
            .await
        else {
            continue;
        };
        if item.state != "queued" {
            continue;
        }

        item.attempts += 1;
        item.state = if item.repository_id.is_some() {
            "metadata"
        } else {
            "cloning"
        }
        .to_owned();
        item.last_error = None;
        item.updated_at = Utc::now();
        let mut active = item.into_active_model();
        active.reset(repository_import_item::Column::Attempts);
        active.reset(repository_import_item::Column::State);
        active.reset(repository_import_item::Column::LastError);
        active.reset(repository_import_item::Column::UpdatedAt);
        let Ok(mut item) = active.update(database).await else {
            continue;
        };

        let repository_result = match item.repository_id {
            Some(repository_id) => repository::Entity::find_by_id(repository_id)
                .one(database)
                .await
                .map_err(database_failure)
                .and_then(|repository| {
                    repository.ok_or_else(|| {
                        "The repository created by this import no longer exists.".to_owned()
                    })
                }),
            None => effects.clone_repository(&import, &item).await,
        };

        let result = match repository_result {
            Ok(repository) => {
                item.repository_id = Some(repository.id);
                if let Some(identity_id) = import.identity_id {
                    let _ = gitadel_identity::mirror_identities::mark_identity_used(
                        auth,
                        &import.target_namespace,
                        identity_id,
                        Utc::now(),
                    )
                    .await;
                }
                item.state = "metadata".to_owned();
                item.updated_at = Utc::now();
                let mut active = item.clone().into_active_model();
                active.reset(repository_import_item::Column::RepositoryId);
                active.reset(repository_import_item::Column::State);
                active.reset(repository_import_item::Column::UpdatedAt);
                match active.update(database).await {
                    Ok(saved) => {
                        item = saved;
                        if import.provider == "direct" {
                            Ok(None)
                        } else {
                            effects.import_metadata(&import, &item, &repository).await
                        }
                    }
                    Err(error) => Err(database_failure(error)),
                }
            }
            Err(error) => Err(error),
        };

        let item_id = item.id;
        let mut active = item.into_active_model();
        active.updated_at = Set(Utc::now());
        match result {
            Ok(warning) => {
                active.state = Set("completed".to_owned());
                active.last_error = Set(warning);
            }
            Err(error) => {
                active.state = Set("failed".to_owned());
                active.last_error = Set(Some(error));
            }
        }
        if let Err(error) = active.update(database).await {
            tracing::error!(import_id = %import.id, %item_id, %error, "could not record repository import item result");
        }
    }
    let all = match repository_import_item::Entity::find()
        .filter(repository_import_item::Column::ImportId.eq(import.id))
        .all(database)
        .await
    {
        Ok(items) => items,
        Err(error) => {
            tracing::error!(import_id = %import.id, %error, "could not finalize repository import");
            import.state = "completed_with_errors".to_owned();
            import.updated_at = Utc::now();
            let import_id = import.id;
            let mut active = import.into_active_model();
            active.reset(repository_import::Column::State);
            active.reset(repository_import::Column::UpdatedAt);
            if let Err(error) = active.update(database).await {
                tracing::error!(%import_id, %error, "could not record repository import state");
            }
            return;
        }
    };
    import.state = if all.iter().all(|item| item.state == "completed") {
        "completed"
    } else if all.iter().any(|item| item.state == "failed") {
        "completed_with_errors"
    } else if all.iter().any(|item| item.state == "credentials_required") {
        "credentials_required"
    } else if all.iter().any(|item| item.state == "cancelled") {
        "cancelled"
    } else {
        "completed_with_errors"
    }
    .to_owned();
    import.updated_at = Utc::now();
    let import_id = import.id;
    let final_state = import.state.clone();
    let mut active = import.into_active_model();
    active.reset(repository_import::Column::State);
    active.reset(repository_import::Column::UpdatedAt);
    if let Err(error) = active.update(database).await {
        tracing::error!(%import_id, state = %final_state, %error, "could not record final repository import state");
    }
}
