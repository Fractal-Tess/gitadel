//! Commit statuses.

use crate::{
    openapi::{Auth, Method, Operation, json, json_list},
    repository::statuses::{CombinedStatusResponse, CreateStatusRequest, StatusResponse},
};

const TAG: &str = "Commit statuses";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/commits/{revision}/status",
        TAG,
        "Get a commit's combined status",
    )
    .auth(Auth::Optional)
    .response(json::<CombinedStatusResponse>()),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/commits/{revision}/statuses",
        TAG,
        "List a commit's statuses",
    )
    .auth(Auth::Optional)
    .response(json_list::<StatusResponse>()),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/statuses/{sha}",
        TAG,
        "List a commit's statuses by SHA",
    )
    .auth(Auth::Optional)
    .response(json_list::<StatusResponse>()),
    Operation::new(
        Method::Post,
        "/repos/{namespace}/{name}/statuses/{sha}",
        TAG,
        "Set a commit status",
    )
    .request(json::<CreateStatusRequest>())
    .response(json::<StatusResponse>())
    .created(),
];
