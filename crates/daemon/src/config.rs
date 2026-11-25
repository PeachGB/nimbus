use anyhow::{Context, Result, anyhow};
use clap::Parser;
use nimbus_vault::{
    fsutil::{config_dir, resolve_against},
    secret::Secret,
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::Path, path::PathBuf};
use tracing::{info, warn};

use crate::auth::AuthConfig;

pub const CONFIG_FILE_NAME: &str = "daemon_config.toml";

const DEFAULT_BIND: &str = "127.0.0.1:8080";

const GENERATED_HEADER: &str = "\
# Written by nimbus-daemon on first run: every value below is what it would have used anyway.
# Edit it, or delete the file and it gets written again.
";

/// Command-line overrides.
///
/// Every setting that also lives in the config file is an `Option`, so "not passed" stays
/// distinguishable from "passed a value that happens to equal the default" — that's what makes
/// the `flags > file > defaults` precedence in [`Config::merge`] work.
#[derive(Parser, Debug, Default)]
#[command(version, about = "Serves a directory of nimbus vaults over HTTP")]
pub struct Args {
    /// Config file to read (default: daemon_config.toml in the config home)
    #[arg(short, long, value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Directory scanned for `*.toml` vault configs
    #[arg(long, value_name = "DIR")]
    pub vaults: Option<PathBuf>,
    /// Address to listen on
    #[arg(short, long, value_name = "ADDR")]
    pub bind: Option<SocketAddr>,
    /// Refuse every write (put/send/delete). Can only be turned on here, never off
    #[arg(long)]
    pub read_only: bool,
    /// Require this bearer token, replacing whatever `[auth]` says
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<Secret>,
    /// Serve HTTPS with this PEM certificate chain; needs --tls-key. Replaces `[tls]`
    #[arg(long, value_name = "FILE", requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,
    /// The PEM private key for --tls-cert; needs --tls-cert
    #[arg(long, value_name = "FILE", requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,
}

/// The `[tls]` table: a PEM certificate chain and its private key. Its presence is what turns
/// HTTPS on; without it the daemon serves plain HTTP exactly as before.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    /// The certificate chain, leaf first, in PEM.
    pub cert_file: PathBuf,
    /// The private key for the leaf certificate, in PEM.
    pub key_file: PathBuf,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub vaults_path: Option<PathBuf>,
    pub bind: Option<SocketAddr>,
    pub read_only: Option<bool>,
    /// Lets a bearer token travel over plain HTTP on a non-loopback address (see
    /// [`Config::validate`]). File only, with no flag, so turning it on is a deliberate edit.
    /// Kept above the tables: TOML needs plain values before any table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_insecure_http: Option<bool>,
    pub auth: Option<AuthConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,
}

#[derive(Debug)]
pub struct Config {
    pub vaults_path: PathBuf,
    pub bind: SocketAddr,
    pub read_only: bool,
    pub auth: AuthConfig,
    /// `Some` when serving HTTPS.
    pub tls: Option<TlsConfig>,
    pub allow_insecure_http: bool,
}

pub fn default_config_path() -> PathBuf {
    nimbus_vault::config_home().join(CONFIG_FILE_NAME)
}

fn default_bind() -> SocketAddr {
    DEFAULT_BIND.parse().expect("valid default bind address")
}

impl TlsConfig {
    /// Resolves relative `cert_file` and `key_file` against `dir`, the config file's directory.
    fn relative_to(self, dir: &Path) -> Self {
        TlsConfig {
            cert_file: resolve_against(dir, self.cert_file),
            key_file: resolve_against(dir, self.key_file),
        }
    }
}

impl FileConfig {
    /// Resolves the file's relative paths (`vaults_path`, `[tls]`, `[auth]`'s `token_file`)
    /// against `dir`, the directory holding the config file. Absolute paths are untouched.
    fn relative_to(self, dir: &Path) -> Self {
        FileConfig {
            vaults_path: self.vaults_path.map(|path| resolve_against(dir, path)),
            tls: self.tls.map(|tls| tls.relative_to(dir)),
            auth: self.auth.map(|auth| auth.relative_to(dir)),
            ..self
        }
    }

    fn initial() -> Self {
        FileConfig {
            vaults_path: Some(nimbus_vault::config::VaultConfig::default_dir()),
            bind: Some(default_bind()),
            read_only: Some(false),
            allow_insecure_http: None,
            auth: Some(AuthConfig::default()),
            tls: None,
        }
    }

    fn write(&self, path: &Path) -> Result<()> {
        let body = toml::to_string_pretty(self).context("serializing the default config")?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        // Atomic, so an interrupted first run can't leave a truncated config that every later
        // start fails to parse. The generated default never holds a token (`--token` is merged
        // in memory only), but a config with one inline is kept private anyway.
        let mode = self
            .auth
            .as_ref()
            .is_some_and(AuthConfig::holds_secret)
            .then_some(0o600);
        nimbus_vault::fsutil::atomic_write(
            path,
            format!("{GENERATED_HEADER}\n{body}").as_bytes(),
            mode,
        )
        .with_context(|| format!("writing {}", path.display()))?;

        if let Some(vaults) = &self.vaults_path {
            std::fs::create_dir_all(vaults)
                .with_context(|| format!("creating {}", vaults.display()))?;
        }
        Ok(())
    }
}

impl Config {
    pub fn resolve(args: &Args) -> Result<Self> {
        let (path, explicit) = match &args.config {
            Some(path) => (path.clone(), true),
            None => (default_config_path(), false),
        };

        let file = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?
        } else if explicit {
            return Err(anyhow!("no config file at {}", path.display()));
        } else {
            let initial = FileConfig::initial();
            match initial.write(&path) {
                Ok(()) => info!("wrote a default config to {}", path.display()),
                Err(e) => warn!("continuing with the defaults: {e:#}"),
            }
            initial
        };

        Self::merge(file, args, &path)
    }

    /// Combines the file at `config_path` with the flags, flags winning. Relative paths from
    /// the file resolve against the file's directory, so they don't depend on where the daemon
    /// was started; paths given as flags (`--vaults`, `--tls-cert`, `--tls-key`) stay relative
    /// to the current directory, like any command-line path.
    pub fn merge(file: FileConfig, args: &Args, config_path: &Path) -> Result<Self> {
        let file = file
            .relative_to(&config_dir(config_path).with_context(|| {
                format!("resolving the directory of {}", config_path.display())
            })?);
        let vaults_path = args.vaults.clone().or(file.vaults_path).ok_or_else(|| {
            anyhow!(
                "no vaults directory configured: set `vaults_path` in {} or pass --vaults",
                config_path.display()
            )
        })?;

        let auth = match &args.token {
            Some(token) => AuthConfig::Bearer {
                token: Some(token.clone()),
                token_file: None,
            },
            None => file.auth.unwrap_or_default(),
        };

        // Both flags or neither: clap enforces it on the command line, and this catches an
        // `Args` built any other way.
        let tls = match (&args.tls_cert, &args.tls_key) {
            (Some(cert_file), Some(key_file)) => Some(TlsConfig {
                cert_file: cert_file.clone(),
                key_file: key_file.clone(),
            }),
            (None, None) => file.tls,
            _ => return Err(anyhow!("--tls-cert and --tls-key must be given together")),
        };

        Ok(Config {
            vaults_path,
            bind: args.bind.or(file.bind).unwrap_or_else(default_bind),
            read_only: args.read_only || file.read_only.unwrap_or(false),
            auth,
            tls,
            allow_insecure_http: file.allow_insecure_http.unwrap_or(false),
        })
    }

    /// Checks the settings that are unsafe together, before anything is bound. Returns the
    /// warnings to log, or an error when the daemon must not start.
    ///
    /// The one hard rule: a bearer token sent over plain HTTP on a non-loopback address can be
    /// read by anyone on the path, so that combination is refused unless the config file says
    /// `allow_insecure_http = true` (for a deployment behind a TLS-terminating proxy on the
    /// same private network, say). `0.0.0.0` and `::` are not loopback. With no
    /// authentication on a non-loopback address the daemon still starts, with a warning.
    pub fn validate(&self) -> Result<Vec<String>> {
        let mut warnings = Vec::new();
        let exposed = !self.bind.ip().is_loopback();
        let bearer = matches!(self.auth, AuthConfig::Bearer { .. });
        if exposed && bearer && self.tls.is_none() {
            if !self.allow_insecure_http {
                return Err(anyhow!(
                    "refusing to serve a bearer token over plain HTTP on {}: anyone between a \
                     client and this host could read it. Configure [tls] (or --tls-cert and \
                     --tls-key), bind to a loopback address behind a TLS-terminating proxy, or \
                     set allow_insecure_http = true in the config file if you accept the risk",
                    self.bind
                ));
            }
            warnings.push(format!(
                "allow_insecure_http is set: the bearer token travels unencrypted to {}",
                self.bind
            ));
        }
        if exposed && self.auth.is_open() {
            warnings.push(format!(
                "listening on {} with no authentication configured — anyone who can reach this \
                 port can read{} these vaults",
                self.bind,
                if self.read_only { "" } else { " and write" }
            ));
        }
        Ok(warnings)
    }
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
