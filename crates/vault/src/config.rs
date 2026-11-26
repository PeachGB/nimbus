use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use toml;

use serde::{Deserialize, Serialize};

use crate::{
    VaultResult,
    crypt::CryptKey,
    error::VaultError,
    fsutil,
    object::ObjectId,
    origin::{
        Origin,
        command::OriginCommand,
        crypt::OriginCrypt,
        fs::OriginFileSystem,
        http::{HttpAuth, HttpTls, OriginHTTP},
        vault::OriginVault,
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
    /// Relative paths in the file resolve against the file's own directory (see
    /// [`OriginConfig::relative_to`]), so the result doesn't depend on the current directory.
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
        let loaded = Self::load(&from, 0)?;
        Ok((loaded.name, loaded.root_id, loaded.origin))
    }

    /// [`VaultConfig::build`], also returning the advisories gathered while building, at
    /// nesting `depth` (how many `vault` origins lead here). Too deep is an error: it is almost
    /// certainly a cycle of configs pointing at each other.
    pub(crate) fn load(from: &Path, depth: usize) -> VaultResult<LoadedVault> {
        if depth > MAX_NESTING {
            return Err(VaultError::Generic(format!(
                "{}: vault configs nested more than {MAX_NESTING} deep (do they point at each \
                 other?)",
                from.display()
            )));
        }
        let dir = fsutil::config_dir(from)?;
        let cfg = std::fs::read_to_string(from)?;
        let cfg: VaultConfig = crate::parse::parse_toml(&cfg, from)?;
        let mut advisories = Vec::new();
        let origin = cfg
            .origin_config
            .relative_to(&dir)?
            .build_nested(&mut advisories, depth)?;
        Ok(LoadedVault {
            name: cfg.name,
            root_id: cfg.root_id,
            origin,
            advisories,
        })
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
        // Atomic, so an interrupted save can't leave a truncated config that no longer parses.
        // Missing parent directories (the conventional one, see [`Self::default_dir`], doesn't
        // exist until the first vault is created) are made on the way.
        crate::fsutil::atomic_write(path, toml.as_bytes(), self.file_mode())?;
        Ok(())
    }

    /// `Some(0o600)` when the config holds a credential in plain text (an HTTP `token`), so
    /// other users on the machine can't read it. `None` keeps the default permissions.
    fn file_mode(&self) -> Option<u32> {
        self.origin_config.holds_secret().then_some(0o600)
    }

    /// Directory new vault configs are written to by default: `vaults/` under
    /// [`crate::config_home`]. Keeping them together is what lets a vault be found again
    /// later without remembering where it was saved.
    ///
    /// # Examples
    ///
    /// ```
    /// let dir = nimbus_vault::config::VaultConfig::default_dir();
    /// assert!(dir.ends_with("vaults"));
    /// ```
    pub fn default_dir() -> PathBuf {
        crate::config_home().join("vaults")
    }

    /// The default config path for a vault called `name`, inside [`Self::default_dir`].
    ///
    /// # Examples
    ///
    /// ```
    /// let path = nimbus_vault::config::VaultConfig::default_path("backup");
    /// assert!(path.ends_with("backup.toml"));
    /// ```
    pub fn default_path(name: &str) -> PathBuf {
        Self::default_dir().join(format!("{name}.toml"))
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
/// A shell command per operation, with its data next to the config file (`{config_dir}`, see
/// [`OriginConfig::relative_to`]):
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
/// root = "{config_dir}/data"
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
///
/// # optional; see `HttpAuth`
/// [auth]
/// type = "bearer"
/// token_env = "NIMBUS_TOKEN"
/// ```
///
/// Another vault, opened from its own config file:
///
/// ```toml
/// type = "vault"
/// path = "inner.toml"
/// ```
///
/// Marked `#[non_exhaustive]`: new origin types are added over time, so a `match` outside this
/// crate needs a wildcard arm.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[non_exhaustive]
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
        /// the object id/name/metadata. Loaded from a file, `{config_dir}` is also available,
        /// and is the one placeholder expanded inside these values.
        extras: Option<HashMap<String, String>>,
        /// Optional template for a native move (`{id}`, `{destination}`, `{name}`); see
        /// `OriginCommand::with_rename_cmd`. Without it, moves copy then delete.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rename_cmd: Option<String>,
    },
    /// Builds an [`crate::origin::fs::OriginFileSystem`]: an origin backed by a directory on
    /// disk.
    Fs {
        /// Directory that `ObjectId`s are resolved relative to. Relative to the config file's
        /// directory when loaded from a file.
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
        /// Optional `{id}`-templated path for a native move; see
        /// `OriginHTTP::with_rename_url`. Without it, moves copy then delete.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rename_url: Option<String>,
        /// Credentials to present on every request; defaults to sending none. It and `tls`
        /// come last because they serialize as TOML tables, and a table has to come after
        /// every plain value in the same struct.
        #[serde(default)]
        auth: HttpAuth,
        /// How to trust an https server no public CA vouches for (`[origin_config.tls]`); see
        /// `HttpTls`. Omitted means the system's public roots, as before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<HttpTls>,
    },
    /// Builds an [`crate::origin::vault::OriginVault`]: an origin backed by another `Vault`,
    /// opened from its own config file.
    Vault {
        /// Path to the inner vault's own `VaultConfig` TOML file. Relative to the config file's
        /// directory when loaded from a file.
        path: PathBuf,
    },
    /// Builds an [`crate::origin::crypt::OriginCrypt`]: every object of the `inner` origin
    /// encrypted at rest with a key that never leaves this machine (see [`crate::crypt`]).
    Crypt {
        /// File holding the key's text form (`nimbus-crypt-key-1:…`), relative to the config's
        /// directory. Exactly one of this or `key_env`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_file: Option<PathBuf>,
        /// Environment variable holding the key's text form. Exactly one of this or `key_file`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_env: Option<String>,
        /// The origin the ciphertext is stored in. Last, because it serializes as a TOML table
        /// (`[origin_config.inner]`) and a table has to come after every plain value.
        inner: Box<OriginConfig>,
    },
}
impl OriginConfig {
    /// Reads an `OriginConfig` from the TOML file at `from` and builds the `Origin` it
    /// describes, without requiring a `name`/`root_id`/full `VaultConfig` wrapper. Useful for
    /// tooling that only needs to talk to an origin directly (e.g. `push`/`pull` between two
    /// origins) rather than opening a full `Vault`. Relative paths resolve against the file's
    /// own directory, as in [`VaultConfig::build`].
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
        let dir = fsutil::config_dir(&from)?;
        let cfg = std::fs::read_to_string(&from)?;
        let cfg: OriginConfig = crate::parse::parse_toml(&cfg, &from)?;
        cfg.relative_to(&dir)?.build()
    }

    /// Resolves this config's relative paths against `dir` (normally [`fsutil::config_dir`] of
    /// the file it came from). Absolute paths are untouched. Per variant: the `fs` root, the
    /// `vault` path (whose own config then resolves against *its* directory), the `http`
    /// `token_file` and `ca_file`, and for `command` the `{config_dir}` placeholder, the one
    /// placeholder also expanded inside `extras` values. A user extra named `config_dir` is
    /// refused.
    ///
    /// Called on the parsed value right before it is built; the config that would be saved is
    /// never changed, so saving can't rewrite a path.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use nimbus_vault::config::OriginConfig;
    ///
    /// let config = OriginConfig::Fs { root: "data".into() };
    /// match config.relative_to(Path::new("/etc/nimbus"))? {
    ///     OriginConfig::Fs { root } => assert_eq!(root, Path::new("/etc/nimbus/data")),
    ///     _ => unreachable!(),
    /// }
    /// # Ok::<(), nimbus_vault::error::VaultError>(())
    /// ```
    pub fn relative_to(self, dir: &Path) -> VaultResult<Self> {
        Ok(match self {
            OriginConfig::Fs { root } => OriginConfig::Fs {
                root: fsutil::resolve_against(dir, root),
            },
            OriginConfig::Vault { path } => OriginConfig::Vault {
                path: fsutil::resolve_against(dir, path),
            },
            OriginConfig::Crypt {
                key_file,
                key_env,
                inner,
            } => OriginConfig::Crypt {
                key_file: key_file.map(|path| fsutil::resolve_against(dir, path)),
                key_env,
                inner: Box::new(inner.relative_to(dir)?),
            },
            OriginConfig::Http {
                auth,
                tls,
                base_url,
                list_url,
                fetch_url,
                get_url,
                put_url,
                send_url,
                delete_url,
                rename_url,
            } => OriginConfig::Http {
                auth: auth.relative_to(dir),
                tls: tls.map(|tls| tls.relative_to(dir)),
                base_url,
                list_url,
                fetch_url,
                get_url,
                put_url,
                send_url,
                delete_url,
                rename_url,
            },
            OriginConfig::Command {
                extras,
                list_cmd,
                fetch_cmd,
                get_cmd,
                put_cmd,
                send_cmd,
                delete_cmd,
                rename_cmd,
            } => {
                let templates = [
                    &list_cmd,
                    &fetch_cmd,
                    &get_cmd,
                    &put_cmd,
                    &send_cmd,
                    &delete_cmd,
                ];
                let in_templates = templates
                    .into_iter()
                    .chain(rename_cmd.as_ref())
                    .any(|template| template.contains(CONFIG_DIR_PLACEHOLDER));
                OriginConfig::Command {
                    extras: Some(with_config_dir(extras, dir, in_templates)?),
                    list_cmd,
                    fetch_cmd,
                    get_cmd,
                    put_cmd,
                    send_cmd,
                    delete_cmd,
                    rename_cmd,
                }
            }
        })
    }

    /// Constructs the `Origin` described by this config.
    ///
    /// Paths are used as given: a relative one resolves against the current directory. Only
    /// configs read from a file are resolved against that file's directory (see
    /// [`OriginConfig::relative_to`]), so a programmatic config keeps its meaning.
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
        self.build_nested(&mut Vec::new(), 0)
    }

    /// [`OriginConfig::build`], appending to `advisories` what is worth telling the user about
    /// this origin and everything it wraps, nested vault configs included. `depth` counts the
    /// `vault` origins that led here (see [`VaultConfig::load`]).
    fn build_nested(
        self,
        advisories: &mut Vec<ConfigAdvisory>,
        depth: usize,
    ) -> VaultResult<Box<dyn Origin>> {
        match self {
            OriginConfig::Command {
                list_cmd,
                fetch_cmd,
                get_cmd,
                put_cmd,
                send_cmd,
                delete_cmd,
                extras,
                rename_cmd,
            } => Ok(Box::new(
                OriginCommand::new(
                    fetch_cmd, list_cmd, get_cmd, put_cmd, send_cmd, delete_cmd, extras,
                )
                .with_rename_cmd(rename_cmd),
            )),
            OriginConfig::Fs { root } => Ok(Box::new(OriginFileSystem::new(root))),
            OriginConfig::Http {
                base_url,
                list_url,
                fetch_url,
                get_url,
                put_url,
                send_url,
                delete_url,
                rename_url,
                auth,
                tls,
            } => {
                let base_url = base_url.unwrap_or_default();
                let origin = OriginHTTP::new(
                    base_url, fetch_url, list_url, get_url, put_url, send_url, delete_url,
                )
                .with_rename_url(rename_url)?
                .with_auth(&auth)?;
                // Checked when the vault opens, like `auth`: a bad tls table fails here.
                Ok(Box::new(match tls {
                    Some(tls) => origin.with_tls(&tls)?,
                    None => origin,
                }))
            }
            OriginConfig::Vault { path } => {
                let vault = Vault::open_nested(&path, depth + 1)?;
                advisories.extend(vault.advisories().iter().cloned());
                Ok(Box::new(OriginVault::new(Arc::new(vault))))
            }
            OriginConfig::Crypt {
                key_file,
                key_env,
                inner,
            } => {
                let key = CryptKey::load(key_file.as_deref(), key_env.as_deref())?;
                advisories.push(ConfigAdvisory::Encrypted);
                if let Some(path) = key_file
                    && let Some(mode) = key_file_exposed(&path)
                {
                    advisories.push(ConfigAdvisory::KeyFileExposed { path, mode });
                }
                let inner = inner.build_nested(advisories, depth)?;
                Ok(Box::new(OriginCrypt::new(inner, key)))
            }
        }
    }
}

/// The extras key, and so the `{config_dir}` placeholder, set to a command config's directory.
const CONFIG_DIR_KEY: &str = "config_dir";
/// [`CONFIG_DIR_KEY`] as it appears in a template or an extras value.
const CONFIG_DIR_PLACEHOLDER: &str = "{config_dir}";

/// The extras an `OriginCommand` loaded from a file gets: the configured ones with
/// `{config_dir}` expanded, plus `config_dir` itself, set to `dir`. This is the one expansion
/// point for `{config_dir}`, and the only placeholder expanded inside an extras value.
///
/// The directory goes into extras values raw, not quoted: a whole extras value is shell-quoted
/// later, when a template uses it. `{config_dir}` in a template is quoted the same way, like
/// every placeholder. A user extra named `config_dir` is refused. Extras are strings, so a
/// directory that isn't UTF-8 can't be one: that is an error only when the config uses
/// `{config_dir}` (`in_templates`, or inside an extras value); otherwise `config_dir` is just
/// left unset, so a vault that never asks for it opens wherever it lives.
fn with_config_dir(
    extras: Option<HashMap<String, String>>,
    dir: &Path,
    in_templates: bool,
) -> VaultResult<HashMap<String, String>> {
    let mut extras = extras.unwrap_or_default();
    if extras.contains_key(CONFIG_DIR_KEY) {
        return Err(VaultError::Generic(format!(
            "extras: `{CONFIG_DIR_KEY}` is reserved; nimbus sets it to the config file's directory"
        )));
    }
    let used = in_templates
        || extras
            .values()
            .any(|value| value.contains(CONFIG_DIR_PLACEHOLDER));
    let Some(dir) = dir.to_str() else {
        if used {
            return Err(VaultError::Generic(format!(
                "{CONFIG_DIR_PLACEHOLDER}: the config directory {} isn't valid UTF-8",
                dir.display()
            )));
        }
        return Ok(extras);
    };
    for value in extras.values_mut() {
        *value = value.replace(CONFIG_DIR_PLACEHOLDER, dir);
    }
    extras.insert(CONFIG_DIR_KEY.to_string(), dir.to_string());
    Ok(extras)
}

impl OriginConfig {
    /// Whether this config, or one nested in it, holds a credential in plain text (an inline
    /// HTTP `token`), so a file containing it is written `0600`. A crypt origin never holds its
    /// key inline, but its inner origin might hold a token.
    fn holds_secret(&self) -> bool {
        match self {
            OriginConfig::Http {
                auth: HttpAuth::Bearer { token: Some(_), .. },
                ..
            } => true,
            OriginConfig::Crypt { inner, .. } => inner.holds_secret(),
            _ => false,
        }
    }
}

/// Something about a vault config worth telling the user that doesn't stop it from opening,
/// gathered while the vault is built (nested vault configs included); see
/// [`crate::vault::Vault::advisories`]. Each program decides which ones to show.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigAdvisory {
    /// The vault is encrypted at rest and this process holds its key, so it reads and writes
    /// plaintext. Expected on a client; on a server it means the server can read the data.
    Encrypted,
    /// A crypt key file other users can read or write.
    KeyFileExposed {
        /// The key file.
        path: PathBuf,
        /// Its permission bits, e.g. `0o644`.
        mode: u32,
    },
}

impl std::fmt::Display for ConfigAdvisory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigAdvisory::Encrypted => f.write_str(
                "the vault is encrypted at rest, and this process holds its key and serves \
                 plaintext",
            ),
            ConfigAdvisory::KeyFileExposed { path, mode } => write!(
                f,
                "crypt key file {} is accessible to other users (mode {mode:o}); chmod 600 it",
                path.display()
            ),
        }
    }
}

/// How many `vault` origins may lead to a config before it is taken for a cycle.
const MAX_NESTING: usize = 16;

/// What [`VaultConfig::load`] produces.
pub(crate) struct LoadedVault {
    pub name: String,
    pub root_id: ObjectId,
    pub origin: Box<dyn Origin>,
    pub advisories: Vec<ConfigAdvisory>,
}

/// The permission bits of `path` when users other than its owner can read or write it (Unix
/// only; elsewhere `None`).
fn key_file_exposed(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
        (mode & 0o077 != 0).then_some(mode)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}
