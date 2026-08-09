use super::*;
use crate::error::VaultError;
use tempfile::tempdir;

fn write_config(contents: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("vault.toml");
    std::fs::write(&path, contents).unwrap();
    (dir, path)
}

#[test]
fn build_returns_not_found_for_missing_file() {
    let dir = tempdir().unwrap();
    let result = VaultConfig::build(dir.path().join("missing.toml"));
    assert!(matches!(result, Err(VaultError::Io(_))));
}

#[test]
fn build_returns_toml_error_for_invalid_toml() {
    let (_dir, path) = write_config("not valid toml {{{");
    let result = VaultConfig::build(path);
    assert!(matches!(result, Err(VaultError::Toml(_))));
}

#[test]
fn build_constructs_fs_origin() {
    let (_dir, path) = write_config(
        r#"
        name = "my-vault"

        [origin_config]
        type = "fs"
        root = "/some/root"
        "#,
    );
    let (name, root_id, _origin) = VaultConfig::build(path).unwrap();
    assert_eq!(name, "my-vault");
    assert_eq!(root_id.as_str(), "/");
}

#[test]
fn build_uses_explicit_root_id_when_given() {
    let (_dir, path) = write_config(
        r#"
        name = "my-vault"
        root_id = "custom-root"

        [origin_config]
        type = "fs"
        root = "/some/root"
        "#,
    );
    let (_name, root_id, _origin) = VaultConfig::build(path).unwrap();
    assert_eq!(root_id.as_str(), "custom-root");
}

#[test]
fn build_constructs_command_origin() {
    let (_dir, path) = write_config(
        r#"
        name = "cmd-vault"

        [origin_config]
        type = "command"
        list_cmd = "ls"
        fetch_cmd = "cat {id}"
        get_cmd = "stat {id}"
        put_cmd = "touch {id}"
        send_cmd = "touch {id}"
        delete_cmd = "rm {id}"
        "#,
    );
    let (name, _root_id, _origin) = VaultConfig::build(path).unwrap();
    assert_eq!(name, "cmd-vault");
}

#[test]
fn build_constructs_http_origin() {
    let (_dir, path) = write_config(
        r#"
        name = "http-vault"

        [origin_config]
        type = "http"
        base_url = "https://example.com"
        list_url = "/list/{id}"
        fetch_url = "/fetch/{id}"
        get_url = "/get/{id}"
        put_url = "/put/{id}"
        send_url = "/send/{id}"
        delete_url = "/delete/{id}"
        "#,
    );
    let (name, _root_id, _origin) = VaultConfig::build(path).unwrap();
    assert_eq!(name, "http-vault");
}

#[test]
fn build_constructs_http_origin_with_default_base_url() {
    let (_dir, path) = write_config(
        r#"
        name = "http-vault-no-base"

        [origin_config]
        type = "http"
        list_url = "/list/{id}"
        fetch_url = "/fetch/{id}"
        get_url = "/get/{id}"
        put_url = "/put/{id}"
        send_url = "/send/{id}"
        delete_url = "/delete/{id}"
        "#,
    );
    let (name, _root_id, _origin) = VaultConfig::build(path).unwrap();
    assert_eq!(name, "http-vault-no-base");
}

#[test]
fn build_constructs_http_origin_with_auth() {
    let name = "NIMBUS_TEST_CONFIG_TOKEN";
    // The environment is process-wide, so this uses a name no other test touches.
    unsafe { std::env::set_var(name, "s3cr3t") };
    let (_dir, path) = write_config(
        r#"
        name = "http-vault-auth"

        [origin_config]
        type = "http"
        base_url = "https://example.com"
        list_url = "/list/{id}"
        fetch_url = "/fetch/{id}"
        get_url = "/get/{id}"
        put_url = "/put/{id}"
        send_url = "/send/{id}"
        delete_url = "/delete/{id}"

        [origin_config.auth]
        type = "bearer"
        token_env = "NIMBUS_TEST_CONFIG_TOKEN"
        "#,
    );

    let (name_out, _root_id, _origin) = VaultConfig::build(path).unwrap();

    assert_eq!(name_out, "http-vault-auth");
    unsafe { std::env::remove_var(name) };
}

#[test]
fn an_unresolvable_token_fails_when_the_vault_is_opened() {
    // Better here, while the config is being read, than as a 401 halfway through a sync.
    let (_dir, path) = write_config(
        r#"
        name = "http-vault-bad-auth"

        [origin_config]
        type = "http"
        list_url = "/list/{id}"
        fetch_url = "/fetch/{id}"
        get_url = "/get/{id}"
        put_url = "/put/{id}"
        send_url = "/send/{id}"
        delete_url = "/delete/{id}"

        [origin_config.auth]
        type = "bearer"
        token_env = "NIMBUS_TEST_CONFIG_TOKEN_NEVER_SET"
        "#,
    );

    let Err(error) = VaultConfig::build(path) else {
        panic!("expected an error, the token can't be resolved");
    };

    let error = error.to_string();
    assert!(
        error.contains("NIMBUS_TEST_CONFIG_TOKEN_NEVER_SET"),
        "{error}"
    );
}

fn write_origin_config(contents: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("origin.toml");
    std::fs::write(&path, contents).unwrap();
    (dir, path)
}

#[test]
fn origin_config_from_file_builds_fs_origin_without_a_vault() {
    let (_dir, path) = write_origin_config(
        r#"
        type = "fs"
        root = "/some/root"
        "#,
    );
    let result = OriginConfig::from_file(path);
    assert!(result.is_ok());
}

#[test]
fn origin_config_from_file_builds_command_origin_without_a_vault() {
    let (_dir, path) = write_origin_config(
        r#"
        type = "command"
        list_cmd = "ls"
        fetch_cmd = "cat {id}"
        get_cmd = "stat {id}"
        put_cmd = "touch {id}"
        send_cmd = "touch {id}"
        delete_cmd = "rm {id}"
        "#,
    );
    let result = OriginConfig::from_file(path);
    assert!(result.is_ok());
}

#[test]
fn origin_config_from_file_builds_http_origin_without_a_vault() {
    let (_dir, path) = write_origin_config(
        r#"
        type = "http"
        base_url = "https://example.com"
        list_url = "/list/{id}"
        fetch_url = "/fetch/{id}"
        get_url = "/get/{id}"
        put_url = "/put/{id}"
        send_url = "/send/{id}"
        delete_url = "/delete/{id}"
        "#,
    );
    let result = OriginConfig::from_file(path);
    assert!(result.is_ok());
}

#[test]
fn origin_config_from_file_returns_not_found_for_missing_file() {
    let dir = tempdir().unwrap();
    let result = OriginConfig::from_file(dir.path().join("missing.toml"));
    assert!(matches!(result, Err(VaultError::Io(_))));
}

#[test]
fn origin_config_from_file_returns_toml_error_for_invalid_toml() {
    let (_dir, path) = write_origin_config("not valid toml {{{");
    let result = OriginConfig::from_file(path);
    assert!(matches!(result, Err(VaultError::Toml(_))));
}

#[test]
fn build_constructs_vault_origin_wrapping_another_vault() {
    let dir = tempdir().unwrap();
    let fs_root = dir.path().join("data");
    std::fs::create_dir_all(&fs_root).unwrap();

    let inner_config_path = dir.path().join("inner.toml");
    std::fs::write(
        &inner_config_path,
        format!(
            r#"
            name = "inner-vault"

            [origin_config]
            type = "fs"
            root = "{}"
            "#,
            fs_root.display()
        ),
    )
    .unwrap();

    let (_dir2, outer_path) = write_config(&format!(
        r#"
        name = "outer-vault"

        [origin_config]
        type = "vault"
        path = "{}"
        "#,
        inner_config_path.display()
    ));

    let (name, _root_id, _origin) = VaultConfig::build(outer_path).unwrap();
    assert_eq!(name, "outer-vault");
}

#[test]
fn build_propagates_error_when_inner_vault_config_is_missing() {
    let dir = tempdir().unwrap();
    let missing_inner_path = dir.path().join("missing-inner.toml");

    let (_dir2, outer_path) = write_config(&format!(
        r#"
        name = "outer-vault"

        [origin_config]
        type = "vault"
        path = "{}"
        "#,
        missing_inner_path.display()
    ));

    let result = VaultConfig::build(outer_path);
    assert!(matches!(result, Err(VaultError::Io(_))));
}

// --- default location for new vault configs ---

#[test]
fn default_path_is_a_named_file_in_the_default_dir() {
    let path = VaultConfig::default_path("backup");
    assert_eq!(path.parent(), Some(VaultConfig::default_dir().as_path()));
    assert_eq!(path.file_name().unwrap(), "backup.toml");
}

#[test]
fn default_dir_lives_under_the_nimbus_config_home() {
    assert!(VaultConfig::default_dir().starts_with(crate::config_home()));
}

#[test]
fn a_saved_http_config_with_auth_can_be_read_back() {
    // TOML can't emit a plain value after a table, so `auth` being the variant's last field
    // isn't cosmetic — writing it anywhere else makes `save` fail on a config the wizard just
    // built.
    let dir = tempdir().unwrap();
    let path = dir.path().join("remote.toml");
    let config = VaultConfig::new(
        "remote".to_string(),
        ObjectId::default(),
        OriginConfig::Http {
            base_url: Some("https://example.com".to_string()),
            list_url: "/list/{id}".to_string(),
            fetch_url: "/fetch/{id}".to_string(),
            get_url: "/get/{id}".to_string(),
            put_url: "/put/{id}".to_string(),
            send_url: "/send/{id}".to_string(),
            delete_url: "/delete/{id}".to_string(),
            auth: crate::origin::http::HttpAuth::Bearer {
                token: None,
                token_env: Some("NIMBUS_TEST_ROUNDTRIP_TOKEN".to_string()),
                token_file: None,
            },
        },
    );

    config.save(&path).unwrap();

    let written = std::fs::read_to_string(&path).unwrap();
    let read_back: VaultConfig = toml::from_str(&written).unwrap();
    match read_back.origin_config {
        OriginConfig::Http { auth, .. } => assert_eq!(
            auth,
            crate::origin::http::HttpAuth::Bearer {
                token: None,
                token_env: Some("NIMBUS_TEST_ROUNDTRIP_TOKEN".to_string()),
                token_file: None,
            }
        ),
        _ => panic!("expected an http origin"),
    }
    // The unset fields don't clutter the file.
    assert!(!written.contains("token_file"), "{written}");
}

#[test]
fn save_creates_the_directory_it_is_asked_to_write_into() {
    // The conventional vaults directory doesn't exist until the first vault is created.
    let dir = tempdir().unwrap();
    let path = dir.path().join("nested/deeper/vault.toml");
    let config = VaultConfig::new(
        "v".to_string(),
        ObjectId::default(),
        OriginConfig::Fs {
            root: dir.path().to_path_buf(),
        },
    );

    config.save(&path).unwrap();
    assert!(path.is_file());
}

#[test]
fn save_still_works_for_a_bare_relative_filename() {
    // `Path::parent()` of "vault.toml" is Some(""), which `create_dir_all` would choke on.
    let dir = tempdir().unwrap();
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let config = VaultConfig::new(
        "v".to_string(),
        ObjectId::default(),
        OriginConfig::Fs {
            root: dir.path().to_path_buf(),
        },
    );
    let result = config.save(std::path::Path::new("bare.toml"));

    std::env::set_current_dir(previous).unwrap();
    result.unwrap();
    assert!(dir.path().join("bare.toml").is_file());
}
