//! Every `/api/v1` operation, one file per tag.

mod account;
mod actions;
mod administration;
mod authentication;
mod backups;
mod collaborators;
mod content;
mod deployments;
mod gitea;
mod imports;
mod instance;
mod issues;
mod mirrors;
mod namespaces;
mod organizations;
mod protection;
mod releases;
mod repositories;
mod statuses;
mod storage;
mod webhooks;

use super::Operation;

/// All described operations, in no particular order.
pub(crate) fn operations() -> impl Iterator<Item = &'static Operation> {
    [
        account::OPERATIONS,
        actions::OPERATIONS,
        administration::OPERATIONS,
        authentication::OPERATIONS,
        backups::OPERATIONS,
        collaborators::OPERATIONS,
        content::OPERATIONS,
        deployments::OPERATIONS,
        gitea::OPERATIONS,
        imports::OPERATIONS,
        instance::OPERATIONS,
        issues::OPERATIONS,
        mirrors::OPERATIONS,
        namespaces::OPERATIONS,
        organizations::OPERATIONS,
        protection::OPERATIONS,
        releases::OPERATIONS,
        repositories::OPERATIONS,
        statuses::OPERATIONS,
        storage::OPERATIONS,
        webhooks::OPERATIONS,
    ]
    .into_iter()
    .flatten()
}
