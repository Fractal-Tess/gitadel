//! Account.

use crate::{
    identity::{
        auth, avatar, commit_emails, email, notifications, oauth, resources, two_factor, users,
    },
    openapi::{Auth, Body, Method, Operation, json, json_list},
    repository::{gitea, releases},
};

const TAG: &str = "Account";

pub(super) const OPERATIONS: &[Operation] = &[
    Operation::new(
        Method::Get,
        "/audit",
        TAG,
        "List the instance audit history",
    )
    .response(json_list::<resources::AuditEventResponse>())
    .query(&[
        ("limit", "Events per page, 1 to 500; defaults to 100"),
        (
            "before",
            "Return events older than this event id; the stable way to page",
        ),
        (
            "offset",
            "Skip this many events; shifts as new events arrive",
        ),
    ]),
    Operation::new(Method::Get, "/invitations", TAG, "List pending invitations")
        .response(json_list::<users::PendingInvitationResponse>()),
    Operation::new(Method::Post, "/invitations", TAG, "Create an invitation")
        .request(json::<auth::CreateInvitationRequest>())
        .response(json::<auth::InvitationResponse>())
        .created(),
    Operation::new(
        Method::Delete,
        "/invitations/{id}",
        TAG,
        "Revoke an invitation",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Get,
        "/me/authorizations",
        TAG,
        "List authorized OAuth applications",
    )
    .response(json_list::<oauth::AuthorizationResponse>()),
    Operation::new(
        Method::Delete,
        "/me/authorizations/{client_id}",
        TAG,
        "Revoke an OAuth application's access",
    )
    .response(Body::Empty),
    Operation::new(Method::Put, "/me/avatar", TAG, "Upload an avatar")
        .request(json::<avatar::UpdateAvatarRequest>())
        .response(Body::Empty),
    Operation::new(Method::Delete, "/me/avatar", TAG, "Remove the avatar").response(Body::Empty),
    Operation::new(
        Method::Post,
        "/me/commit-emails",
        TAG,
        "Add a commit email address",
    )
    .request(json::<commit_emails::AddEmailRequest>())
    .response(json::<commit_emails::CommitIdentityResponse>())
    .created(),
    Operation::new(
        Method::Delete,
        "/me/commit-emails/{email}",
        TAG,
        "Remove a commit email address",
    )
    .response(json::<commit_emails::CommitIdentityResponse>()),
    Operation::new(
        Method::Post,
        "/me/commit-emails/{email}/verification",
        TAG,
        "Resend a commit email verification",
    )
    .request(Body::Empty)
    .response(Body::Empty)
    .accepted(),
    Operation::new(
        Method::Get,
        "/me/commit-identity",
        TAG,
        "Get the commit identity",
    )
    .response(json::<commit_emails::CommitIdentityResponse>()),
    Operation::new(
        Method::Put,
        "/me/commit-identity",
        TAG,
        "Update the commit identity",
    )
    .request(json::<commit_emails::UpdateIdentityRequest>())
    .response(json::<commit_emails::CommitIdentityResponse>()),
    Operation::new(
        Method::Get,
        "/me/email",
        TAG,
        "Get the account email address",
    )
    .response(json::<email::EmailStatusResponse>()),
    Operation::new(
        Method::Put,
        "/me/email",
        TAG,
        "Set the account email address",
    )
    .request(json::<email::SetEmailRequest>())
    .response(json::<email::EmailStatusResponse>()),
    Operation::new(
        Method::Delete,
        "/me/email",
        TAG,
        "Remove the account email address",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Post,
        "/me/email/verification",
        TAG,
        "Resend the account email verification",
    )
    .request(Body::Empty)
    .response(Body::Empty)
    .accepted(),
    Operation::new(
        Method::Put,
        "/me/motion-preference",
        TAG,
        "Update the motion preference",
    )
    .request(json::<auth::UpdateMotionPreferenceRequest>())
    .response(json::<auth::AuthResponse>()),
    Operation::new(
        Method::Get,
        "/me/notifications",
        TAG,
        "Get email notification preferences",
    )
    .response(json::<notifications::NotificationSettingsResponse>()),
    Operation::new(
        Method::Put,
        "/me/notifications",
        TAG,
        "Update email notification preferences",
    )
    .request(json::<notifications::NotificationPreferences>())
    .response(json::<notifications::NotificationSettingsResponse>()),
    Operation::new(
        Method::Get,
        "/me/oauth-applications",
        TAG,
        "List OAuth applications",
    )
    .response(json_list::<oauth::OauthApplicationResponse>()),
    Operation::new(
        Method::Post,
        "/me/oauth-applications",
        TAG,
        "Create an OAuth application",
    )
    .request(json::<oauth::CreateOauthApplicationRequest>())
    .response(json::<oauth::CreatedOauthApplicationResponse>())
    .created(),
    Operation::new(
        Method::Delete,
        "/me/oauth-applications/{id}",
        TAG,
        "Delete an OAuth application",
    )
    .response(Body::Empty),
    Operation::new(Method::Get, "/me/passkeys", TAG, "List passkeys")
        .response(json_list::<auth::PasskeyResponse>()),
    Operation::new(
        Method::Post,
        "/me/passkeys/register/finish",
        TAG,
        "Finish registering a passkey",
    )
    .request(json::<auth::FinishPasskeyRegistrationRequest>())
    .response(Body::Empty)
    .created(),
    Operation::new(
        Method::Post,
        "/me/passkeys/register/start",
        TAG,
        "Start registering a passkey",
    )
    .request(json::<auth::StartPasskeyRegistrationRequest>())
    .response(json::<auth::PasskeyCreationResponse>()),
    Operation::new(Method::Delete, "/me/passkeys/{id}", TAG, "Delete a passkey")
        .response(Body::Empty),
    Operation::new(Method::Put, "/me/password", TAG, "Change the password")
        .request(json::<auth::UpdatePasswordRequest>())
        .response(Body::Empty),
    Operation::new(
        Method::Put,
        "/me/repository-preferences",
        TAG,
        "Update repository preferences",
    )
    .request(json::<auth::UpdateRepositoryPreferencesRequest>())
    .response(json::<auth::AuthResponse>()),
    Operation::new(Method::Get, "/me/ssh-keys", TAG, "List SSH keys")
        .response(json_list::<resources::SshKeyResponse>()),
    Operation::new(Method::Post, "/me/ssh-keys", TAG, "Add an SSH key")
        .request(json::<resources::CreateSshKeyRequest>())
        .response(json::<resources::SshKeyResponse>())
        .created(),
    Operation::new(
        Method::Delete,
        "/me/ssh-keys/{id}",
        TAG,
        "Delete an SSH key",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Put,
        "/me/theme-preference",
        TAG,
        "Update the theme preference",
    )
    .request(json::<auth::UpdateThemePreferenceRequest>())
    .response(json::<auth::AuthResponse>()),
    Operation::new(Method::Get, "/me/tokens", TAG, "List API tokens")
        .response(json_list::<resources::TokenResponse>()),
    Operation::new(Method::Post, "/me/tokens", TAG, "Create an API token")
        .request(json::<resources::CreateTokenRequest>())
        .response(json::<resources::CreatedTokenResponse>())
        .created(),
    Operation::new(
        Method::Delete,
        "/me/tokens/{id}",
        TAG,
        "Revoke an API token",
    )
    .response(Body::Empty),
    Operation::new(
        Method::Get,
        "/me/two-factor",
        TAG,
        "Get two-factor authentication status",
    )
    .response(json::<two_factor::TwoFactorStatusResponse>()),
    Operation::new(
        Method::Post,
        "/me/two-factor/confirm",
        TAG,
        "Confirm two-factor enrollment",
    )
    .request(json::<two_factor::ConfirmEnrollmentRequest>())
    .response(json::<two_factor::RecoveryCodesResponse>())
    .created(),
    Operation::new(
        Method::Post,
        "/me/two-factor/disable",
        TAG,
        "Disable two-factor authentication",
    )
    .request(json::<two_factor::ReauthenticationRequest>())
    .response(Body::Empty),
    Operation::new(
        Method::Post,
        "/me/two-factor/enroll",
        TAG,
        "Start two-factor enrollment",
    )
    .request(Body::Empty)
    .response(json::<two_factor::EnrollmentResponse>()),
    Operation::new(
        Method::Post,
        "/me/two-factor/recovery-codes",
        TAG,
        "Regenerate recovery codes",
    )
    .request(json::<two_factor::ReauthenticationRequest>())
    .response(json::<two_factor::RecoveryCodesResponse>()),
    Operation::new(Method::Put, "/me/username", TAG, "Change the username")
        .request(json::<auth::UpdateUsernameRequest>())
        .response(json::<auth::AuthResponse>()),
    Operation::new(
        Method::Get,
        "/user",
        TAG,
        "Get the current user, Forgejo-compatible",
    )
    .response(json::<releases::ForgejoUserResponse>()),
    Operation::new(
        Method::Get,
        "/user/repos",
        TAG,
        "List the current user's repositories, Gitea-compatible",
    )
    .query(&[
        ("page", "Page number, starting at 1"),
        ("limit", "Repositories per page, 1 to 50; defaults to 30"),
    ])
    .response(json_list::<gitea::GiteaRepositoryResponse>()),
    Operation::new(
        Method::Get,
        "/users/{user_id}/avatar",
        TAG,
        "Download a user's avatar",
    )
    .auth(Auth::Public)
    .response(Body::Binary("image/png")),
];
