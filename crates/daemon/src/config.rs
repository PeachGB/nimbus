use anyhow::{Context, Result, anyhow};
use clap::Parser;
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
    pub token: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub vaults_path: Option<PathBuf>,
    pub bind: Option<SocketAddr>,
    pub read_only: Option<bool>,
    pub auth: Option<AuthConfig>,
}

#[derive(Debug)]
pub struct Config {
    pub vaults_path: PathBuf,
    pub bind: SocketAddr,
    pub read_only: bool,
    pub auth: AuthConfig,
}

pub fn default_config_path() -> PathBuf {
    nimbus_vault::config_home().join(CONFIG_FILE_NAME)
}

fn default_bind() -> SocketAddr {
    DEFAULT_BIND.parse().expect("valid default bind address")
}

impl FileConfig {
    fn initial() -> Self {
        FileConfig {
            vaults_path: Some(nimbus_vault::config::VaultConfig::default_dir()),
            bind: Some(default_bind()),
            read_only: Some(false),
            auth: Some(AuthConfig::default()),
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
        std::fs::write(path, format!("{GENERATED_HEADER}\n{body}"))
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

    pub fn merge(file: FileConfig, args: &Args, config_path: &Path) -> Result<Self> {
        let vaults_path = args.vaults.clone().or(file.vaults_path).ok_or_else(|| {
            anyhow!(
                "no vaults directory configured: set `vaults_path` in {} or pass --vaults",
                config_path.display()
            )
        })?;

        let auth = match &args.token {
            Some(token) => AuthConfig::Bearer {
                token: token.clone(),
            },
            None => file.auth.unwrap_or_default(),
        };

        Ok(Config {
            vaults_path,
            bind: args.bind.or(file.bind).unwrap_or_else(default_bind),
            read_only: args.read_only || file.read_only.unwrap_or(false),
            auth,
        })
    }
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
