//! Namespaces.

use crate::{
    identity::resources,
    openapi::{Auth, Method, Operation, json},
    repository::profile,
};

const TAG: &str = "Namespaces";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/namespaces/{slug}",
        TAG,
        "Get whether a namespace is a user or an organization",
    )
    .auth(Auth::Public)
    .response(json::<resources::NamespaceResponse>()),
    Operation::new(
        Method::Get,
        "/namespaces/{slug}/activity",
        TAG,
        "Get a namespace's contribution activity",
    )
    .auth(Auth::Optional)
    .response(json::<profile::NamespaceActivityResponse>()),
    Operation::new(
        Method::Get,
        "/namespaces/{slug}/activity/{date}",
        TAG,
        "Get a namespace's activity on one day",
    )
    .auth(Auth::Optional)
    .response(json::<profile::NamespaceDayResponse>()),
    Operation::new(
        Method::Get,
        "/namespaces/{slug}/pins",
        TAG,
        "List a namespace's pinned repositories",
    )
    .auth(Auth::Optional)
    .response(json::<profile::PinnedRepositoriesResponse>()),
    Operation::new(
        Method::Put,
        "/namespaces/{slug}/pins",
        TAG,
        "Replace a namespace's pinned repositories",
    )
    .request(json::<profile::SetPinsRequest>())
    .response(json::<profile::PinnedRepositoriesResponse>()),
];
