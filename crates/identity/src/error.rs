//! Transport-independent identity failures.
#[derive(Debug)]
pub enum IdentityError {
    BadRequest(String),
    Unauthorized,
    InvalidCredentials,
    InvalidTwoFactorCode,
    RateLimited(String),
    NotFound,
    Forbidden(String),
    Conflict(String),
    Internal(String),
    SecretOpen(gitadel_secrets::OpenError),
}

impl IdentityError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }
    pub fn unauthorized() -> Self {
        Self::Unauthorized
    }
    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self::RateLimited(message.into())
    }
    pub fn not_found() -> Self {
        Self::NotFound
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden(message.into())
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }
    pub fn internal(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(message)
            | Self::Forbidden(message)
            | Self::Conflict(message)
            | Self::Internal(message) => f.write_str(message),
            Self::RateLimited(message) => f.write_str(message),
            Self::InvalidCredentials => f.write_str("The username or credential was not accepted."),
            Self::InvalidTwoFactorCode => f.write_str("The authentication code was not accepted."),
            Self::Unauthorized => f.write_str("Sign in to continue."),
            Self::NotFound => f.write_str("Not found."),
            Self::SecretOpen(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for IdentityError {}
impl From<sea_orm::DbErr> for IdentityError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::internal(error)
    }
}

impl From<gitadel_secrets::OpenError> for IdentityError {
    fn from(error: gitadel_secrets::OpenError) -> Self {
        Self::SecretOpen(error)
    }
}
