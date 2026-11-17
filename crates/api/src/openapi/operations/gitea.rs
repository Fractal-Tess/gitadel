//! Gitea compatibility.

use crate::{
    openapi::{Method, Operation, json, json_list},
    repository::{gitea::GiteaBranchResponse, releases::ForgejoTagResponse},
};

const TAG: &str = "Gitea compatibility";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/branches",
        TAG,
        "List branches (Gitea compatible)",
    )
    .response(json_list::<GiteaBranchResponse>())
    .query(&[
        ("page", "Page number, starting at 1"),
        ("limit", "Branches per page, from 1 to 50 (default 30)"),
    ]),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/git/refs/tags/{tag}",
        TAG,
        "Get a tag's commit (Forgejo compatible)",
    )
    .response(json::<ForgejoTagResponse>()),
    Operation::new(
        Method::Get,
        "/repos/{namespace}/{name}/tags/{tag}",
        TAG,
        "Get a tag (Forgejo compatible)",
    )
    .response(json::<ForgejoTagResponse>()),
];
