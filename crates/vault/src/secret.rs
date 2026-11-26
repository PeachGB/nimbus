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
use zeroize::{Zeroize, Zeroizing};

/// A secret string, such as a bearer token, for config fields that hold credentials. Its
/// `Debug` never shows the value, so a struct holding one can derive `Debug` safely. It
/// serializes as the plain string, so config files are unchanged. There is deliberately no
/// `==`: compare with [`Secret::matches`]. Its memory is wiped when it is dropped (each clone's
/// too), and [`From<Zeroizing<String>>`](Secret#impl-From<Zeroizing<String>>-for-Secret) takes
/// a secret read into a wiped buffer without copying it. It can't wipe copies made before or
/// after it: the config file's text and `toml`'s buffers for an inline `token`, the copy
/// `std::env::var` returns and the process environment itself, and the header and `reqwest`
/// copies made to send it. That's one reason `token_file` and `token_env` are preferred over an
/// inline token.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// The secret itself, for the places that must send or use it, such as building the
    /// `Authorization` header. To check a credential, use [`Secret::matches`].
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether `presented` is this secret, compared in constant time so the response time
    /// doesn't reveal how many leading bytes matched. Use this, never `expose() ==`, to check a
    /// credential. The length still shows in the timing, as with any comparison of this kind;
    /// only where the first difference lies is hidden.
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

/// Moves the text out of a wiped buffer (as [`crate::fsutil::read_secret_zeroizing`] returns)
/// without copying it, so the only copy left is this secret's own, wiped on drop.
impl From<Zeroizing<String>> for Secret {
    fn from(mut secret: Zeroizing<String>) -> Self {
        Secret(std::mem::take(&mut *secret))
    }
}

/// Wipes the secret's memory, spare capacity included.
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
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
