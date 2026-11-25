use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use nimbus_vault::error::VaultError;
use serde::{Deserialize, Serialize};
use tracing::error;

#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorBody {
    pub error: String,
}

impl ErrorBody {
    pub fn json(message: impl Into<String>) -> Json<ErrorBody> {
        Json(ErrorBody {
            error: message.into(),
        })
    }
}

#[derive(Debug)]
pub enum ApiError {
    UnknownVault(String),
    InvalidId(String),
    /// Anything else malformed in the request: a name that isn't one path component, an
    /// unparseable header. Answered with 400 and the reason.
    BadRequest(String),
    ReadOnly,
    Vault(VaultError),
}

impl From<VaultError> for ApiError {
    fn from(error: VaultError) -> Self {
        ApiError::Vault(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::UnknownVault(name) => {
                (StatusCode::NOT_FOUND, format!("no vault named '{name}'"))
            }
            ApiError::InvalidId(reason) | ApiError::BadRequest(reason) => {
                (StatusCode::BAD_REQUEST, reason)
            }
            ApiError::ReadOnly => (
                StatusCode::FORBIDDEN,
                "this daemon is serving read-only".to_string(),
            ),
            // `NotFound` is part of the protocol, not a leak: `OriginHTTP` turns this status
            // back into `VaultError::NotFound`, which is what `push`/`pull` match on.
            ApiError::Vault(VaultError::NotFound(what)) => (StatusCode::NOT_FOUND, what),
            ApiError::Vault(e @ VaultError::Unsupported(_)) => {
                (StatusCode::NOT_IMPLEMENTED, e.to_string())
            }
            // The request itself was bad (a hostile or malformed name/id), not the origin.
            ApiError::Vault(e @ (VaultError::InvalidName(_) | VaultError::InvalidId(_))) => {
                (StatusCode::BAD_REQUEST, e.to_string())
            }
            // Anything else is the origin's own message, which can name a path on this host or
            // the upstream a proxying daemon sits in front of. The operator gets the detail in
            // the log; the client gets the status.
            ApiError::Vault(error) => {
                error!("origin error: {error}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "the vault's origin failed — see the daemon's log".to_string(),
                )
            }
        };
        (status, ErrorBody::json(message)).into_response()
    }
}
