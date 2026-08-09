use anyhow::{Context, Result, anyhow};
use nimbus_vault::vault::Vault;
use std::{collections::HashMap, path::Path, sync::Arc};
use tracing::warn;

use crate::{auth::AuthConfig, config::Config, error::ApiError};

#[derive(Clone)]
pub struct AppState {
    vaults: Arc<HashMap<String, Vault>>,
    pub read_only: bool,
    pub auth: Arc<AuthConfig>,
}

impl AppState {
    pub fn load(config: &Config) -> Result<Self> {
        Ok(AppState {
            vaults: Arc::new(load_vaults(&config.vaults_path)?),
            read_only: config.read_only,
            auth: Arc::new(config.auth.clone()),
        })
    }

    #[cfg(test)]
    pub fn from_parts(vaults: HashMap<String, Vault>, read_only: bool, auth: AuthConfig) -> Self {
        AppState {
            vaults: Arc::new(vaults),
            read_only,
            auth: Arc::new(auth),
        }
    }

    pub fn vault(&self, name: &str) -> Result<&Vault, ApiError> {
        self.vaults
            .get(name)
            .ok_or_else(|| ApiError::UnknownVault(name.to_string()))
    }

    pub fn vault_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.vaults.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn ensure_writable(&self) -> Result<(), ApiError> {
        match self.read_only {
            true => Err(ApiError::ReadOnly),
            false => Ok(()),
        }
    }
}

fn load_vaults(dir: &Path) -> Result<HashMap<String, Vault>> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("reading vaults directory {}", dir.display()))?;

    let mut vaults: HashMap<String, Vault> = HashMap::new();
    let mut sources: HashMap<String, std::path::PathBuf> = HashMap::new();

    for entry in entries {
        let path = entry
            .with_context(|| format!("reading an entry of {}", dir.display()))?
            .path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        let vault = match Vault::new(path.clone()) {
            Ok(vault) => vault,
            Err(e) => {
                warn!(path = %path.display(), "skipping vault config: {e}");
                continue;
            }
        };

        let name = vault.get_name().clone();
        if let Some(previous) = sources.get(&name) {
            return Err(anyhow!(
                "two vault configs claim the name '{name}': {} and {}",
                previous.display(),
                path.display()
            ));
        }
        sources.insert(name.clone(), path);
        vaults.insert(name, vault);
    }

    Ok(vaults)
}

#[cfg(test)]
#[path = "tests/state.rs"]
mod tests;
