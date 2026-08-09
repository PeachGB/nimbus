use super::*;

fn write_vault_config(dir: &Path, file_stem: &str, name: &str) -> std::path::PathBuf {
    let root = dir.join(format!("{file_stem}-data"));
    std::fs::create_dir_all(&root).unwrap();
    let path = dir.join(format!("{file_stem}.toml"));
    std::fs::write(
        &path,
        format!(
            "name = \"{name}\"\n\n[origin_config]\ntype = \"fs\"\nroot = \"{}\"\n",
            root.display()
        ),
    )
    .unwrap();
    path
}

#[test]
fn vaults_are_keyed_by_the_name_inside_the_config_not_the_file_name() {
    let dir = tempfile::tempdir().unwrap();
    write_vault_config(dir.path(), "some-file", "photos");

    let vaults = load_vaults(dir.path()).unwrap();

    assert!(vaults.contains_key("photos"));
    assert!(!vaults.contains_key("some-file"));
}

#[test]
fn files_that_are_not_toml_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    write_vault_config(dir.path(), "photos", "photos");
    std::fs::write(dir.path().join("README.md"), "not a vault").unwrap();
    std::fs::write(dir.path().join("notes"), "not a vault either").unwrap();

    let vaults = load_vaults(dir.path()).unwrap();

    assert_eq!(vaults.len(), 1);
}

#[test]
fn a_broken_config_is_skipped_so_the_rest_still_get_served() {
    let dir = tempfile::tempdir().unwrap();
    write_vault_config(dir.path(), "good", "good");
    std::fs::write(dir.path().join("broken.toml"), "this is not a vault config").unwrap();

    let vaults = load_vaults(dir.path()).unwrap();

    assert_eq!(vaults.keys().collect::<Vec<_>>(), vec!["good"]);
}

#[test]
fn two_configs_claiming_one_name_is_fatal() {
    let dir = tempfile::tempdir().unwrap();
    write_vault_config(dir.path(), "first", "photos");
    write_vault_config(dir.path(), "second", "photos");

    let error = load_vaults(dir.path()).unwrap_err();

    assert!(error.to_string().contains("photos"), "{error}");
}

#[test]
fn a_missing_vaults_directory_is_an_error_naming_the_path() {
    let error = load_vaults(Path::new("/nonexistent/vaults")).unwrap_err();
    assert!(error.to_string().contains("/nonexistent/vaults"), "{error}");
}

#[test]
fn an_empty_directory_serves_no_vaults() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_vaults(dir.path()).unwrap().is_empty());
}

#[test]
fn writes_are_refused_only_when_read_only() {
    let open = AppState::from_parts(HashMap::new(), false, AuthConfig::None);
    assert!(open.ensure_writable().is_ok());

    let locked = AppState::from_parts(HashMap::new(), true, AuthConfig::None);
    assert!(matches!(locked.ensure_writable(), Err(ApiError::ReadOnly)));
}

#[test]
fn an_unknown_vault_name_is_reported_with_the_name() {
    let state = AppState::from_parts(HashMap::new(), false, AuthConfig::None);
    match state.vault("nope") {
        Err(ApiError::UnknownVault(name)) => assert_eq!(name, "nope"),
        other => panic!("expected UnknownVault, got {other:?}"),
    }
}
