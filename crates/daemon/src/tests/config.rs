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
        ..FileConfig::default()
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
        token: Some("from-flag".into()),
        ..args()
    };

    let config = Config::merge(file, &args, &config_path()).unwrap();

    match config.auth {
        AuthConfig::Bearer { token, token_file } => {
            assert_eq!(token.as_ref().map(Secret::expose), Some("from-flag"));
            assert_eq!(token_file, None);
        }
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
        AuthConfig::Bearer { token, .. } => {
            assert_eq!(token.as_ref().map(Secret::expose), Some("s3cr3t"))
        }
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

// --- regressions: the generated config is written atomically, and kept private with a token ---

#[test]
fn the_generated_default_config_is_written_whole_and_parses_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/daemon_config.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // A stale, longer file in the way must be replaced completely, not partially overwritten.
    std::fs::write(&path, "x".repeat(4096)).unwrap();
    let mut initial = FileConfig::initial();
    initial.vaults_path = Some(dir.path().join("vaults"));

    initial.write(&path).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let parsed: FileConfig = toml::from_str(&raw).unwrap();
    assert_eq!(parsed.vaults_path, Some(dir.path().join("vaults")));
    assert!(!raw.contains("xxxx"));
    // No temp file left next to it.
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn a_config_holding_a_bearer_token_is_written_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mode_of = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

    let mut with_token = FileConfig::initial();
    with_token.vaults_path = Some(dir.path().join("vaults"));
    with_token.auth = Some(bearer());
    let secret = dir.path().join("secret.toml");
    with_token.write(&secret).unwrap();
    assert_eq!(mode_of(&secret), 0o600);

    let mut open = FileConfig::initial();
    open.vaults_path = Some(dir.path().join("vaults"));
    let plain = dir.path().join("plain.toml");
    open.write(&plain).unwrap();
    assert_ne!(mode_of(&plain), 0o600);
}

// --- TLS settings and the plain-HTTP safety rule ---

/// A resolved config with `bind` and `auth`, and everything TLS-related off.
fn config_with(bind: &str, auth: AuthConfig) -> Config {
    Config {
        vaults_path: PathBuf::from("/vaults"),
        bind: bind.parse().unwrap(),
        read_only: false,
        auth,
        tls: None,
        allow_insecure_http: false,
    }
}

fn bearer() -> AuthConfig {
    AuthConfig::Bearer {
        token: Some("s3cr3t".into()),
        token_file: None,
    }
}

fn bearer_from_file() -> AuthConfig {
    AuthConfig::Bearer {
        token: None,
        token_file: Some(PathBuf::from("/etc/nimbus/token")),
    }
}

fn some_tls() -> TlsConfig {
    TlsConfig {
        cert_file: PathBuf::from("/etc/nimbus/cert.pem"),
        key_file: PathBuf::from("/etc/nimbus/key.pem"),
    }
}

#[test]
fn validate_refuses_a_bearer_token_over_plain_http_on_a_reachable_address() {
    for bind in ["192.168.1.10:8080", "0.0.0.0:8080", "[::]:8080"] {
        let error = config_with(bind, bearer()).validate().unwrap_err();
        assert!(error.to_string().contains("plain HTTP"), "{bind}: {error}");
    }
}

#[test]
fn validate_allows_it_with_allow_insecure_http_but_warns() {
    let mut config = config_with("0.0.0.0:8080", bearer());
    config.allow_insecure_http = true;
    let warnings = config.validate().unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("allow_insecure_http")),
        "{warnings:?}"
    );
}

#[test]
fn validate_accepts_loopback_or_tls_without_warnings() {
    for bind in ["127.0.0.1:8080", "[::1]:8080"] {
        assert_eq!(
            config_with(bind, bearer()).validate().unwrap(),
            Vec::<String>::new()
        );
    }
    let mut with_tls = config_with("0.0.0.0:8443", bearer());
    with_tls.tls = Some(some_tls());
    assert_eq!(with_tls.validate().unwrap(), Vec::<String>::new());
}

#[test]
fn validate_only_warns_about_no_auth_on_a_reachable_address() {
    let warnings = config_with("0.0.0.0:8080", AuthConfig::None)
        .validate()
        .unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("no authentication"), "{warnings:?}");
    assert!(
        config_with("127.0.0.1:8080", AuthConfig::None)
            .validate()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_tls_table_round_trips_through_the_config_file() {
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        allow_insecure_http: Some(true),
        tls: Some(some_tls()),
        ..FileConfig::default()
    };
    let written = toml::to_string_pretty(&file).unwrap();
    assert!(written.contains("[tls]"), "{written}");
    let read_back: FileConfig = toml::from_str(&written).unwrap();
    assert_eq!(read_back.tls, Some(some_tls()));
    assert_eq!(read_back.allow_insecure_http, Some(true));
}

#[test]
fn the_generated_default_config_has_no_tls_settings() {
    let written = toml::to_string_pretty(&FileConfig::initial()).unwrap();
    assert!(!written.contains("tls"), "{written}");
    assert!(!written.contains("allow_insecure_http"), "{written}");
}

#[test]
fn an_unknown_key_in_the_tls_table_is_an_error() {
    let raw = "vaults_path = \"/vaults\"\n\n[tls]\ncert_file = \"c.pem\"\nkey_file = \"k.pem\"\nskip_verify = true\n";
    assert!(toml::from_str::<FileConfig>(raw).is_err());
}

#[test]
fn tls_cert_without_tls_key_is_an_error() {
    // Rejected by clap on the command line...
    assert!(Args::try_parse_from(["nimbus-daemon", "--tls-cert", "c.pem"]).is_err());
    assert!(Args::try_parse_from(["nimbus-daemon", "--tls-key", "k.pem"]).is_err());
    // ...and by merge for an `Args` built any other way.
    let args = Args {
        tls_cert: Some(PathBuf::from("c.pem")),
        ..Args::default()
    };
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        ..FileConfig::default()
    };
    assert!(Config::merge(file, &args, &config_path()).is_err());
}

#[test]
fn tls_flags_win_over_the_tls_table() {
    let parsed = Args::try_parse_from([
        "nimbus-daemon",
        "--tls-cert",
        "flag.pem",
        "--tls-key",
        "flag.key",
    ])
    .unwrap();
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        tls: Some(some_tls()),
        ..FileConfig::default()
    };
    let config = Config::merge(file, &parsed, &config_path()).unwrap();
    assert_eq!(
        config.tls,
        Some(TlsConfig {
            cert_file: PathBuf::from("flag.pem"),
            key_file: PathBuf::from("flag.key"),
        })
    );

    let from_file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        tls: Some(some_tls()),
        ..FileConfig::default()
    };
    let config = Config::merge(from_file, &args(), &config_path()).unwrap();
    assert_eq!(config.tls, Some(some_tls()));
}

#[test]
fn allow_insecure_http_comes_only_from_the_file() {
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        allow_insecure_http: Some(true),
        ..FileConfig::default()
    };
    assert!(
        Config::merge(file, &args(), &config_path())
            .unwrap()
            .allow_insecure_http
    );
    assert!(Args::try_parse_from(["nimbus-daemon", "--allow-insecure-http"]).is_err());
}

// --- token_file ---

#[test]
fn the_auth_table_accepts_token_file() {
    let raw = r#"
        [auth]
        type = "bearer"
        token_file = "/etc/nimbus/token"
    "#;

    let file: FileConfig = toml::from_str(raw).unwrap();

    match file.auth.unwrap() {
        AuthConfig::Bearer { token, token_file } => {
            assert_eq!(token.as_ref().map(Secret::expose), None);
            assert_eq!(token_file, Some(PathBuf::from("/etc/nimbus/token")));
        }
        other => panic!("expected a bearer token file, got {other:?}"),
    }
}

#[test]
fn token_flag_replaces_a_token_file_from_the_file() {
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        auth: Some(bearer_from_file()),
        ..FileConfig::default()
    };
    let args = Args {
        token: Some("from-flag".into()),
        ..args()
    };

    let config = Config::merge(file, &args, &config_path()).unwrap();

    match config.auth {
        AuthConfig::Bearer { token, token_file } => {
            assert_eq!(token.as_ref().map(Secret::expose), Some("from-flag"));
            assert_eq!(token_file, None);
        }
        other => panic!("expected a bearer token, got {other:?}"),
    }
}

#[test]
fn validate_refuses_a_token_file_over_plain_http_on_a_reachable_address() {
    let error = config_with("0.0.0.0:8080", bearer_from_file())
        .validate()
        .unwrap_err();
    assert!(error.to_string().contains("plain HTTP"), "{error}");
}

#[cfg(unix)]
#[test]
fn a_config_with_only_a_token_file_is_not_made_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut config = FileConfig::initial();
    config.vaults_path = Some(dir.path().join("vaults"));
    config.auth = Some(bearer_from_file());
    assert!(!bearer_from_file().holds_secret());

    let path = dir.path().join("daemon_config.toml");
    config.write(&path).unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_ne!(mode, 0o600);
}

// --- secrets stay out of Debug ---

#[test]
fn no_debug_view_shows_the_token() {
    let args = Args::try_parse_from(["nimbus-daemon", "--token", "s3cr3t"]).unwrap();
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/vaults")),
        auth: Some(bearer()),
        ..FileConfig::default()
    };
    let file_view = format!("{file:?}");
    let config = Config::merge(file, &args, &config_path()).unwrap();
    let views = [
        format!("{:?}", bearer()),
        format!("{:?}", bearer().resolve().unwrap()),
        file_view,
        format!("{args:?}"),
        format!("{config:?}"),
    ];

    for view in views {
        assert!(!view.contains("s3cr3t"), "{view}");
    }
}

#[test]
fn relative_paths_in_the_file_resolve_against_its_directory() {
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("vaults")),
        auth: Some(AuthConfig::Bearer {
            token: None,
            token_file: Some(PathBuf::from("secrets/token")),
        }),
        tls: Some(TlsConfig {
            cert_file: PathBuf::from("tls/cert.pem"),
            key_file: PathBuf::from("../keys/key.pem"),
        }),
        ..FileConfig::default()
    };
    let config = Config::merge(file, &args(), Path::new("/etc/nimbus/daemon_config.toml")).unwrap();

    assert_eq!(config.vaults_path, PathBuf::from("/etc/nimbus/vaults"));
    assert_eq!(
        config.tls,
        Some(TlsConfig {
            cert_file: PathBuf::from("/etc/nimbus/tls/cert.pem"),
            key_file: PathBuf::from("/etc/nimbus/../keys/key.pem"),
        })
    );
    match config.auth {
        AuthConfig::Bearer { token_file, .. } => {
            assert_eq!(token_file, Some(PathBuf::from("/etc/nimbus/secrets/token")))
        }
        other => panic!("expected bearer auth, got {other:?}"),
    }
}

#[test]
fn absolute_paths_in_the_file_are_untouched() {
    let file = FileConfig {
        vaults_path: Some(PathBuf::from("/srv/vaults")),
        tls: Some(some_tls()),
        ..FileConfig::default()
    };
    let config = Config::merge(file, &args(), Path::new("/etc/nimbus/daemon_config.toml")).unwrap();
    assert_eq!(config.vaults_path, PathBuf::from("/srv/vaults"));
    assert_eq!(config.tls, Some(some_tls()));
}

#[test]
fn path_flags_stay_relative_to_the_current_directory() {
    let parsed = Args::try_parse_from([
        "nimbus-daemon",
        "--vaults",
        "vaults",
        "--tls-cert",
        "cert.pem",
        "--tls-key",
        "key.pem",
    ])
    .unwrap();
    let config = Config::merge(
        FileConfig::default(),
        &parsed,
        Path::new("/etc/nimbus/daemon_config.toml"),
    )
    .unwrap();
    assert_eq!(config.vaults_path, PathBuf::from("vaults"));
    assert_eq!(
        config.tls,
        Some(TlsConfig {
            cert_file: PathBuf::from("cert.pem"),
            key_file: PathBuf::from("key.pem"),
        })
    );
}

#[test]
fn a_malformed_config_never_echoes_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon_config.toml");
    for (line, secret) in [
        ("token = \"SUPERSECRET-123\" extra", "SUPERSECRET-123"),
        ("token = 12345678", "12345678"),
        ("token = [\"SUPERSECRET-456\"]", "SUPERSECRET-456"),
    ] {
        std::fs::write(
            &path,
            format!("vaults_path = \"/srv/vaults\"\n\n[auth]\ntype = \"bearer\"\n{line}\n"),
        )
        .unwrap();
        let args = Args {
            config: Some(path.clone()),
            ..Args::default()
        };
        let error = Config::resolve(&args).unwrap_err();
        for shown in [
            error.to_string(),
            format!("{error:?}"),
            format!("{error:#}"),
        ] {
            assert!(!shown.contains(secret), "{line:?} leaked: {shown}");
        }
        assert!(
            format!("{error:#}").contains("daemon_config.toml"),
            "{error:#}"
        );
    }
}
