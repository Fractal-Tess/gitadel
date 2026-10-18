//! Mirrors.

use crate::openapi::{Auth, Method, Operation};

const TAG: &str = "Mirrors";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/namespaces/{namespace}/mirror-identities",
        TAG,
        "List identities",
    ),
    Operation::new(
        Method::Post,
        "/namespaces/{namespace}/mirror-identities",
        TAG,
        "Create identity",
    ),
    Operation::new(
        Method::Put,
        "/namespaces/{namespace}/mirror-identities/{id}",
        TAG,
        "Update identity",
    ),
    Operation::new(
        Method::Delete,
        "/namespaces/{namespace}/mirror-identities/{id}",
        TAG,
        "Delete identity",
    ),
    Operation::new(
        Method::Get,
        "/repositories/{namespace}/{name}/mirror",
        TAG,
        "Get mirror",
    )
    .auth(Auth::Optional),
    Operation::new(
        Method::Patch,
        "/repositories/{namespace}/{name}/mirror",
        TAG,
        "Update mirror",
    ),
    Operation::new(
        Method::Delete,
        "/repositories/{namespace}/{name}/mirror",
        TAG,
        "Convert mirror",
    ),
    Operation::new(
        Method::Post,
        "/repositories/{namespace}/{name}/mirror/sync",
        TAG,
        "Sync mirror",
    ),
];
