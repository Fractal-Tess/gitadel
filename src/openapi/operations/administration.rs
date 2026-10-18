//! Administration.

use crate::openapi::{Method, Operation};

const TAG: &str = "Administration";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/admin/authentication",
        TAG,
        "Admin configuration",
    ),
    Operation::new(
        Method::Put,
        "/admin/authentication",
        TAG,
        "Update configuration",
    ),
    Operation::new(
        Method::Get,
        "/admin/authentication/providers",
        TAG,
        "List providers",
    ),
    Operation::new(
        Method::Post,
        "/admin/authentication/providers",
        TAG,
        "Create provider",
    )
    .created(),
    Operation::new(
        Method::Put,
        "/admin/authentication/providers/{provider_id}",
        TAG,
        "Update provider",
    ),
    Operation::new(
        Method::Delete,
        "/admin/authentication/providers/{provider_id}",
        TAG,
        "Delete provider",
    ),
    Operation::new(Method::Get, "/admin/instance", TAG, "Get instance settings"),
    Operation::new(
        Method::Put,
        "/admin/instance",
        TAG,
        "Update instance settings",
    ),
    Operation::new(
        Method::Put,
        "/admin/instance/favicon/{theme}",
        TAG,
        "Update instance favicon",
    ),
    Operation::new(
        Method::Delete,
        "/admin/instance/favicon/{theme}",
        TAG,
        "Delete instance favicon",
    ),
    Operation::new(
        Method::Get,
        "/admin/integrity",
        TAG,
        "Get integrity settings",
    ),
    Operation::new(
        Method::Put,
        "/admin/integrity",
        TAG,
        "Update integrity settings",
    ),
    Operation::new(Method::Get, "/admin/smtp", TAG, "Smtp status"),
    Operation::new(Method::Post, "/admin/smtp/test", TAG, "Send test email"),
    Operation::new(Method::Get, "/admin/users", TAG, "List users"),
    Operation::new(
        Method::Delete,
        "/admin/users/{username}",
        TAG,
        "Delete user",
    ),
    Operation::new(
        Method::Post,
        "/admin/users/{username}/disable",
        TAG,
        "Disable user",
    ),
    Operation::new(
        Method::Post,
        "/admin/users/{username}/enable",
        TAG,
        "Enable user",
    ),
    Operation::new(
        Method::Delete,
        "/admin/users/{username}/two-factor",
        TAG,
        "Reset two factor",
    ),
];
