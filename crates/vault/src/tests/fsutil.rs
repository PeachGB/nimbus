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
