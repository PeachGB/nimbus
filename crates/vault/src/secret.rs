//! [`Secret`](crate::secret::Secret): a string that `Debug` never prints, so a token can sit in any config struct that
//! derives `Debug` without reaching a log line or a panic message.
//!
//! # Examples
//!
//! ```
//! use nimbus_vault::secret::Secret;
//!
//! let token = Secret::from("s3cr3t");
//! assert_eq!(format!("{token:?}"), "<redacted>");
//! assert_eq!(token.expose(), "s3cr3t");
//! ```

use serde::{Deserialize, Serialize};

/// A secret string, such as a bearer token, for config fields that hold credentials. Its
/// `Debug` never shows the value, so a struct holding one can derive `Debug` safely. It
/// serializes as the plain string, so config files are unchanged. There is deliberately no
/// `==`: compare with [`Secret::matches`].
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// The secret itself, for the one place that needs it: comparing a presented token.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether `presented` is this secret, compared in constant time so the response time
    /// doesn't reveal how many leading bytes matched. Use this, never `expose() ==`, to check a
    /// credential.
    pub fn matches(&self, presented: &[u8]) -> bool {
        constant_time_eq(self.0.as_bytes(), presented)
    }

    /// Whether the secret is the empty string, which no credential should be.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for Secret {
    fn from(secret: String) -> Self {
        Secret(secret)
    }
}

impl From<&str> for Secret {
    fn from(secret: &str) -> Self {
        Secret(secret.to_string())
    }
}

/// Lets clap parse `--token` straight into a `Secret`.
impl std::str::FromStr for Secret {
    type Err = std::convert::Infallible;

    fn from_str(secret: &str) -> Result<Self, Self::Err> {
        Ok(Secret::from(secret))
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
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
#[path = "tests/secret.rs"]
mod tests;
