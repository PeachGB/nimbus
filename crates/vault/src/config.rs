use std::{collections::HashMap, path::PathBuf};
use toml;

use serde::{Deserialize, Serialize};

use crate::{
    VaultResult,
    object::ObjectId,
    origin::{Origin, command::OriginCommand, fs::OriginFileSystem, http::OriginHTTP},
};

#[derive(Serialize, Deserialize)]
pub struct VaultConfig {
    name: String,
    #[serde(default)]
    root_id: ObjectId,
    origin_config: OriginConfig,
}
impl VaultConfig {
    pub fn build(from: PathBuf) -> VaultResult<(String, ObjectId, Box<dyn Origin>)> {
        let cfg = std::fs::read_to_string(from)?;
        let cfg: VaultConfig = toml::from_str(&cfg)?;
        let origin = cfg.origin_config.build();
        Ok((cfg.name, cfg.root_id, origin))
    }
}

#[cfg(test)]
mod tests {
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
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OriginConfig {
    Command {
        list_cmd: String,
        fetch_cmd: String,
        get_cmd: String,

        put_cmd: String,
        send_cmd: String,

        delete_cmd: String,
        extras: Option<HashMap<String, String>>,
    },
    Fs {
        root: PathBuf,
    },
    Http {
        base_url: Option<String>,
        list_url: String,
        fetch_url: String,
        get_url: String,

        put_url: String,
        send_url: String,

        delete_url: String,
    },
}
impl OriginConfig {
    /// Reads an `OriginConfig` from the TOML file at `from` and builds the `Origin` it
    /// describes, without requiring a `name`/`root_id`/full `VaultConfig` wrapper. Useful for
    /// tooling that only needs to talk to an origin directly (e.g. `push`/`pull` between two
    /// origins) rather than opening a full `Vault`.
    pub fn from_file(from: PathBuf) -> VaultResult<Box<dyn Origin>> {
        let cfg = std::fs::read_to_string(from)?;
        let cfg: OriginConfig = toml::from_str(&cfg)?;
        Ok(cfg.build())
    }
    /// Constructs the `Origin` described by this config.
    pub fn build(&self) -> Box<dyn Origin> {
        match self {
            OriginConfig::Command {
                list_cmd,
                fetch_cmd,
                get_cmd,
                put_cmd,
                send_cmd,
                delete_cmd,
                extras,
            } => Box::new(OriginCommand::new(
                fetch_cmd.clone(),
                list_cmd.clone(),
                get_cmd.clone(),
                put_cmd.clone(),
                send_cmd.clone(),
                delete_cmd.clone(),
                extras.clone(),
            )),
            OriginConfig::Fs { root } => Box::new(OriginFileSystem::new(root.clone())),
            OriginConfig::Http {
                base_url,
                list_url,
                fetch_url,
                get_url,
                put_url,
                send_url,
                delete_url,
            } => {
                let base_url = base_url.clone().unwrap_or_default();
                Box::new(OriginHTTP::new(
                    base_url,
                    fetch_url.clone(),
                    list_url.clone(),
                    get_url.clone(),
                    put_url.clone(),
                    send_url.clone(),
                    delete_url.clone(),
                ))
            }
        }
    }
}
