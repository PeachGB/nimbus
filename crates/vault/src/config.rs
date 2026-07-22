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

    /// Builds a `VaultConfig` from its parts, without reading anything from disk. Useful for
    /// tooling that constructs a config programmatically (e.g. an interactive vault-creation
    /// wizard) and wants to serialize it out via [`VaultConfig::save`].
    pub fn new(name: String, root_id: ObjectId, origin_config: OriginConfig) -> Self {
        VaultConfig {
            name,
            root_id,
            origin_config,
        }
    }

    /// Serializes this config to TOML and writes it to `path`.
    pub fn save(&self, path: &std::path::Path) -> VaultResult<()> {
        let toml = toml::to_string_pretty(self)?;
        std::fs::write(path, toml)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;

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
