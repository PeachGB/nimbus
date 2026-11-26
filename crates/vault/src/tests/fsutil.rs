use super::*;
use tempfile::tempdir;

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn atomic_write_creates_file_and_parents() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("nested/deeper/file.toml");
    atomic_write(&path, b"hello", None).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
}

#[test]
fn atomic_write_replaces_existing_contents_and_leaves_no_temp() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file.toml");
    std::fs::write(&path, b"a much longer original body").unwrap();
    atomic_write(&path, b"new", None).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"new");
    assert_eq!(entries(dir.path()), vec!["file.toml".to_string()]);
}

#[cfg(unix)]
#[test]
fn atomic_write_applies_exact_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let path = dir.path().join("secret.toml");
    std::fs::write(&path, b"old").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    atomic_write(&path, b"token = \"x\"", Some(0o600)).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(unix)]
#[test]
fn atomic_write_keeps_existing_mode_by_default() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let path = dir.path().join("file");
    std::fs::write(&path, b"old").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

    atomic_write(&path, b"new", None).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
}

#[test]
fn atomic_write_failure_keeps_original_and_cleans_up() {
    let dir = tempdir().unwrap();
    // Renaming a file over a non-empty directory fails, after the temp file was written.
    let path = dir.path().join("target");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("inside"), b"keep").unwrap();

    assert!(atomic_write(&path, b"new", None).is_err());
    assert_eq!(std::fs::read(path.join("inside")).unwrap(), b"keep");
    assert_eq!(entries(dir.path()), vec!["target".to_string()]);
}

#[test]
fn temp_sibling_is_hidden_unique_and_in_the_same_directory() {
    let a = temp_sibling(Path::new("/x/y/file.txt")).unwrap();
    let b = temp_sibling(Path::new("/x/y/file.txt")).unwrap();
    assert_ne!(a, b);
    assert_eq!(a.parent(), Some(Path::new("/x/y")));
    assert!(
        a.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(TEMP_PREFIX)
    );
}

#[test]
fn read_secret_drops_trailing_newlines_only() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("token");
    for (written, read) in [
        ("s3cr3t\r\n", "s3cr3t"),
        ("s3cr3t\n\n", "s3cr3t"),
        ("  s3 cr3t\n", "  s3 cr3t"),
    ] {
        std::fs::write(&path, written).unwrap();
        assert_eq!(read_secret(&path).unwrap(), read, "{written:?}");
    }
}

#[test]
fn read_secret_refuses_an_empty_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("token");
    for written in ["", "\n"] {
        std::fs::write(&path, written).unwrap();
        let error = read_secret(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{written:?}");
    }
}

#[test]
fn read_secret_reports_a_missing_file() {
    let dir = tempdir().unwrap();
    let error = read_secret(&dir.path().join("missing")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn resolve_against_leaves_an_absolute_path_untouched() {
    let resolved = resolve_against(Path::new("/etc/nimbus"), PathBuf::from("/srv/data"));
    assert_eq!(resolved, PathBuf::from("/srv/data"));
}

#[test]
fn resolve_against_joins_a_relative_path_onto_the_dir() {
    let resolved = resolve_against(Path::new("/etc/nimbus"), PathBuf::from("../data"));
    assert_eq!(resolved, PathBuf::from("/etc/nimbus/../data"));
}

#[test]
fn config_dir_of_a_bare_file_name_is_the_cwd() {
    let dir = config_dir(Path::new("x.toml")).unwrap();
    assert_eq!(dir, std::env::current_dir().unwrap());
}

#[test]
fn config_dir_of_an_absolute_path_is_its_parent() {
    let dir = config_dir(Path::new("/etc/nimbus/x.toml")).unwrap();
    assert_eq!(dir, PathBuf::from("/etc/nimbus"));
}

#[test]
fn read_secret_zeroizing_trims_like_read_secret() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("key");
    for (written, read) in [
        ("k3y\r\n", "k3y"),
        ("k3y\n\n", "k3y"),
        ("  k 3y\n", "  k 3y"),
    ] {
        std::fs::write(&path, written).unwrap();
        assert_eq!(
            read_secret_zeroizing(&path).unwrap().as_str(),
            read,
            "{written:?}"
        );
    }
}

#[test]
fn read_secret_zeroizing_refuses_empty_and_non_utf8_files() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("key");
    for written in [&b""[..], b"\n\r\n", b"\xff\xfe secret"] {
        std::fs::write(&path, written).unwrap();
        let error = read_secret_zeroizing(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{written:?}");
        assert!(!error.to_string().contains("secret"), "{error}");
    }
}
