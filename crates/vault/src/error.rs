use thiserror::Error;

/// The error type used across this crate; see [`crate::VaultResult`].
///
/// # Examples
///
/// The most common variant to match on is `NotFound`, e.g. to distinguish "doesn't exist yet"
/// from a hard failure when deciding whether an object needs to be created (this is exactly
/// what `Vault::pull`/`Vault::push` do internally):
///
/// ```
/// use nimbus_vault::error::VaultError;
///
/// fn needs_create(result: Result<(), VaultError>) -> bool {
///     matches!(result, Err(VaultError::NotFound(_)))
/// }
///
/// assert!(needs_create(Err(VaultError::NotFound("notes.txt".to_string()))));
/// assert!(!needs_create(Ok(())));
/// ```
///
/// Every variant implements `Display` via `thiserror`, so `?`/`.to_string()` produce a
/// readable message out of the box:
///
/// ```
/// use nimbus_vault::error::VaultError;
///
/// let err = VaultError::NotFound("notes.txt".to_string());
/// assert_eq!(err.to_string(), "Object not found: notes.txt");
/// ```
#[derive(Error, Debug)]
pub enum VaultError {
    /// A catch-all error for conditions that don't fit a more specific variant.
    #[error("Vault error: {0}")]
    Generic(String),
    /// The requested object/id/url/name doesn't exist at the origin.
    #[error("Object not found: {0}")]
    NotFound(String),
    /// A method was called on an `Object`/`Vault` in a state that doesn't support it (e.g.
    /// pushing children onto a `Leaf`).
    #[error("Invalid method call")]
    InvalidMethodCall,
    /// `fetch` was called on a vault that has no `Origin` configured.
    #[error("Called fetch on vault {0}, but {0} has no Origin defined")]
    FetchToVaultWithNoOrigin(String),
    /// The vault being created already exists.
    #[error("Vault already exists")]
    AlreadyExists,
    /// A code path that should be statically unreachable was reached.
    #[error("Unreachable pattern reached")]
    Unreachable,
    /// The vault is locked and can't be operated on.
    #[error("Vault is locked")]
    Locked,
    /// The vault is unlocked when a locked state was expected.
    #[error("Vault is unlocked")]
    Unlocked,
    /// An object name that can't be used as a single path component (see
    /// [`crate::object::validate_name`]). Names come from the other side of a transfer, so this
    /// is what stops a remote from naming its way out of the destination directory.
    #[error(
        "Invalid object name {0:?}: a name can't be empty, '.' or '..', or contain '/', '\\' or NUL"
    )]
    InvalidName(String),
    /// An object id that resolves outside the vault it was given to — `..` segments, an
    /// absolute path, a NUL byte, or a symlink pointing out of the root.
    #[error("Invalid object id {0:?}: it escapes the vault")]
    InvalidId(String),
    /// The origin can't do this operation natively (e.g. [`crate::origin::Origin::rename`]).
    /// Callers that have a fallback, like copy-then-delete for a move, should use it.
    #[error("Unsupported by this origin: {0}")]
    Unsupported(String),
    /// An origin-specific operation failed; the string carries the origin's own error detail.
    #[error("Origin Error: {0}")]
    OriginError(String),
    /// Wraps a `std::io::Error` (e.g. reading a config file, touching the filesystem origin).
    #[error("io error:{0}")]
    Io(#[from] std::io::Error),
    /// Wraps a `serde_json::Error` (e.g. deserializing a command/HTTP origin's JSON response).
    #[error("json error:{0}")]
    Json(#[from] serde_json::Error),
    /// Wraps a `reqwest::Error` from the HTTP origin.
    #[error("HTTP Error{0}")]
    HTTP(#[from] reqwest::Error),
    /// Wraps a `toml::de::Error` from parsing a `VaultConfig`/`OriginConfig` TOML file.
    #[error("Toml Error:{0}")]
    Toml(#[from] toml::de::Error),
    /// Wraps a `toml::ser::Error` from serializing a `VaultConfig` (e.g. `VaultConfig::save`).
    #[error("Toml Error:{0}")]
    TomlSer(#[from] toml::ser::Error),
}

#[cfg(test)]
#[path = "tests/error.rs"]
mod tests;
