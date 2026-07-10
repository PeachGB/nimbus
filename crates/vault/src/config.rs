use std::{collections::HashMap, path::PathBuf, sync::Arc};
use toml;

use serde::{Deserialize, Serialize};

use crate::{
    VaultResult,
    object::ObjectId,
    origin::{
        Origin, command::OriginCommand, fs::OriginFileSystem, http::OriginHTTP, vault::OriginVault,
    },
    vault::Vault,
};

/// The on-disk (TOML) shape of a [`crate::vault::Vault`]: a `name`, an optional `root_id`
/// (defaulting to [`ObjectId::default`], `"/"`), and the [`OriginConfig`] describing how to
/// build its origin.
///
/// # Examples
///
/// ```toml
/// # vault.toml
/// name = "my-vault"
/// # root_id defaults to "/" if omitted
///
/// [origin_config]
/// type = "fs"
/// root = "/srv/data"
/// ```
///
/// Most callers won't touch `VaultConfig` directly — [`crate::vault::Vault::new`] reads and
/// builds one in a single step. See [`VaultConfig::build`] for the lower-level entry point.
#[derive(Serialize, Deserialize)]
pub struct VaultConfig {
    name: String,
    #[serde(default)]
    root_id: ObjectId,
    origin_config: OriginConfig,
}
impl VaultConfig {
    /// Reads a `VaultConfig` from the TOML file at `from` and builds the `Vault` it describes,
    /// returning its name, root id, and the `Box<dyn Origin>` built from `origin_config`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::fs;
    /// use nimbus_vault::config::VaultConfig;
    ///
    /// let dir = tempfile::tempdir()?;
    /// let data_dir = dir.path().join("data");
    /// fs::create_dir_all(&data_dir)?;
    ///
    /// let config_path = dir.path().join("vault.toml");
    /// fs::write(
    ///     &config_path,
    ///     format!(
    ///         "name = \"my-vault\"\n\n[origin_config]\ntype = \"fs\"\nroot = \"{}\"\n",
    ///         data_dir.display(),
    ///     ),
    /// )?;
    ///
    /// let (name, root_id, _origin) = VaultConfig::build(config_path)?;
    /// assert_eq!(name, "my-vault");
    /// assert_eq!(root_id.as_str(), "/"); // default, since root_id was omitted
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn build(from: PathBuf) -> VaultResult<(String, ObjectId, Box<dyn Origin>)> {
        let cfg = std::fs::read_to_string(from)?;
        let cfg: VaultConfig = toml::from_str(&cfg)?;
        let origin = cfg.origin_config.build()?;
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
}

/// The on-disk (TOML) shape of an [`crate::origin::Origin`], tagged by `type` (`"fs"`,
/// `"command"`, `"http"`, or `"vault"`). Deserialized from the `[origin_config]` table of a
/// `VaultConfig`, or standalone via [`OriginConfig::from_file`].
///
/// # Examples
///
/// A directory on disk:
///
/// ```toml
/// type = "fs"
/// root = "/srv/data"
/// ```
///
/// A shell command per operation:
///
/// ```toml
/// type = "command"
/// list_cmd   = "ls {root}"
/// fetch_cmd  = "cat {root}/{id}"
/// get_cmd    = "stat {root}/{id}"
/// put_cmd    = "touch {root}/{id}"
/// send_cmd   = "tee {root}/{id}"
/// delete_cmd = "rm {root}/{id}"
///
/// [extras]
/// root = "/srv/data"
/// ```
///
/// A REST-ish HTTP API:
///
/// ```toml
/// type = "http"
/// base_url   = "https://example.com"
/// list_url   = "/list/{id}"
/// fetch_url  = "/fetch/{id}"
/// get_url    = "/get/{id}"
/// put_url    = "/put/{id}"
/// send_url   = "/send/{id}"
/// delete_url = "/delete/{id}"
/// ```
///
/// Another vault, opened from its own config file:
///
/// ```toml
/// type = "vault"
/// path = "inner.toml"
/// ```
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OriginConfig {
    /// Builds an [`crate::origin::command::OriginCommand`]: an origin backed by a shell
    /// command per operation.
    Command {
        /// Template run to list a directory-like `id`'s children.
        list_cmd: String,
        /// Template run to stream an object's payload to stdout.
        fetch_cmd: String,
        /// Template run to fetch a single object's metadata as JSON on stdout.
        get_cmd: String,

        /// Template run to write an object's metadata (without payload).
        put_cmd: String,
        /// Template run to stream an object's payload from stdin.
        send_cmd: String,

        /// Template run to delete an object.
        delete_cmd: String,
        /// Extra `{placeholder}` substitutions available to every command template, beyond
        /// the object id/name/metadata.
        extras: Option<HashMap<String, String>>,
    },
    /// Builds an [`crate::origin::fs::OriginFileSystem`]: an origin backed by a directory on
    /// disk.
    Fs {
        /// Directory that `ObjectId`s are resolved relative to.
        root: PathBuf,
    },
    /// Builds an [`crate::origin::http::OriginHTTP`]: an origin backed by a REST-ish HTTP API.
    Http {
        /// Prefix prepended to every templated URL below; defaults to empty if omitted.
        base_url: Option<String>,
        /// `{id}`-templated path to list a directory-like `id`'s children.
        list_url: String,
        /// `{id}`-templated path to fetch an object's payload.
        fetch_url: String,
        /// `{id}`-templated path to fetch an object's metadata.
        get_url: String,

        /// `{id}`-templated path to write an object's metadata.
        put_url: String,
        /// `{id}`-templated path to write an object's payload.
        send_url: String,

        /// `{id}`-templated path to delete an object.
        delete_url: String,
    },
    /// Builds an [`crate::origin::vault::OriginVault`]: an origin backed by another `Vault`,
    /// opened from its own config file.
    Vault {
        /// Path to the inner vault's own `VaultConfig` TOML file.
        path: PathBuf,
    },
}
impl OriginConfig {
    /// Reads an `OriginConfig` from the TOML file at `from` and builds the `Origin` it
    /// describes, without requiring a `name`/`root_id`/full `VaultConfig` wrapper. Useful for
    /// tooling that only needs to talk to an origin directly (e.g. `push`/`pull` between two
    /// origins) rather than opening a full `Vault`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::fs;
    /// use nimbus_vault::config::OriginConfig;
    ///
    /// let dir = tempfile::tempdir()?;
    /// let config_path = dir.path().join("origin.toml");
    /// fs::write(&config_path, "type = \"fs\"\nroot = \"/srv/data\"\n")?;
    ///
    /// let origin = OriginConfig::from_file(config_path)?;
    /// // `origin` is a `Box<dyn Origin>`, ready to pass to `Vault::pull`/`Vault::push`
    /// // or to call `fetch`/`list`/`get`/`put`/`send`/`delete` on directly.
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_file(from: PathBuf) -> VaultResult<Box<dyn Origin>> {
        let cfg = std::fs::read_to_string(from)?;
        let cfg: OriginConfig = toml::from_str(&cfg)?;
        cfg.build()
    }
    /// Constructs the `Origin` described by this config.
    ///
    /// # Examples
    ///
    /// ```
    /// use nimbus_vault::config::OriginConfig;
    ///
    /// let config = OriginConfig::Fs { root: "/srv/data".into() };
    /// let origin = config.build()?; // consumes `config`, moving its fields in
    /// # Ok::<(), nimbus_vault::error::VaultError>(())
    /// ```
    pub fn build(self) -> VaultResult<Box<dyn Origin>> {
        match self {
            OriginConfig::Command {
                list_cmd,
                fetch_cmd,
                get_cmd,
                put_cmd,
                send_cmd,
                delete_cmd,
                extras,
            } => Ok(Box::new(OriginCommand::new(
                fetch_cmd, list_cmd, get_cmd, put_cmd, send_cmd, delete_cmd, extras,
            ))),
            OriginConfig::Fs { root } => Ok(Box::new(OriginFileSystem::new(root))),
            OriginConfig::Http {
                base_url,
                list_url,
                fetch_url,
                get_url,
                put_url,
                send_url,
                delete_url,
            } => {
                let base_url = base_url.unwrap_or_default();
                Ok(Box::new(OriginHTTP::new(
                    base_url, fetch_url, list_url, get_url, put_url, send_url, delete_url,
                )))
            }
            OriginConfig::Vault { path } => {
                let vault = Vault::new(path)?;
                Ok(Box::new(OriginVault::new(Arc::new(vault))))
            }
        }
    }
}
