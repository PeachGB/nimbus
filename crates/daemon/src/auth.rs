use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::{error::ErrorBody, state::AppState};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    #[default]
    None,
    Bearer {
        token: String,
    },
}

impl AuthConfig {
    pub fn authenticate(&self, headers: &HeaderMap) -> Result<Identity, AuthError> {
        match self {
            AuthConfig::None => Ok(Identity::anonymous()),
            AuthConfig::Bearer { token } => {
                let presented = bearer_token(headers).ok_or(AuthError::Missing)?;
                if constant_time_eq(presented.as_bytes(), token.as_bytes()) {
                    Ok(Identity::new("bearer"))
                } else {
                    Err(AuthError::Invalid)
                }
            }
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            AuthConfig::None => "none",
            AuthConfig::Bearer { .. } => "bearer token",
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self, AuthConfig::None)
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

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
