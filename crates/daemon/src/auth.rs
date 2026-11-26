use anyhow::{Context, Result, anyhow};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use nimbus_vault::secret::Secret;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::{error::ErrorBody, state::AppState};

/// The `[auth]` table as written in the config file. [`AuthConfig::resolve`] turns it into the
/// [`Auth`] the server checks requests against.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    /// Every request is served as the anonymous identity.
    #[default]
    None,
    /// A bearer token: exactly one of `token` (inline) or `token_file`.
    Bearer {
        /// The token, written out in the config.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<Secret>,
        /// Path to a file holding only the token; a trailing newline is not part of it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_file: Option<PathBuf>,
    },
}

impl AuthConfig {
    /// Reads the secret. Called once at startup; the token is not re-read on SIGHUP.
    /// Both fields or neither, an unreadable token_file, and an empty token are errors, and
    /// any error about the file names it.
    pub fn resolve(&self) -> Result<Auth> {
        match self {
            AuthConfig::None => Ok(Auth::None),
            AuthConfig::Bearer { token, token_file } => {
                let token = bearer_secret(token.as_ref(), token_file.as_deref())?;
                Ok(Auth::Bearer { token })
            }
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            AuthConfig::None => "none",
            AuthConfig::Bearer {
                token_file: Some(_),
                ..
            } => "bearer token file",
            AuthConfig::Bearer { .. } => "bearer token",
        }
    }

    /// Resolves a relative `token_file` against `dir`, the config file's directory. An inline
    /// token has no path, so it is untouched.
    pub fn relative_to(self, dir: &Path) -> Self {
        match self {
            AuthConfig::Bearer { token, token_file } => AuthConfig::Bearer {
                token,
                token_file: token_file.map(|path| nimbus_vault::fsutil::resolve_against(dir, path)),
            },
            none => none,
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self, AuthConfig::None)
    }

    /// Whether the table holds a secret inline, so a config file containing it is written 0600.
    pub fn holds_secret(&self) -> bool {
        matches!(self, AuthConfig::Bearer { token: Some(_), .. })
    }
}

/// Reads the bearer token from whichever of the two sources was configured, insisting that it
/// be exactly one: silently preferring one would let a stale inline `token` override the
/// `token_file` that replaced it.
fn bearer_secret(token: Option<&Secret>, token_file: Option<&Path>) -> Result<Secret> {
    let secret = match (token, token_file) {
        (Some(token), None) => token.clone(),
        (None, Some(path)) => read_token_file(path)?,
        (None, None) => return Err(anyhow!("auth: set one of token or token_file")),
        (Some(_), Some(_)) => return Err(anyhow!("auth: set only one of token or token_file")),
    };
    if secret.is_empty() {
        return Err(anyhow!("auth: the configured token is empty"));
    }
    Ok(secret)
}

/// Reads `path` as a token file (see [`nimbus_vault::fsutil::read_secret_zeroizing`]) straight
/// into a [`Secret`], with no unwiped copy along the way, naming the file in any error, an empty
/// one included.
fn read_token_file(path: &Path) -> Result<Secret> {
    nimbus_vault::fsutil::read_secret_zeroizing(path)
        .map(Secret::from)
        .with_context(|| format!("reading bearer token file {}", path.display()))
}

/// The authentication the running server enforces: an [`AuthConfig`] with its secret read.
#[derive(Debug, Clone, Default)]
pub enum Auth {
    /// Every request is served as the anonymous identity.
    #[default]
    None,
    /// Requests must carry `Authorization: Bearer <token>`.
    Bearer { token: Secret },
}

impl Auth {
    pub fn authenticate(&self, headers: &HeaderMap) -> Result<Identity, AuthError> {
        match self {
            Auth::None => Ok(Identity::anonymous()),
            Auth::Bearer { token } => {
                let presented = bearer_token(headers).ok_or(AuthError::Missing)?;
                if token.matches(presented.as_bytes()) {
                    Ok(Identity::new("bearer"))
                } else {
                    Err(AuthError::Invalid)
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
}

impl Identity {
    pub fn new(name: impl Into<String>) -> Self {
        Identity { name: name.into() }
    }
    pub fn anonymous() -> Self {
        Identity::new("anonymous")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    Missing,
    Invalid,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let message = match self {
            AuthError::Missing => "missing credentials",
            AuthError::Invalid => "invalid credentials",
        };
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            ErrorBody::json(message),
        )
            .into_response()
    }
}

pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let identity = state.auth.authenticate(request.headers())?;
    request.extensions_mut().insert(identity);
    Ok(next.run(request).await)
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim_start())
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
