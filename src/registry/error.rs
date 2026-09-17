use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use super::store::StoreError;

#[derive(Debug)]
pub(crate) enum RegistryError {
    Challenge(String, String),
    Status(StatusCode, &'static str, String),
    Store(StoreError),
}

impl RegistryError {
    pub(crate) fn challenge(header: impl Into<String>) -> Self {
        Self::Challenge(header.into(), "Authentication is required.".to_owned())
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::Status(StatusCode::BAD_REQUEST, "NAME_INVALID", message.into())
    }

    pub(crate) fn unauthorized(message: impl Into<String>) -> Self {
        Self::Status(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", message.into())
    }

    pub(crate) fn forbidden(message: impl Into<String>) -> Self {
        Self::Status(StatusCode::FORBIDDEN, "DENIED", message.into())
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::Status(StatusCode::NOT_FOUND, "NAME_UNKNOWN", message.into())
    }

    pub(crate) fn method_not_allowed(message: impl Into<String>) -> Self {
        Self::Status(
            StatusCode::METHOD_NOT_ALLOWED,
            "UNSUPPORTED",
            message.into(),
        )
    }

    pub(crate) fn range(message: impl Into<String>) -> Self {
        Self::Status(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "RANGE_INVALID",
            message.into(),
        )
    }

    pub(crate) fn internal() -> Self {
        Self::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            "UNKNOWN",
            "The registry request failed.".to_owned(),
        )
    }

    pub(crate) fn with_challenge(self, challenge: String) -> Self {
        match self {
            Self::Status(StatusCode::UNAUTHORIZED, _, message) => {
                Self::Challenge(challenge, message)
            }
            other => other,
        }
    }

    fn into_parts(self) -> (StatusCode, &'static str, String, Option<String>) {
        match self {
            Self::Challenge(challenge, message) => (
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                message,
                Some(challenge),
            ),
            Self::Status(status, code, message) => (status, code, message, None),
            Self::Store(error) => match error {
                StoreError::Invalid(message) => {
                    (StatusCode::BAD_REQUEST, "NAME_INVALID", message, None)
                }
                StoreError::UploadUnknown => (
                    StatusCode::NOT_FOUND,
                    "BLOB_UPLOAD_UNKNOWN",
                    "Upload does not exist.".to_owned(),
                    None,
                ),
                StoreError::RangeInvalid => (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "RANGE_INVALID",
                    "The upload range is invalid.".to_owned(),
                    None,
                ),
                StoreError::DigestInvalid => (
                    StatusCode::BAD_REQUEST,
                    "DIGEST_INVALID",
                    "The digest is invalid.".to_owned(),
                    None,
                ),
                StoreError::TooLarge => (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "SIZE_INVALID",
                    "The request exceeds the size limit.".to_owned(),
                    None,
                ),
                StoreError::ManifestBlobUnknown(digest) => (
                    StatusCode::BAD_REQUEST,
                    "MANIFEST_BLOB_UNKNOWN",
                    format!("Referenced content {digest} is unavailable or has a different size."),
                    None,
                ),
                StoreError::Referenced => (
                    StatusCode::CONFLICT,
                    "DENIED",
                    "The content is still referenced by a manifest.".to_owned(),
                    None,
                ),
                StoreError::Io(error) => {
                    tracing::error!(%error, "Registry storage operation failed");
                    Self::internal().into_parts()
                }
            },
        }
    }
}

impl From<StoreError> for RegistryError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

impl From<axum::http::Error> for RegistryError {
    fn from(_: axum::http::Error) -> Self {
        Self::internal()
    }
}

impl From<axum::http::header::InvalidHeaderValue> for RegistryError {
    fn from(_: axum::http::header::InvalidHeaderValue) -> Self {
        Self::internal()
    }
}

#[derive(Serialize)]
struct ErrorEnvelope {
    errors: Vec<ErrorBody>,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

impl IntoResponse for RegistryError {
    fn into_response(self) -> Response {
        let (status, code, message, challenge) = self.into_parts();
        let mut response = (
            status,
            Json(ErrorEnvelope {
                errors: vec![ErrorBody { code, message }],
            }),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response.headers_mut().insert(
            "docker-distribution-api-version",
            HeaderValue::from_static("registry/2.0"),
        );
        if let Some(value) = challenge.and_then(|value| HeaderValue::from_str(&value).ok()) {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, value);
        }
        response
    }
}
