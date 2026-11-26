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
    let open = AppState::from_parts(HashMap::new(), false, Auth::None);
    assert!(open.ensure_writable().is_ok());

    let locked = AppState::from_parts(HashMap::new(), true, Auth::None);
    assert!(matches!(locked.ensure_writable(), Err(ApiError::ReadOnly)));
}

#[test]
fn an_unknown_vault_name_is_reported_with_the_name() {
    let state = AppState::from_parts(HashMap::new(), false, Auth::None);
    match state.vault("nope") {
        Err(ApiError::UnknownVault(name)) => assert_eq!(name, "nope"),
        other => panic!("expected UnknownVault, got {other:?}"),
    }
}

#[test]
fn load_fails_naming_a_missing_token_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        vaults_path: dir.path().to_path_buf(),
        bind: "127.0.0.1:8080".parse().unwrap(),
        read_only: false,
        auth: crate::auth::AuthConfig::Bearer {
            token: None,
            token_file: Some(dir.path().join("missing-token")),
        },
        tls: None,
        allow_insecure_http: false,
    };

    let error = match AppState::load(&config) {
        Ok(_) => panic!("a missing token file should stop startup"),
        Err(e) => format!("{e:#}"),
    };

    assert!(error.contains("missing-token"), "{error}");
}

/// Log lines written while a test runs, in memory.
#[derive(Clone, Default)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Runs `f` with every log line it writes captured, returning them.
fn logs_of(f: impl FnOnce()) -> String {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    String::from_utf8(captured.0.lock().unwrap().clone()).unwrap()
}

#[cfg(unix)]
#[test]
fn serving_a_crypt_vault_warns_that_this_host_holds_the_key() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("data")).unwrap();
    let key = nimbus_vault::crypt::CryptKey::generate().unwrap().to_text();
    std::fs::write(dir.path().join("secret.key"), format!("{}\n", key.as_str())).unwrap();
    std::fs::set_permissions(
        dir.path().join("secret.key"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("secret.toml"),
        "name = \"secret\"\n\n[origin_config]\ntype = \"crypt\"\nkey_file = \"secret.key\"\n\n\
         [origin_config.inner]\ntype = \"fs\"\nroot = \"data\"\n",
    )
    .unwrap();

    let mut vaults = HashMap::new();
    let logs = logs_of(|| vaults = load_vaults(dir.path()).unwrap());

    assert!(vaults.contains_key("secret"));
    assert!(logs.contains("encrypted at rest"), "{logs}");
    assert!(
        logs.contains("keep the crypt vault on the client"),
        "{logs}"
    );
    assert!(logs.contains("accessible to other users"), "{logs}");
    assert!(!logs.contains(&key[20..]), "{logs}");
}

#[test]
fn a_plain_vault_gets_no_crypt_warning() {
    let dir = tempfile::tempdir().unwrap();
    write_vault_config(dir.path(), "plain", "plain");
    let logs = logs_of(|| {
        load_vaults(dir.path()).unwrap();
    });
    assert!(!logs.contains("encrypted"), "{logs}");
}

#[test]
fn serving_a_vault_that_wraps_a_crypt_vault_warns_too() {
    let dir = tempfile::tempdir().unwrap();
    let vaults = dir.path().join("vaults");
    let hidden = dir.path().join("hidden");
    std::fs::create_dir_all(&vaults).unwrap();
    std::fs::create_dir_all(hidden.join("data")).unwrap();
    let key = nimbus_vault::crypt::CryptKey::generate().unwrap().to_text();
    std::fs::write(hidden.join("secret.key"), format!("{}\n", key.as_str())).unwrap();
    std::fs::write(
        hidden.join("secret.toml"),
        "name = \"secret\"\n\n[origin_config]\ntype = \"crypt\"\nkey_file = \"secret.key\"\n\n\
         [origin_config.inner]\ntype = \"fs\"\nroot = \"data\"\n",
    )
    .unwrap();
    std::fs::write(
        vaults.join("wrapper.toml"),
        format!(
            "name = \"wrapper\"\n\n[origin_config]\ntype = \"vault\"\npath = \"{}\"\n",
            hidden.join("secret.toml").display()
        ),
    )
    .unwrap();

    let mut loaded = HashMap::new();
    let logs = logs_of(|| loaded = load_vaults(&vaults).unwrap());

    assert!(loaded.contains_key("wrapper"));
    assert!(logs.contains("encrypted at rest"), "{logs}");
}
