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
