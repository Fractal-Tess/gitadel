//! Repositories.

use crate::openapi::{Auth, Body, Method, Operation, json, json_list};
use crate::repository::{
    analysis::RepositoryOverviewResponse,
    icon::{IconCandidatesResponse, IconSelectionRequest, UpdateIconRequest},
    registry::RegistryResponse,
    resources::{CreateRepositoryRequest, RepositoryResponse, UpdateRepositoryControlRequest},
    topics::{ReplaceTopicsRequest, TopicsResponse},
};

const TAG: &str = "Repositories";

const FILTER: &[(&str, &str)] = &[
    (
        "q",
        "Case-insensitive substring of the namespace, name, description, or `namespace/name` path",
    ),
    ("namespace", "Only repositories in this namespace"),
];

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(Method::Get, "/repositories", TAG, "List the repositories the caller can read")
        .auth(Auth::Optional)
        .response(json_list::<RepositoryResponse>())
        .query(FILTER),
    Operation::new(Method::Post, "/repositories", TAG, "Create a repository")
        .request(json::<CreateRepositoryRequest>())
        .response(json::<RepositoryResponse>()).created(),
    Operation::new(Method::Get, "/repositories/overview", TAG, "List repositories with branch, language, and activity summaries")
        .auth(Auth::Optional)
        .response(json::<RepositoryOverviewResponse>())
        .query(&[
            ("page", "Page number, starting at 1"),
            ("per_page", "Repositories per page, 1 to 50 (default 20)"),
            ("q", "Case-insensitive substring of the namespace, name, description, or `namespace/name` path"),
            ("namespace", "Only repositories in this namespace"),
        ]),
    Operation::new(Method::Get, "/repositories/{namespace}/{name}", TAG, "Get a repository")
        .auth(Auth::Optional)
        .response(json::<RepositoryResponse>()),
    Operation::new(Method::Post, "/repositories/{namespace}/{name}/archive", TAG, "Archive a repository")
        .request(Body::Empty)
        .response(Body::Empty),
    Operation::new(Method::Delete, "/repositories/{namespace}/{name}/archive", TAG, "Unarchive a repository")
        .response(Body::Empty),
    Operation::new(Method::Patch, "/repositories/{namespace}/{name}/control", TAG, "Rename, move, or reconfigure a repository")
        .request(json::<UpdateRepositoryControlRequest>())
        .response(json::<RepositoryResponse>()),
    Operation::new(Method::Post, "/repositories/{namespace}/{name}/delete", TAG, "Delete a repository")
        .request(Body::Empty)
        .response(Body::Empty),
    Operation::new(Method::Put, "/repositories/{namespace}/{name}/favorite", TAG, "Add a repository to the caller's favorites")
        .request(Body::Empty)
        .response(Body::Empty),
    Operation::new(Method::Delete, "/repositories/{namespace}/{name}/favorite", TAG, "Remove a repository from the caller's favorites")
        .response(Body::Empty),
    Operation::new(Method::Get, "/repositories/{namespace}/{name}/icon", TAG, "Download the repository icon")
        .auth(Auth::Optional)
        .response(Body::Binary("image/png")),
    Operation::new(Method::Put, "/repositories/{namespace}/{name}/icon", TAG, "Upload a repository icon")
        .request(json::<UpdateIconRequest>())
        .response(Body::Empty),
    Operation::new(Method::Delete, "/repositories/{namespace}/{name}/icon", TAG, "Remove the repository icon")
        .response(Body::Empty),
    Operation::new(Method::Get, "/repositories/{namespace}/{name}/icon/candidates", TAG, "List icon candidates found in the repository")
        .auth(Auth::Optional)
        .response(json::<IconCandidatesResponse>()),
    Operation::new(Method::Put, "/repositories/{namespace}/{name}/icon/selection", TAG, "Choose how the repository icon is selected")
        .request(json::<IconSelectionRequest>())
        .response(json::<IconCandidatesResponse>()),
    Operation::new(Method::Get, "/repositories/{namespace}/{name}/registry", TAG, "List the repository's container images")
        .auth(Auth::Optional)
        .response(json::<RegistryResponse>()),
    Operation::new(Method::Get, "/repositories/{namespace}/{name}/topics", TAG, "List the repository's topics")
        .auth(Auth::Optional)
        .response(json::<TopicsResponse>()),
    Operation::new(Method::Put, "/repositories/{namespace}/{name}/topics", TAG, "Replace the repository's topics")
        .request(json::<ReplaceTopicsRequest>())
        .response(json::<TopicsResponse>()),
];
