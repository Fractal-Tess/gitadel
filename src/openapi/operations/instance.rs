//! Instance.

use crate::openapi::{Auth, Method, Operation};

const TAG: &str = "Instance";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(Method::Get, "/openapi.json", TAG, "This OpenAPI document").auth(Auth::Public),
    Operation::new(Method::Get, "/", TAG, "Version").auth(Auth::Public),
    Operation::new(Method::Get, "/changelog", TAG, "Changelog").auth(Auth::Public),
    Operation::new(Method::Get, "/instance", TAG, "Public instance settings").auth(Auth::Public),
    Operation::new(
        Method::Get,
        "/instance/favicon/{theme}",
        TAG,
        "Public instance favicon",
    )
    .auth(Auth::Public),
    Operation::new(Method::Post, "/setup", TAG, "Setup")
        .auth(Auth::Public)
        .created(),
    Operation::new(Method::Get, "/topics", TAG, "Suggest topics").auth(Auth::Optional),
    Operation::new(Method::Get, "/version", TAG, "Forgejo version").auth(Auth::Public),
];
