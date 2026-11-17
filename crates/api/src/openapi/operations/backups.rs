//! Backups.

use crate::openapi::{Body, Method, Operation};

const TAG: &str = "Backups";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/admin/backup/providers",
        TAG,
        "List backup providers",
    ),
    Operation::new(
        Method::Post,
        "/admin/backup/providers",
        TAG,
        "Create backup provider",
    )
    .created(),
    Operation::new(
        Method::Post,
        "/admin/backup/providers/test",
        TAG,
        "Test backup provider",
    ),
    Operation::new(
        Method::Put,
        "/admin/backup/providers/{provider_id}",
        TAG,
        "Update backup provider",
    ),
    Operation::new(
        Method::Delete,
        "/admin/backup/providers/{provider_id}",
        TAG,
        "Delete backup provider",
    ),
    Operation::new(
        Method::Get,
        "/admin/backup/providers/{provider_id}/backups",
        TAG,
        "List backups",
    ),
    Operation::new(
        Method::Post,
        "/admin/backup/providers/{provider_id}/backups",
        TAG,
        "Create backup",
    )
    .accepted(),
    Operation::new(
        Method::Get,
        "/admin/backup/providers/{provider_id}/backups/object",
        TAG,
        "Download backup",
    ),
    Operation::new(
        Method::Delete,
        "/admin/backup/providers/{provider_id}/backups/object",
        TAG,
        "Delete backup",
    ),
    Operation::new(
        Method::Post,
        "/admin/backup/providers/{provider_id}/backups/preflight",
        TAG,
        "Preflight restore",
    ),
    Operation::new(
        Method::Post,
        "/admin/backup/providers/{provider_id}/backups/restore",
        TAG,
        "Restore backup",
    )
    .accepted(),
    Operation::new(
        Method::Put,
        "/admin/backup/providers/{provider_id}/schedule",
        TAG,
        "Update backup schedule",
    ),
    Operation::new(
        Method::Get,
        "/admin/backups/progress/{operation_id}",
        TAG,
        "Backup progress",
    )
    .response(Body::EventStream),
];
