use anyhow::Result;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct CliConfig {
    #[serde(default = "default_true")]
    pub default_local_vault: bool,
    #[serde(default)]
    pub local_vault_path: Option<PathBuf>,
}
fn default_true() -> bool {
    true
}
impl CliConfig {
    pub fn path() -> PathBuf {
        nimbus_vault::config_home().join("cli_config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(CliConfig {
                default_local_vault: true,
                local_vault_path: None,
            });
        }
        let raw = std::fs::read_to_string(&path)?;
        let config: CliConfig = toml::from_str(&raw)?;
        config.relative_to(&path)
    }

    /// Resolves a relative `local_vault_path` against the directory of `config_path`, the file
    /// it was read from, so it doesn't depend on the current directory.
    fn relative_to(self, config_path: &Path) -> Result<Self> {
        let dir = nimbus_vault::fsutil::config_dir(config_path)?;
        Ok(CliConfig {
            local_vault_path: self
                .local_vault_path
                .map(|path| nimbus_vault::fsutil::resolve_against(&dir, path)),
            ..self
        })
    }
    pub fn local_path(&self) -> PathBuf {
        self.local_vault_path
            .clone()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")))
    }
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
