//! Authentication.

use crate::openapi::{Auth, Method, Operation};

const TAG: &str = "Authentication";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(Method::Post, "/auth/email/verify", TAG, "Verify email").auth(Auth::Public),
    Operation::new(Method::Post, "/auth/login", TAG, "Login").auth(Auth::Public),
    Operation::new(
        Method::Post,
        "/auth/login/two-factor",
        TAG,
        "Finish two factor login",
    )
    .auth(Auth::Public),
    Operation::new(Method::Post, "/auth/logout", TAG, "Logout"),
    Operation::new(
        Method::Get,
        "/auth/oidc/{provider_id}/callback",
        TAG,
        "Callback",
    )
    .auth(Auth::Public),
    Operation::new(Method::Get, "/auth/oidc/{provider_id}/start", TAG, "Start").auth(Auth::Public),
    Operation::new(
        Method::Post,
        "/auth/passkeys/login/finish",
        TAG,
        "Finish passkey login",
    )
    .auth(Auth::Public),
    Operation::new(
        Method::Post,
        "/auth/passkeys/login/start",
        TAG,
        "Start passkey login",
    )
    .auth(Auth::Public),
    Operation::new(
        Method::Post,
        "/auth/password-reset",
        TAG,
        "Request password reset",
    )
    .auth(Auth::Public)
    .accepted(),
    Operation::new(
        Method::Post,
        "/auth/password-reset/confirm",
        TAG,
        "Confirm password reset",
    )
    .auth(Auth::Public),
    Operation::new(Method::Get, "/auth/status", TAG, "Status").auth(Auth::Public),
    Operation::new(Method::Post, "/register", TAG, "Register")
        .auth(Auth::Public)
        .created(),
];
