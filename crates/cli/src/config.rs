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
mod tests {
    use super::*;

    #[test]
    fn default_local_vault_defaults_to_true_when_omitted() {
        let cfg: CliConfig = toml::from_str("").unwrap();
        assert!(cfg.default_local_vault);
        assert!(cfg.local_vault_path.is_none());
    }

    #[test]
    fn deserializes_explicit_values() {
        let cfg: CliConfig = toml::from_str(
            r#"
            default_local_vault = false
            local_vault_path = "/srv/data"
            "#,
        )
        .unwrap();
        assert!(!cfg.default_local_vault);
        assert_eq!(cfg.local_vault_path, Some(PathBuf::from("/srv/data")));
    }

    #[test]
    fn local_path_returns_configured_path_when_set() {
        let cfg = CliConfig {
            default_local_vault: true,
            local_vault_path: Some(PathBuf::from("/srv/data")),
        };
        assert_eq!(cfg.local_path(), PathBuf::from("/srv/data"));
    }

    #[test]
    fn local_path_falls_back_to_home_dir_when_unset() {
        let cfg = CliConfig {
            default_local_vault: true,
            local_vault_path: None,
        };
        let expected = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        assert_eq!(cfg.local_path(), expected);
    }
}
