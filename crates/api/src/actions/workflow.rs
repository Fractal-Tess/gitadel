//! API compatibility bridge; discovery belongs to Actions.
use super::ActionsState;
use crate::{entity::repository, identity::ApiError};

use uuid::Uuid;
#[expect(
    clippy::too_many_arguments,
    reason = "push fields mirror the Git event contract"
)]
pub(crate) async fn ingest_push(
    state: &ActionsState,
    repository: &repository::Model,
    actor_id: Uuid,
    reference: &str,
    before: &str,
    after: &str,
    zero: &str,
    changed_paths: Vec<String>,
) -> Result<(), ApiError> {
    Ok(gitadel_actions::discovery::ingest_push(
        state.service(),
        repository,
        actor_id,
        reference,
        before,
        after,
        zero,
        changed_paths,
    )
    .await?)
}
