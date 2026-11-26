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
            rename_url: None,
            tls: None,
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
        OriginConfig::Http { auth, .. } => assert!(
            matches!(
                &auth,
                crate::origin::http::HttpAuth::Bearer {
                    token: None,
                    token_env: Some(name),
                    token_file: None,
                } if name == "NIMBUS_TEST_ROUNDTRIP_TOKEN"
            ),
            "{auth:?}"
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

// --- regressions: atomic saves; literal tokens aren't world-readable ---

#[cfg(unix)]
#[test]
fn save_makes_a_config_holding_a_literal_token_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mode_of =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let http = |auth| OriginConfig::Http {
        base_url: Some("https://example.com".to_string()),
        list_url: "/list/{id}".to_string(),
        fetch_url: "/fetch/{id}".to_string(),
        get_url: "/get/{id}".to_string(),
        put_url: "/put/{id}".to_string(),
        send_url: "/send/{id}".to_string(),
        delete_url: "/delete/{id}".to_string(),
        rename_url: None,
        tls: None,
        auth,
    };

    let secret = dir.path().join("secret.toml");
    VaultConfig::new(
        "s".to_string(),
        ObjectId::default(),
        http(crate::origin::http::HttpAuth::Bearer {
            token: Some("hunter2".into()),
            token_env: None,
            token_file: None,
        }),
    )
    .save(&secret)
    .unwrap();
    assert_eq!(mode_of(&secret), 0o600);

    // A token taken from the environment isn't in the file, so nothing to hide.
    let plain = dir.path().join("plain.toml");
    VaultConfig::new(
        "p".to_string(),
        ObjectId::default(),
        http(crate::origin::http::HttpAuth::Bearer {
            token: None,
            token_env: Some("TOKEN".to_string()),
            token_file: None,
        }),
    )
    .save(&plain)
    .unwrap();
    assert_ne!(mode_of(&plain), 0o600);
}

#[test]
fn save_replaces_an_existing_config_without_leaving_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.toml");
    std::fs::write(&path, "old contents that are much longer than the new ones").unwrap();
    VaultConfig::new(
        "v".to_string(),
        ObjectId::default(),
        OriginConfig::Fs {
            root: "/srv".into(),
        },
    )
    .save(&path)
    .unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("name = \"v\"")
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

// --- [origin_config.tls] ---

/// An http vault config for `base_url`, followed by `tls_table` verbatim.
fn http_config_with_tls(base_url: &str, tls_table: &str) -> String {
    format!(
        r#"
        name = "tls-vault"

        [origin_config]
        type = "http"
        base_url = "{base_url}"
        list_url = "/list/{{id}}"
        fetch_url = "/fetch/{{id}}"
        get_url = "/get/{{id}}"
        put_url = "/put/{{id}}"
        send_url = "/send/{{id}}"
        delete_url = "/delete/{{id}}"

        {tls_table}
        "#
    )
}

#[test]
fn build_constructs_http_origin_with_a_pinned_certificate() {
    let (_dir, path) = write_config(&http_config_with_tls(
        "https://example.com",
        &format!("[origin_config.tls]\npin_sha256 = \"{}\"", "ab".repeat(32)),
    ));
    assert!(VaultConfig::build(path).is_ok());
}

#[test]
fn build_refuses_a_bad_tls_table_when_the_vault_opens() {
    // Like auth: a broken tls table fails when the vault opens, not on its first request.
    for (base_url, table) in [
        (
            "http://example.com",
            format!("[origin_config.tls]\npin_sha256 = \"{}\"", "ab".repeat(32)),
        ),
        (
            "https://example.com",
            "[origin_config.tls]\npin_sha256 = \"abcd\"".to_string(),
        ),
        (
            "https://example.com",
            "[origin_config.tls]\nca_file = \"/nonexistent/ca.pem\"".to_string(),
        ),
        ("https://example.com", "[origin_config.tls]".to_string()),
    ] {
        let (_dir, path) = write_config(&http_config_with_tls(base_url, &table));
        assert!(
            matches!(VaultConfig::build(path), Err(VaultError::Generic(m)) if m.starts_with("tls:")),
            "{base_url} {table}"
        );
    }
}

#[test]
fn an_unknown_key_in_the_tls_table_is_an_error() {
    let (_dir, path) = write_config(&http_config_with_tls(
        "https://example.com",
        "[origin_config.tls]\nskip_verify = true",
    ));
    assert!(matches!(VaultConfig::build(path), Err(VaultError::Toml(_))));
}

/// An http origin config with `tls` and nothing else optional.
fn http_origin_with(tls: Option<HttpTls>) -> OriginConfig {
    OriginConfig::Http {
        base_url: Some("https://example.com".to_string()),
        list_url: "/list/{id}".to_string(),
        fetch_url: "/fetch/{id}".to_string(),
        get_url: "/get/{id}".to_string(),
        put_url: "/put/{id}".to_string(),
        send_url: "/send/{id}".to_string(),
        delete_url: "/delete/{id}".to_string(),
        rename_url: None,
        auth: crate::origin::http::HttpAuth::None,
        tls,
    }
}

#[test]
fn the_tls_table_round_trips_and_is_omitted_when_unset() {
    let tls = HttpTls {
        ca_file: Some("/etc/nimbus/ca.pem".into()),
        pin_sha256: None,
    };
    let written = toml::to_string_pretty(&http_origin_with(Some(tls.clone()))).unwrap();
    assert!(
        written.contains("[tls]") && written.contains("ca_file"),
        "{written}"
    );
    assert!(!written.contains("pin_sha256"), "{written}");
    match toml::from_str::<OriginConfig>(&written).unwrap() {
        OriginConfig::Http { tls: read_back, .. } => assert_eq!(read_back, Some(tls)),
        _ => panic!("expected an http origin"),
    }

    let without = toml::to_string_pretty(&http_origin_with(None)).unwrap();
    assert!(!without.contains("tls"), "{without}");
}

// --- relative paths resolve against the config file's directory ---

/// `path` (absolute) spelled relative to the current directory: one `..` per component of the
/// cwd, back up to `/`, then `path` itself.
fn relative_from_cwd(path: &Path) -> PathBuf {
    let cwd = std::env::current_dir().unwrap();
    let ups = cwd.components().count() - 1;
    let mut relative: PathBuf = std::iter::repeat_n("..", ups).collect();
    relative.push(path.strip_prefix("/").unwrap());
    relative
}

/// Writes `dir/vault.toml` as an fs vault named `name` with `root = "<root>"`, and a file in
/// `dir/<root>` so a listing shows whether the root resolved where it should.
fn write_fs_vault(dir: &Path, name: &str, root: &str) -> PathBuf {
    std::fs::create_dir_all(dir.join(root)).unwrap();
    std::fs::write(dir.join(root).join("hello.txt"), b"hi").unwrap();
    let path = dir.join("vault.toml");
    std::fs::write(
        &path,
        format!("name = \"{name}\"\n\n[origin_config]\ntype = \"fs\"\nroot = \"{root}\"\n"),
    )
    .unwrap();
    path
}

async fn names_listed(origin: &dyn Origin) -> Vec<String> {
    let mut names: Vec<String> = origin
        .list(&ObjectId::from(""))
        .await
        .unwrap()
        .iter()
        .map(|o| o.get_name())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_config_loaded_from_another_cwd_builds_the_same() {
    // No `set_current_dir`: the cwd is process-wide and tests run in parallel. Instead the
    // same file is opened by its absolute path and by a path relative to the test's cwd (the
    // crate directory, which has no `data/`); a root resolved against the cwd would find
    // nothing either way.
    let dir = tempdir().unwrap();
    let config = write_fs_vault(dir.path(), "v", "data");

    let (_, _, by_absolute) = VaultConfig::build(config.clone()).unwrap();
    let (_, _, by_relative) = VaultConfig::build(relative_from_cwd(&config)).unwrap();

    assert_eq!(names_listed(by_absolute.as_ref()).await, vec!["hello.txt"]);
    assert_eq!(names_listed(by_relative.as_ref()).await, vec!["hello.txt"]);
}

/// An http config with `token_file` and `ca_file` set to `path`.
fn http_with_paths(path: &str) -> OriginConfig {
    OriginConfig::Http {
        base_url: Some("https://example.com".to_string()),
        list_url: "/list/{id}".to_string(),
        fetch_url: "/fetch/{id}".to_string(),
        get_url: "/get/{id}".to_string(),
        put_url: "/put/{id}".to_string(),
        send_url: "/send/{id}".to_string(),
        delete_url: "/delete/{id}".to_string(),
        rename_url: None,
        auth: HttpAuth::Bearer {
            token: None,
            token_env: None,
            token_file: Some(path.into()),
        },
        tls: Some(HttpTls {
            ca_file: Some(path.into()),
            pin_sha256: None,
        }),
    }
}

/// Every path field of `config` after `relative_to(dir)`: fs root, vault path, or the http
/// token_file and ca_file.
fn paths_after_relative_to(config: OriginConfig, dir: &str) -> Vec<PathBuf> {
    match config.relative_to(Path::new(dir)).unwrap() {
        OriginConfig::Fs { root } => vec![root],
        OriginConfig::Vault { path } => vec![path],
        OriginConfig::Http {
            auth: HttpAuth::Bearer { token_file, .. },
            tls: Some(HttpTls { ca_file, .. }),
            ..
        } => vec![token_file.unwrap(), ca_file.unwrap()],
        _ => panic!("unexpected variant"),
    }
}

#[test]
fn absolute_paths_are_untouched_by_relative_to() {
    let configs = [
        OriginConfig::Fs {
            root: "/srv/data".into(),
        },
        OriginConfig::Vault {
            path: "/srv/data".into(),
        },
        http_with_paths("/srv/data"),
    ];
    for config in configs {
        for path in paths_after_relative_to(config, "/etc/nimbus") {
            assert_eq!(path, PathBuf::from("/srv/data"));
        }
    }
}

#[test]
fn each_path_field_resolves_against_the_config_dir() {
    let configs = [
        OriginConfig::Fs {
            root: "data".into(),
        },
        OriginConfig::Vault {
            path: "data".into(),
        },
        http_with_paths("data"),
    ];
    for config in configs {
        for path in paths_after_relative_to(config, "/etc/nimbus") {
            assert_eq!(path, PathBuf::from("/etc/nimbus/data"));
        }
    }
}

#[tokio::test]
async fn a_nested_chain_resolves_per_directory() {
    let dir = tempdir().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    write_fs_vault(&b, "inner", "data");
    let outer = a.join("outer.toml");
    std::fs::write(
        &outer,
        "name = \"outer\"\n\n[origin_config]\ntype = \"vault\"\npath = \"../b/vault.toml\"\n",
    )
    .unwrap();

    let (_, _, origin) = VaultConfig::build(outer).unwrap();

    // outer's `../b/vault.toml` is relative to a/, inner's `data` to b/: b/data.
    assert_eq!(names_listed(origin.as_ref()).await, vec!["hello.txt"]);
}

/// A command origin config in `dir` whose `list_cmd` is `list_cmd` and whose extras are
/// `extras` (a TOML table body), the rest of its commands unused.
fn write_command_config(dir: &Path, list_cmd: &str, extras: &str) -> PathBuf {
    let path = dir.join("origin.toml");
    std::fs::write(
        &path,
        format!(
            "type = \"command\"\nlist_cmd = \"{list_cmd}\"\nfetch_cmd = \"true\"\n\
             get_cmd = \"true\"\nput_cmd = \"true\"\nsend_cmd = \"true\"\n\
             delete_cmd = \"true\"\n\n[extras]\n{extras}"
        ),
    )
    .unwrap();
    path
}

/// A directory whose name has a space and a `'`, the two things shell quoting has to survive.
fn awkward_dir() -> (tempfile::TempDir, PathBuf) {
    let base = tempdir().unwrap();
    let dir = base.path().join("it's a dir");
    std::fs::create_dir_all(&dir).unwrap();
    (base, dir)
}

#[tokio::test]
async fn config_dir_in_a_template_is_shell_quoted() {
    let (_base, dir) = awkward_dir();
    std::fs::write(dir.join("list.json"), "[]").unwrap();
    let config = write_command_config(&dir, "cat {config_dir}/list.json", "");

    let origin = OriginConfig::from_file(config).unwrap();

    // Unquoted, `sh` would split the path at the space (or choke on the `'`).
    assert!(names_listed(origin.as_ref()).await.is_empty());
}

#[tokio::test]
async fn config_dir_inside_an_extras_value_is_expanded() {
    let (_base, dir) = awkward_dir();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data").join("list.json"), "[]").unwrap();
    let config = write_command_config(
        &dir,
        "cat {root}/list.json",
        "root = \"{config_dir}/data\"\n",
    );

    let origin = OriginConfig::from_file(config).unwrap();

    assert!(names_listed(origin.as_ref()).await.is_empty());
}

#[test]
fn a_user_config_dir_extra_is_refused() {
    let config = OriginConfig::Command {
        list_cmd: "true".to_string(),
        fetch_cmd: "true".to_string(),
        get_cmd: "true".to_string(),
        put_cmd: "true".to_string(),
        send_cmd: "true".to_string(),
        delete_cmd: "true".to_string(),
        extras: Some(HashMap::from([(
            "config_dir".to_string(),
            "/mine".to_string(),
        )])),
        rename_cmd: None,
    };

    let error = match config.relative_to(Path::new("/etc/nimbus")) {
        Ok(_) => panic!("a user `config_dir` extra should be refused"),
        Err(e) => e.to_string(),
    };

    assert!(error.contains("`config_dir` is reserved"), "{error}");
}

#[test]
fn save_keeps_relative_paths() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("vault.toml");
    let config = VaultConfig::new(
        "v".to_string(),
        ObjectId::default(),
        OriginConfig::Fs {
            root: "data".into(),
        },
    );

    config.save(&path).unwrap();

    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("root = \"data\""), "{saved}");
}

/// A command config whose every template is `template`, with `extras`.
fn command_config(template: &str, extras: &[(&str, &str)]) -> OriginConfig {
    OriginConfig::Command {
        list_cmd: template.to_string(),
        fetch_cmd: template.to_string(),
        get_cmd: template.to_string(),
        put_cmd: template.to_string(),
        send_cmd: template.to_string(),
        delete_cmd: template.to_string(),
        extras: Some(
            extras
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ),
        rename_cmd: None,
    }
}

#[cfg(unix)]
fn non_utf8_dir() -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(b"/srv/vault-\xff"))
}

#[cfg(unix)]
#[test]
fn a_non_utf8_config_dir_is_fine_when_nothing_uses_config_dir() {
    let config = command_config("true", &[("root", "/srv/data")])
        .relative_to(&non_utf8_dir())
        .unwrap();
    match config {
        OriginConfig::Command { extras, .. } => {
            let extras = extras.unwrap();
            assert_eq!(extras.get("root").map(String::as_str), Some("/srv/data"));
            assert!(!extras.contains_key("config_dir"));
        }
        _ => panic!("expected a command config"),
    }
}

#[cfg(unix)]
#[test]
fn a_non_utf8_config_dir_is_refused_when_config_dir_is_used() {
    for config in [
        command_config("ls {config_dir}", &[]),
        command_config("ls {root}", &[("root", "{config_dir}/data")]),
    ] {
        let error = match config.relative_to(&non_utf8_dir()) {
            Ok(_) => panic!("a non-UTF-8 {{config_dir}} should be refused"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("UTF-8"), "{error}");
    }
}

/// The three ways a token line can be malformed, each with the secret it must not echo.
const MALFORMED_TOKEN_LINES: [(&str, &str); 3] = [
    ("token = \"SUPERSECRET-123\" extra", "SUPERSECRET-123"),
    ("token = 12345678", "12345678"),
    (
        "token = \"t\"\ntoken_env = [\"SUPERSECRET-456\"]",
        "SUPERSECRET-456",
    ),
];

#[test]
fn a_malformed_token_line_is_never_echoed_by_the_loader() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.toml");
    for (line, secret) in MALFORMED_TOKEN_LINES {
        std::fs::write(
            &path,
            format!(
                "name = \"remote\"\n\n[origin_config]\ntype = \"http\"\n\
                 list_url = \"/l\"\nfetch_url = \"/f\"\nget_url = \"/g\"\nput_url = \"/p\"\n\
                 send_url = \"/s\"\ndelete_url = \"/d\"\n\n\
                 [origin_config.auth]\ntype = \"bearer\"\n{line}\n"
            ),
        )
        .unwrap();
        let error = match VaultConfig::build(path.clone()) {
            Ok(_) => panic!("{line:?} should not parse"),
            Err(error) => error,
        };
        assert!(matches!(error, VaultError::Toml(_)), "{error:?}");
        for shown in [
            error.to_string(),
            format!("{error:?}"),
            format!("{error:#}"),
        ] {
            assert!(!shown.contains(secret), "{line:?} leaked: {shown}");
            assert!(shown.contains("vault.toml"), "{shown}");
        }
    }
}

// --- crypt ---

/// Writes a fresh key's text form to `dir/name` with `mode`, returning the text.
fn write_key_file(dir: &Path, name: &str, mode: u32) -> String {
    let text = crate::crypt::CryptKey::generate()
        .unwrap()
        .to_text()
        .to_string();
    let path = dir.join(name);
    std::fs::write(&path, format!("{text}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = mode;
    text
}

/// A crypt vault config in `dir` whose key line is `key_line` and whose inner fs root is
/// `data` (relative), returning its path.
fn write_crypt_config(dir: &Path, key_line: &str) -> PathBuf {
    std::fs::create_dir_all(dir.join("data")).unwrap();
    let path = dir.join("secure.toml");
    std::fs::write(
        &path,
        format!(
            "name = \"secure\"\n\n[origin_config]\ntype = \"crypt\"\n{key_line}\n\n\
             [origin_config.inner]\ntype = \"fs\"\nroot = \"data\"\n"
        ),
    )
    .unwrap();
    path
}

#[tokio::test]
async fn a_crypt_config_builds_with_paths_relative_to_it() {
    let dir = tempfile::tempdir().unwrap();
    write_key_file(dir.path(), "secure.key", 0o600);
    let config = write_crypt_config(dir.path(), "key_file = \"secure.key\"");

    let (name, _, origin) = VaultConfig::build(config).unwrap();
    assert_eq!(name, "secure");
    let mut leaf = crate::object::Object::Leaf {
        name: "n.txt".into(),
        id: "n.txt".into(),
        meta: Default::default(),
    };
    let stored = origin.put(&mut leaf, &ObjectId::from("/")).await.unwrap();
    origin
        .send(
            &stored,
            futures::StreamExt::boxed(futures::stream::iter([Ok(bytes::Bytes::from_static(
                b"PLAINTEXT-IN-DATA",
            ))])),
        )
        .await
        .unwrap();
    // The inner root resolved to dir/data, and holds ciphertext.
    let stored_bytes = std::fs::read(dir.path().join("data/n.txt")).unwrap();
    assert!(!stored_bytes.windows(17).any(|w| w == b"PLAINTEXT-IN-DATA"));
    assert!(dir.path().join("data/.nimbus-crypt").is_file());
}

#[test]
fn a_crypt_key_can_come_from_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    let text = crate::crypt::CryptKey::generate()
        .unwrap()
        .to_text()
        .to_string();
    let name = "NIMBUS_TEST_CRYPT_KEY_FROM_ENV";
    unsafe { std::env::set_var(name, &text) };
    let config = write_crypt_config(dir.path(), &format!("key_env = \"{name}\""));
    let built = VaultConfig::build(config);
    unsafe { std::env::remove_var(name) };
    assert!(built.is_ok());
}

#[test]
fn a_crypt_config_needs_exactly_one_key_source() {
    let dir = tempfile::tempdir().unwrap();
    write_key_file(dir.path(), "secure.key", 0o600);
    for (line, expected) in [
        ("", "set one of key_file or key_env"),
        (
            "key_file = \"secure.key\"\nkey_env = \"NIMBUS_TEST_UNUSED\"",
            "only one",
        ),
    ] {
        let config = write_crypt_config(dir.path(), line);
        let error = match VaultConfig::build(config) {
            Ok(_) => panic!("{line:?} should be refused"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains(expected), "{line:?}: {error}");
    }
}

#[test]
fn a_bad_key_is_refused_without_showing_it() {
    let dir = tempfile::tempdir().unwrap();
    let body = "SECRETKEYMATERIAL0123456789abcdefghijklmnopq";
    std::fs::write(
        dir.path().join("bad.key"),
        format!("nimbus-crypt-key-1:{body}!"),
    )
    .unwrap();
    let config = write_crypt_config(dir.path(), "key_file = \"bad.key\"");
    let error = match VaultConfig::build(config) {
        Ok(_) => panic!("a malformed key should be refused"),
        Err(error) => error,
    };
    for shown in [
        error.to_string(),
        format!("{error:?}"),
        format!("{error:#}"),
    ] {
        assert!(!shown.contains("SECRETKEYMATERIAL"), "{shown}");
        assert!(shown.contains("bad.key"), "{shown}");
    }

    // A key pasted inline, as an unknown field, isn't echoed by the parse error either.
    let config = write_crypt_config(dir.path(), &format!("key = \"nimbus-crypt-key-1:{body}\""));
    let error = match VaultConfig::build(config) {
        Ok(_) => panic!("an inline key isn't a field"),
        Err(error) => error,
    };
    for shown in [error.to_string(), format!("{error:?}")] {
        assert!(!shown.contains("SECRETKEYMATERIAL"), "{shown}");
    }
}

#[test]
fn crypt_relative_to_resolves_the_key_file_and_the_inner_origin() {
    let config = OriginConfig::Crypt {
        key_file: Some("keys/v.key".into()),
        key_env: None,
        inner: Box::new(OriginConfig::Fs {
            root: "data".into(),
        }),
    };
    match config.relative_to(Path::new("/etc/nimbus")).unwrap() {
        OriginConfig::Crypt {
            key_file, inner, ..
        } => {
            assert_eq!(key_file, Some(PathBuf::from("/etc/nimbus/keys/v.key")));
            assert!(
                matches!(*inner, OriginConfig::Fs { ref root } if root == Path::new("/etc/nimbus/data"))
            );
        }
        _ => panic!("expected a crypt config"),
    }
}

/// An http origin config with `auth` and placeholder urls.
fn http_with_auth(auth: crate::origin::http::HttpAuth) -> OriginConfig {
    OriginConfig::Http {
        base_url: Some("https://example.com".to_string()),
        list_url: "/list/{id}".to_string(),
        fetch_url: "/fetch/{id}".to_string(),
        get_url: "/get/{id}".to_string(),
        put_url: "/put/{id}".to_string(),
        send_url: "/send/{id}".to_string(),
        delete_url: "/delete/{id}".to_string(),
        rename_url: None,
        tls: None,
        auth,
    }
}

#[cfg(unix)]
#[test]
fn a_token_inside_a_crypt_origin_still_makes_the_config_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.toml");
    let crypt = |auth| OriginConfig::Crypt {
        key_file: Some("v.key".into()),
        key_env: None,
        inner: Box::new(http_with_auth(auth)),
    };
    VaultConfig::new(
        "v".to_string(),
        ObjectId::default(),
        crypt(crate::origin::http::HttpAuth::Bearer {
            token: Some("hunter2".into()),
            token_env: None,
            token_file: None,
        }),
    )
    .save(&path)
    .unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    // And it round-trips, the inner table after the key line.
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.find("key_file").unwrap() < written.find("[origin_config.inner]").unwrap());
    let read_back: VaultConfig = toml::from_str(&written).unwrap();
    assert!(matches!(
        read_back.origin_config,
        OriginConfig::Crypt { .. }
    ));
}

/// The advisories a vault opened from `config` reports.
fn advisories_of(config: &Path) -> Vec<ConfigAdvisory> {
    crate::vault::Vault::new(config.to_path_buf())
        .unwrap()
        .advisories()
        .to_vec()
}

#[cfg(unix)]
#[test]
fn advisories_flag_an_encrypted_vault_and_an_exposed_key_file() {
    let dir = tempfile::tempdir().unwrap();
    write_key_file(dir.path(), "private.key", 0o600);
    write_key_file(dir.path(), "shared.key", 0o644);

    let private = write_crypt_config(dir.path(), "key_file = \"private.key\"");
    assert_eq!(advisories_of(&private), vec![ConfigAdvisory::Encrypted]);
    let shared = write_crypt_config(dir.path(), "key_file = \"shared.key\"");
    assert_eq!(
        advisories_of(&shared),
        vec![
            ConfigAdvisory::Encrypted,
            ConfigAdvisory::KeyFileExposed {
                path: dir.path().join("shared.key"),
                mode: 0o644
            }
        ]
    );
    let plain = dir.path().join("plain.toml");
    std::fs::write(
        &plain,
        "name = \"p\"\n[origin_config]\ntype = \"fs\"\nroot = \"data\"\n",
    )
    .unwrap();
    assert!(advisories_of(&plain).is_empty());
}

#[cfg(unix)]
#[test]
fn advisories_come_through_a_vault_wrapping_a_crypt_vault() {
    let dir = tempfile::tempdir().unwrap();
    write_key_file(dir.path(), "shared.key", 0o644);
    write_crypt_config(dir.path(), "key_file = \"shared.key\"");
    let outer = dir.path().join("outer.toml");
    std::fs::write(
        &outer,
        "name = \"outer\"\n\n[origin_config]\ntype = \"vault\"\npath = \"secure.toml\"\n",
    )
    .unwrap();
    assert_eq!(
        advisories_of(&outer),
        vec![
            ConfigAdvisory::Encrypted,
            ConfigAdvisory::KeyFileExposed {
                path: dir.path().join("shared.key"),
                mode: 0o644
            }
        ]
    );
}

#[test]
fn vault_configs_pointing_at_each_other_are_refused_not_followed_forever() {
    let dir = tempfile::tempdir().unwrap();
    for (name, target) in [("a", "b"), ("b", "a")] {
        std::fs::write(
            dir.path().join(format!("{name}.toml")),
            format!(
                "name = \"{name}\"\n\n[origin_config]\ntype = \"vault\"\npath = \"{target}.toml\"\n"
            ),
        )
        .unwrap();
    }
    let error = match crate::vault::Vault::new(dir.path().join("a.toml")) {
        Ok(_) => panic!("a cycle of configs should not open"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("nested more than"), "{error}");
}
