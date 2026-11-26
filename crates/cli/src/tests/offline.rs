use super::*;

#[test]
fn keygen_writes_a_private_key_that_loads_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.key");

    keygen(&path).unwrap();
    assert!(CryptKey::load(Some(&path), None).is_ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    let before = std::fs::read(&path).unwrap();
    let error = keygen(&path).unwrap_err().to_string();
    assert!(error.contains("never replaces a key file"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn the_loss_warning_says_what_losing_the_key_means() {
    assert!(KEY_LOSS_WARNING.contains("losing the key loses the data"));
    assert!(KEY_LOSS_WARNING.contains("Back it up"));
}

#[tokio::test]
async fn decrypt_recovers_a_file_and_refuses_to_overwrite_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let key_path = dir.path().join("vault.key");
    keygen(&key_path).unwrap();
    let key = CryptKey::load(Some(&key_path), None).unwrap();
    let input = dir.path().join("notes.txt");
    std::fs::write(
        &input,
        nimbus_vault::crypt::encrypt_bytes(&key, b"recovered").unwrap(),
    )
    .unwrap();
    let output = dir.path().join("notes.plain.txt");

    decrypt(&input, &output, Some(&key_path), None, false)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), b"recovered");

    let error = decrypt(&input, &output, Some(&key_path), None, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("--force"), "{error}");
    decrypt(&input, &output, Some(&key_path), None, true)
        .await
        .unwrap();
}

#[tokio::test]
async fn decrypt_with_the_wrong_key_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (right, wrong) = (dir.path().join("right.key"), dir.path().join("wrong.key"));
    keygen(&right).unwrap();
    keygen(&wrong).unwrap();
    let key = CryptKey::load(Some(&right), None).unwrap();
    let input = dir.path().join("in");
    std::fs::write(
        &input,
        nimbus_vault::crypt::encrypt_bytes(&key, b"x").unwrap(),
    )
    .unwrap();
    let output = dir.path().join("out");

    let error = decrypt(&input, &output, Some(&wrong), None, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("different key"), "{error}");
    assert!(!output.exists());
}
