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
