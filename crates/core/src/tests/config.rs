use super::*;

#[test]
fn default_local_vault_defaults_to_true_when_omitted() {
    let cfg: CliConfig = toml::from_str("").unwrap();
    assert!(cfg.default_local_vault);
    assert!(cfg.local_vault_path.is_none());
}

#[test]
fn deserializes_explicit_values() {
    let cfg: CliConfig = toml::from_str(
        r#"
        default_local_vault = false
        local_vault_path = "/srv/data"
        "#,
    )
    .unwrap();
    assert!(!cfg.default_local_vault);
    assert_eq!(cfg.local_vault_path, Some(PathBuf::from("/srv/data")));
}

#[test]
fn local_path_returns_configured_path_when_set() {
    let cfg = CliConfig {
        default_local_vault: true,
        local_vault_path: Some(PathBuf::from("/srv/data")),
    };
    assert_eq!(cfg.local_path(), PathBuf::from("/srv/data"));
}

#[test]
fn local_path_falls_back_to_home_dir_when_unset() {
    let cfg = CliConfig {
        default_local_vault: true,
        local_vault_path: None,
    };
    let expected = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    assert_eq!(cfg.local_path(), expected);
}

#[test]
fn a_relative_local_vault_path_resolves_against_the_config_dir() {
    let cfg: CliConfig = toml::from_str("local_vault_path = \"files\"").unwrap();
    let cfg = cfg
        .relative_to(Path::new("/home/me/.config/.nimbus/cli_config.toml"))
        .unwrap();
    assert_eq!(
        cfg.local_vault_path,
        Some(PathBuf::from("/home/me/.config/.nimbus/files"))
    );
}

#[test]
fn an_absolute_local_vault_path_is_untouched() {
    let cfg: CliConfig = toml::from_str("local_vault_path = \"/srv/files\"").unwrap();
    let cfg = cfg
        .relative_to(Path::new("/home/me/.config/.nimbus/cli_config.toml"))
        .unwrap();
    assert_eq!(cfg.local_vault_path, Some(PathBuf::from("/srv/files")));
}
