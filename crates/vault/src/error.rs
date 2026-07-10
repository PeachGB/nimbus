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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_formatted_as_expected() {
        assert_eq!(
            VaultError::Generic("boom".to_string()).to_string(),
            "Vault error: boom"
        );
        assert_eq!(
            VaultError::NotFound("obj1".to_string()).to_string(),
            "Object not found: obj1"
        );
        assert_eq!(
            VaultError::InvalidMethodCall.to_string(),
            "Invalid method call"
        );
        assert_eq!(
            VaultError::FetchToVaultWithNoOrigin("v1".to_string()).to_string(),
            "Called fetch on vault v1, but v1 has no Origin defined"
        );
        assert_eq!(
            VaultError::AlreadyExists.to_string(),
            "Vault already exists"
        );
        assert_eq!(
            VaultError::Unreachable.to_string(),
            "Unreachable pattern reached"
        );
        assert_eq!(VaultError::Locked.to_string(), "Vault is locked");
        assert_eq!(VaultError::Unlocked.to_string(), "Vault is unlocked");
        assert_eq!(
            VaultError::OriginError("bad".to_string()).to_string(),
            "Origin Error: bad"
        );
    }

    #[test]
    fn io_error_converts_via_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let vault_err: VaultError = io_err.into();
        assert!(matches!(vault_err, VaultError::Io(_)));
        assert!(vault_err.to_string().starts_with("io error:"));
    }

    #[test]
    fn json_error_converts_via_from() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let vault_err: VaultError = json_err.into();
        assert!(matches!(vault_err, VaultError::Json(_)));
        assert!(vault_err.to_string().starts_with("json error:"));
    }

    #[test]
    fn toml_error_converts_via_from() {
        let toml_err = toml::from_str::<toml::Value>("not = = valid").unwrap_err();
        let vault_err: VaultError = toml_err.into();
        assert!(matches!(vault_err, VaultError::Toml(_)));
        assert!(vault_err.to_string().starts_with("Toml Error:"));
    }

    #[test]
    fn http_error_converts_via_from() {
        let reqwest_err = reqwest::Client::new()
            .get("not a valid url")
            .build()
            .unwrap_err();
        let vault_err: VaultError = reqwest_err.into();
        assert!(matches!(vault_err, VaultError::HTTP(_)));
        assert!(vault_err.to_string().starts_with("HTTP Error"));
    }
}
