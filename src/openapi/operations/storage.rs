//! Storage.

use crate::openapi::{Body, Method, Operation};

const TAG: &str = "Storage";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(Method::Get, "/admin/storage/domains", TAG, "List domains"),
    Operation::new(
        Method::Get,
        "/admin/storage/domains/{domain}",
        TAG,
        "Domain status",
    ),
    Operation::new(
        Method::Post,
        "/admin/storage/domains/{domain}/migrate",
        TAG,
        "Migrate domain",
    )
    .accepted(),
    Operation::new(
        Method::Get,
        "/admin/storage/domains/{domain}/migrations/{operation_id}",
        TAG,
        "Domain migration",
    ),
    Operation::new(
        Method::Get,
        "/admin/storage/domains/{domain}/migrations/{operation_id}/events",
        TAG,
        "Domain migration events",
    )
    .response(Body::EventStream),
    Operation::new(
        Method::Get,
        "/admin/storage/domains/{domain}/repositories",
        TAG,
        "Domain repositories",
    ),
    Operation::new(
        Method::Get,
        "/admin/storage/lfs/repositories",
        TAG,
        "Lfs repositories",
    ),
    Operation::new(Method::Get, "/admin/storage/lfs/status", TAG, "Lfs status"),
    Operation::new(
        Method::Post,
        "/admin/storage/migrations",
        TAG,
        "Lfs migrate",
    ),
    Operation::new(
        Method::Get,
        "/admin/storage/progress/{operation_id}",
        TAG,
        "Lfs progress",
    )
    .response(Body::EventStream),
    Operation::new(
        Method::Get,
        "/admin/storage/overview",
        TAG,
        "Storage overview",
    ),
    Operation::new(
        Method::Post,
        "/admin/storage/registry/migrate",
        TAG,
        "Registry migrate",
    )
    .accepted(),
    Operation::new(
        Method::Get,
        "/admin/storage/registry/migrations/{operation_id}/events",
        TAG,
        "Registry events",
    )
    .response(Body::EventStream),
    Operation::new(
        Method::Get,
        "/admin/storage/registry/repositories",
        TAG,
        "Registry repositories",
    ),
    Operation::new(
        Method::Get,
        "/admin/storage/registry/status",
        TAG,
        "Registry status",
    ),
    Operation::new(
        Method::Get,
        "/admin/storage/targets",
        TAG,
        "List storage targets",
    ),
    Operation::new(
        Method::Post,
        "/admin/storage/targets",
        TAG,
        "Create storage target",
    )
    .created(),
    Operation::new(
        Method::Post,
        "/admin/storage/targets/test",
        TAG,
        "Test storage target",
    ),
    Operation::new(
        Method::Delete,
        "/admin/storage/targets/{target_id}",
        TAG,
        "Delete storage target",
    ),
    Operation::new(
        Method::Post,
        "/admin/storage/targets/{target_id}/test",
        TAG,
        "Test saved storage target",
    ),
    Operation::new(
        Method::Post,
        "/admin/storage/targets/{target_id}/usage",
        TAG,
        "Measure storage target",
    ),
];
