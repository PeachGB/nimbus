use super::*;
use crate::crypt::encrypt_bytes;

/// Every name in `dir`, temp files included.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// --- key files ---

#[test]
fn a_new_key_file_is_private_and_loads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys/vault.key"); // parent created on the way
    let key = create_key_file(&path).unwrap();

    let loaded = CryptKey::load(Some(&path), None).unwrap();
    assert_eq!(loaded.bytes_for_test(), key.bytes_for_test());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("nimbus-crypt-key-1:") && text.ends_with('\n'));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert_eq!(names_in(&dir.path().join("keys")), vec!["vault.key"]);
}

#[test]
fn a_key_file_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.key");
    std::fs::write(&path, "an existing key\n").unwrap();

    assert!(create_key_file(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "an existing key\n");
    assert_eq!(names_in(dir.path()), vec!["vault.key"]);
}

#[test]
fn load_needs_exactly_one_source_and_names_it() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.key");
    let error = CryptKey::load(Some(&missing), None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing.key"), "{error}");
    let error = CryptKey::load(None, None).unwrap_err().to_string();
    assert!(error.contains("set one of"), "{error}");
    let error = CryptKey::load(Some(&missing), Some("X"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("only one"), "{error}");
    let error = CryptKey::load(None, Some("NIMBUS_TEST_UNSET_KEY_VAR"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("NIMBUS_TEST_UNSET_KEY_VAR"), "{error}");
}

// --- decrypting a file ---

/// `plaintext` sealed under `key` into `dir/name`, returning the path.
fn sealed_file(dir: &Path, name: &str, key: &CryptKey, plaintext: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, encrypt_bytes(key, plaintext).unwrap()).unwrap();
    path
}

fn plaintext() -> Vec<u8> {
    (0..150_000u32).map(|i| (i % 253) as u8).collect()
}

#[tokio::test]
async fn a_file_decrypts_to_a_private_output() {
    let dir = tempfile::tempdir().unwrap();
    let key = CryptKey::generate().unwrap();
    let input = sealed_file(dir.path(), "notes.txt", &key, &plaintext());
    let output = dir.path().join("out/notes.txt");

    let size = decrypt_file(&key, &input, &output, false).await.unwrap();

    assert_eq!(size, plaintext().len() as u64);
    assert_eq!(std::fs::read(&output).unwrap(), plaintext());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&output).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert_eq!(names_in(&dir.path().join("out")), vec!["notes.txt"]);
}

#[tokio::test]
async fn a_damaged_or_foreign_input_leaves_no_output_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let key = CryptKey::generate().unwrap();
    let good = std::fs::read(sealed_file(dir.path(), "good", &key, &plaintext())).unwrap();
    let mut flipped = good.clone();
    let at = flipped.len() - 30; // in the footer: every data chunk decrypts before it fails
    flipped[at] ^= 1;
    let cases: Vec<(&str, Vec<u8>, CryptKey)> = vec![
        ("tampered", flipped, key.clone()),
        ("truncated", good[..good.len() - 24].to_vec(), key.clone()),
        ("wrong key", good.clone(), CryptKey::generate().unwrap()),
        ("plaintext", b"just a plain file".to_vec(), key.clone()),
    ];
    for (case, bytes, with_key) in cases {
        let work = tempfile::tempdir().unwrap();
        let input = work.path().join("in");
        std::fs::write(&input, bytes).unwrap();
        let output = work.path().join("out");

        let error = decrypt_file(&with_key, &input, &output, false).await;
        assert!(error.is_err(), "{case} decrypted");
        assert_eq!(
            names_in(work.path()),
            vec!["in"],
            "{case}: output or temp left behind"
        );
    }
}

#[tokio::test]
async fn the_wrong_key_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let input = sealed_file(dir.path(), "in", &CryptKey::generate().unwrap(), b"x");
    let error = decrypt_file(
        &CryptKey::generate().unwrap(),
        &input,
        &dir.path().join("out"),
        false,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, VaultError::Crypt(crate::crypt::CryptError::WrongKey)),
        "{error:?}"
    );
}

#[tokio::test]
async fn an_existing_output_is_refused_unless_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let key = CryptKey::generate().unwrap();
    let input = sealed_file(dir.path(), "in", &key, b"new contents");
    let output = dir.path().join("out");
    std::fs::write(&output, b"keep me").unwrap();

    let error = decrypt_file(&key, &input, &output, false)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, VaultError::Io(e) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "{error:?}"
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"keep me");
    assert_eq!(names_in(dir.path()), vec!["in", "out"]);

    decrypt_file(&key, &input, &output, true).await.unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), b"new contents");
}

#[tokio::test]
async fn a_failed_overwrite_keeps_the_old_output() {
    let dir = tempfile::tempdir().unwrap();
    let key = CryptKey::generate().unwrap();
    let input = dir.path().join("in");
    std::fs::write(&input, b"not encrypted").unwrap();
    let output = dir.path().join("out");
    std::fs::write(&output, b"keep me").unwrap();

    assert!(decrypt_file(&key, &input, &output, true).await.is_err());
    assert_eq!(std::fs::read(&output).unwrap(), b"keep me");
    assert_eq!(names_in(dir.path()), vec!["in", "out"]);
}

// --- temp names that are taken, and filesystems without hard links ---

#[test]
fn a_temp_name_that_is_already_taken_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let planted = dir.path().join(".nimbus-tmp.planted");
    std::fs::write(&planted, b"not ours").unwrap();
    let fresh = dir.path().join(".nimbus-tmp.fresh");
    let mut names = vec![fresh.clone(), planted.clone()];

    let temp = PrivateTemp::create_with(|| Ok(names.pop().unwrap())).unwrap();
    assert_eq!(temp.path, fresh);
    drop(temp); // never placed: removes only its own file

    assert_eq!(std::fs::read(&planted).unwrap(), b"not ours");
    assert!(!fresh.exists());
}

#[test]
fn every_temp_name_taken_is_a_temp_error_not_the_destinations() {
    let dir = tempfile::tempdir().unwrap();
    let planted = dir.path().join(".nimbus-tmp.planted");
    std::fs::write(&planted, b"not ours").unwrap();

    let error = PrivateTemp::create_with(|| Ok(planted.clone())).unwrap_err();
    assert!(error.to_string().contains("temp file"), "{error}");
    assert_eq!(std::fs::read(&planted).unwrap(), b"not ours");
}

/// A `link` for a filesystem without hard links.
fn no_hard_links(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

fn written_temp(dir: &Path, contents: &[u8]) -> PrivateTemp {
    use std::io::Write;
    let temp = PrivateTemp::create(&dir.join("destination")).unwrap();
    temp.file().unwrap().write_all(contents).unwrap();
    temp
}

#[test]
fn without_hard_links_the_file_is_copied_into_a_new_private_file() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("destination");
    written_temp(dir.path(), b"key text\n")
        .place(&destination, false, no_hard_links)
        .unwrap();

    assert_eq!(std::fs::read(&destination).unwrap(), b"key text\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert_eq!(names_in(dir.path()), vec!["destination"]);
}

#[test]
fn without_hard_links_an_existing_file_is_still_never_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("destination");
    std::fs::write(&destination, b"existing").unwrap();

    let error = written_temp(dir.path(), b"new")
        .place(&destination, false, no_hard_links)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
    assert_eq!(names_in(dir.path()), vec!["destination"]);
}

#[cfg(unix)]
#[test]
fn a_temp_swapped_for_a_symlink_is_never_written_through() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let victim = dir.path().join("someone-elses-file");
    std::fs::write(&victim, b"").unwrap();
    let temp = PrivateTemp::create(&dir.path().join("destination")).unwrap();

    // Between creating the temp and writing it, its name is swapped for a symlink.
    std::fs::remove_file(&temp.path).unwrap();
    std::os::unix::fs::symlink(&victim, &temp.path).unwrap();
    temp.file().unwrap().write_all(b"KEY TEXT").unwrap();

    assert_eq!(
        std::fs::read(&victim).unwrap(),
        b"",
        "written through the symlink"
    );
}

/// A reader that hands out a few bytes, then fails, like a disk error midway.
struct FailsMidway(usize);

impl std::io::Read for FailsMidway {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.0 == 0 {
            return Err(std::io::Error::other("disk error"));
        }
        let n = self.0.min(buf.len());
        buf[..n].fill(b'k');
        self.0 -= n;
        Ok(n)
    }
}

#[test]
fn a_copy_that_fails_midway_leaves_no_partial_destination() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("destination");
    assert!(copy_new(&mut FailsMidway(10), &destination).is_err());
    assert!(!destination.exists());
}

#[test]
fn a_given_key_is_written_private_and_never_over_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("given.key");
    let key = CryptKey::generate().unwrap();

    write_key_file(&path, &key).unwrap();
    let loaded = CryptKey::load(Some(&path), None).unwrap();
    assert_eq!(loaded.bytes_for_test(), key.bytes_for_test());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let error = write_key_file(&path, &CryptKey::generate().unwrap()).unwrap_err();
    assert!(
        matches!(&error, VaultError::Io(e) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "{error:?}"
    );
    assert_eq!(
        CryptKey::load(Some(&path), None).unwrap().bytes_for_test(),
        key.bytes_for_test()
    );
}

/// Replaces the temp name with someone else's file: a plain file of theirs, or a symlink to it.
#[cfg(unix)]
fn swap_temp(temp_path: &Path, attacker: &Path, as_symlink: bool) {
    std::fs::remove_file(temp_path).unwrap();
    if as_symlink {
        std::os::unix::fs::symlink(attacker, temp_path).unwrap();
    } else {
        std::fs::copy(attacker, temp_path).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn a_temp_replaced_before_placing_is_never_placed() {
    use std::io::Write;
    for as_symlink in [false, true] {
        for replace in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let attacker = dir.path().join("attacker");
            std::fs::write(&attacker, b"ATTACKER CONTENT").unwrap();
            let destination = dir.path().join("destination");
            let temp = written_temp(dir.path(), b"ours");
            temp.file().unwrap().flush().unwrap();
            swap_temp(&temp.path.clone(), &attacker, as_symlink);

            let case = format!("symlink={as_symlink} replace={replace}");
            let error = temp.place(&destination, replace, hard_link).unwrap_err();
            assert!(
                error.to_string().contains("replaced while writing"),
                "{case}: {error}"
            );
            assert!(
                std::fs::symlink_metadata(&destination).is_err(),
                "{case}: destination left"
            );
            assert_eq!(
                std::fs::read(&attacker).unwrap(),
                b"ATTACKER CONTENT",
                "{case}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn a_key_file_whose_temp_is_replaced_is_an_error_never_a_key() {
    for as_symlink in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let attacker = dir.path().join("attacker.key");
        let known = CryptKey::generate().unwrap();
        std::fs::write(&attacker, format!("{}\n", known.to_text().as_str())).unwrap();
        let path = dir.path().join("vault.key");

        let result = write_key_file_with(&path, &CryptKey::generate().unwrap(), |temp| {
            swap_temp(temp, &attacker, as_symlink)
        });

        assert!(
            result.is_err(),
            "symlink={as_symlink}: accepted a key the attacker knows"
        );
        assert!(
            std::fs::symlink_metadata(&path).is_err(),
            "symlink={as_symlink}"
        );
    }
}
