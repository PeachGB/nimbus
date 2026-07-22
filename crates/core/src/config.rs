use anyhow::Result;
use std::path::PathBuf;

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
        dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(".nimbus")
            .join("cli_config.toml")
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
        Ok(toml::from_str(&raw)?)
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
