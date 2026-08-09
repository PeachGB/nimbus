use super::*;
use crate::auth::AuthConfig;

fn args() -> Args {
    Args::default()
}

fn config_path() -> PathBuf {
    PathBuf::from("/nonexistent/daemon_config.toml")
}

#[test]
fn flags_win_over_the_file() {
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/from/file")),
        bind: Some("127.0.0.1:1111".parse().unwrap()),
        read_only: Some(false),
        auth: None,
    };
    let args = Args {
        vaults: Some(PathBuf::from("/from/flag")),
        bind: Some("127.0.0.1:2222".parse().unwrap()),
        ..args()
    };

    let config = Config::merge(file, &args, &config_path()).unwrap();

    assert_eq!(config.vaults_path, PathBuf::from("/from/flag"));
    assert_eq!(config.bind, "127.0.0.1:2222".parse().unwrap());
}

#[test]
fn the_file_wins_over_the_defaults() {
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        bind: Some("0.0.0.0:9000".parse().unwrap()),
        ..FileConfig::default()
    };

    let config = Config::merge(file, &args(), &config_path()).unwrap();

    assert_eq!(config.bind, "0.0.0.0:9000".parse().unwrap());
}

#[test]
fn the_default_bind_is_loopback() {
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        ..FileConfig::default()
    };

    let config = Config::merge(file, &args(), &config_path()).unwrap();

    assert_eq!(config.bind, DEFAULT_BIND.parse().unwrap());
    assert!(config.bind.ip().is_loopback());
}

#[test]
fn a_missing_vaults_path_names_the_config_file_to_edit() {
    let error = Config::merge(FileConfig::default(), &args(), &config_path()).unwrap_err();

    let message = error.to_string();
    assert!(
        message.contains("/nonexistent/daemon_config.toml"),
        "{message}"
    );
    assert!(message.contains("--vaults"), "{message}");
}

#[test]
fn read_only_is_a_one_way_switch() {
    let locked = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        read_only: Some(true),
        ..FileConfig::default()
    };
    assert!(
        Config::merge(locked, &args(), &config_path())
            .unwrap()
            .read_only
    );

    let open = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        read_only: Some(false),
        ..FileConfig::default()
    };
    let args = Args {
        read_only: true,
        ..args()
    };
    assert!(
        Config::merge(open, &args, &config_path())
            .unwrap()
            .read_only
    );
}

#[test]
fn the_token_flag_replaces_the_configured_auth() {
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        auth: Some(AuthConfig::None),
        ..FileConfig::default()
    };
    let args = Args {
        token: Some("from-flag".to_string()),
        ..args()
    };

    let config = Config::merge(file, &args, &config_path()).unwrap();

    match config.auth {
        AuthConfig::Bearer { token } => assert_eq!(token, "from-flag"),
        other => panic!("expected a bearer token, got {other:?}"),
    }
}

#[test]
fn auth_defaults_to_none_when_the_file_omits_it() {
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        ..FileConfig::default()
    };

    let config = Config::merge(file, &args(), &config_path()).unwrap();

    assert!(config.auth.is_open());
}

#[test]
fn a_full_config_file_parses() {
    let raw = r#"
        vaults_path = "/srv/nimbus/vaults"
        bind = "0.0.0.0:8080"
        read_only = true

        [auth]
        type = "bearer"
        token = "s3cr3t"
    "#;

    let file: FileConfig = toml::from_str(raw).unwrap();

    assert_eq!(file.vaults_path, Some(PathBuf::from("/srv/nimbus/vaults")));
    assert_eq!(file.read_only, Some(true));
    match file.auth.unwrap() {
        AuthConfig::Bearer { token } => assert_eq!(token, "s3cr3t"),
        other => panic!("expected a bearer token, got {other:?}"),
    }
}

#[test]
fn a_config_file_with_only_a_vaults_path_parses() {
    let file: FileConfig = toml::from_str(r#"vaults_path = "/srv/vaults""#).unwrap();
    assert_eq!(file.vaults_path, Some(PathBuf::from("/srv/vaults")));
    assert!(file.auth.is_none());
}

#[test]
fn a_misspelled_key_is_an_error_rather_than_silently_ignored() {
    let result: Result<FileConfig, _> = toml::from_str(r#"vaults_paths = "/srv/vaults""#);
    assert!(result.is_err());
}

#[test]
fn an_explicit_config_that_does_not_exist_is_an_error() {
    let args = Args {
        config: Some(PathBuf::from("/nonexistent/nimbus-daemon.toml")),
        vaults: Some(PathBuf::from("/vaults")),
        ..args()
    };

    let error = Config::resolve(&args).unwrap_err();

    assert!(error.to_string().contains("no config file at"), "{error}");
}

#[test]
fn the_config_written_on_first_run_parses_back_into_the_built_in_defaults() {
    let raw = toml::to_string_pretty(&FileConfig::initial()).unwrap();

    let file: FileConfig = toml::from_str(&raw).unwrap();
    let config = Config::merge(file, &args(), &config_path()).unwrap();

    assert_eq!(config.bind, DEFAULT_BIND.parse().unwrap());
    assert!(!config.read_only);
    assert!(config.auth.is_open());
    assert_eq!(
        config.vaults_path,
        nimbus_vault::config::VaultConfig::default_dir()
    );
}

#[test]
fn writing_the_default_config_creates_the_directories_it_needs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/daemon_config.toml");
    let vaults = dir.path().join("nested/vaults");

    let file = FileConfig {
        vaults_path: Some(vaults.clone()),
        ..FileConfig::initial()
    };
    file.write(&path).unwrap();

    assert!(path.is_file());
    assert!(vaults.is_dir());

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.starts_with('#'), "{raw}");
    let parsed: FileConfig = toml::from_str(&raw).unwrap();
    assert_eq!(parsed.vaults_path, Some(vaults));
}

#[test]
fn an_existing_config_file_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon_config.toml");
    let original = "vaults_path = \"/from/file\"\n";
    std::fs::write(&path, original).unwrap();

    let args = Args {
        config: Some(path.clone()),
        ..args()
    };
    Config::resolve(&args).unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn resolve_reads_the_file_and_then_applies_the_flags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon_config.toml");
    std::fs::write(
        &path,
        "vaults_path = \"/from/file\"\nbind = \"127.0.0.1:1111\"\n",
    )
    .unwrap();

    let args = Args {
        config: Some(path),
        bind: Some("127.0.0.1:2222".parse().unwrap()),
        ..args()
    };

    let config = Config::resolve(&args).unwrap();

    assert_eq!(config.vaults_path, PathBuf::from("/from/file"));
    assert_eq!(config.bind, "127.0.0.1:2222".parse().unwrap());
}
