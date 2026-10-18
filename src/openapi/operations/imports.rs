//! Imports.

use crate::openapi::{Method, Operation};

const TAG: &str = "Imports";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(Method::Post, "/repository-imports", TAG, "Create import").accepted(),
    Operation::new(
        Method::Post,
        "/repository-imports/direct",
        TAG,
        "Create direct import",
    )
    .accepted(),
    Operation::new(
        Method::Post,
        "/repository-imports/discover",
        TAG,
        "Discover",
    ),
    Operation::new(Method::Get, "/repository-imports/{id}", TAG, "Get import"),
    Operation::new(
        Method::Delete,
        "/repository-imports/{id}",
        TAG,
        "Cancel import",
    ),
    Operation::new(
        Method::Post,
        "/repository-imports/{id}/retry",
        TAG,
        "Retry import",
    )
    .accepted(),
];
