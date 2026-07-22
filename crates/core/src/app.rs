use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

use nimbus_vault::{
    config::OriginConfig,
    object::{Object, ObjectId},
    vault::Vault,
};
use std::path::{Component, Path, PathBuf};

use crate::config::CliConfig;

const APP_ROOT_ID: &str = "#APP_ROOT#";
const LOCAL_VAULT_NAME: &str = "LOCAL";

#[derive(Serialize, Deserialize, Default)]
struct SavedState {
    vault_configs: HashMap<String, PathBuf>,
}

pub struct App {
    vaults: HashMap<String, Vault>,
    vault_configs: HashMap<String, PathBuf>,
    cwd: ObjectId,
    cwd_path: PathBuf,
    current_vault: Option<String>,
    local_root: Option<PathBuf>,
    local_root_canonical: Option<PathBuf>,
}

impl Default for App {
    fn default() -> Self {
        App {
            vaults: HashMap::new(),
            vault_configs: HashMap::new(),
            cwd: ObjectId::from(APP_ROOT_ID),
            cwd_path: PathBuf::from("/"),
            current_vault: None,
            local_root: None,
            local_root_canonical: None,
        }
    }
}

impl App {
    pub fn pwd(&self) -> String {
        self.cwd_path.to_string_lossy().to_string()
    }
    pub fn current_vault(&self) -> Option<String> {
        self.current_vault.clone()
    }
    fn state_path() -> PathBuf {
        dirs::state_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("nimbus")
            .join("session.toml")
    }
    pub fn init() -> Result<Self> {
        let cli_config = CliConfig::load()?;

        let (local_root, local_root_canonical) = if cli_config.default_local_vault {
            let root = cli_config.local_path();
            let canonical = root
                .canonicalize()
                .map_err(|e| anyhow!("LOCAL VAULT ROOT IS INVALID: {}", e))?;
            (Some(root), Some(canonical))
        } else {
            (None, None)
        };

        let state_path = Self::state_path();
        let saved: SavedState = if state_path.exists() {
            let raw = std::fs::read_to_string(&state_path)?;
            toml::from_str(&raw)?
        } else {
            SavedState::default()
        };

        let mut vaults = HashMap::new();
        let mut vault_configs = HashMap::new();
        for (name, cfg_path) in saved.vault_configs {
            match Vault::new(cfg_path.clone()) {
                Ok(vault) => {
                    vaults.insert(name.clone(), vault);
                    vault_configs.insert(name, cfg_path);
                }
                Err(e) => eprintln!(
                    "WARNING: skipping vault '{name}' ({}): {e}",
                    cfg_path.display()
                ),
            }
        }

        let mut app = App {
            vaults,
            vault_configs,
            cwd: ObjectId::from(APP_ROOT_ID),
            cwd_path: PathBuf::from("/"),
            current_vault: None,
            local_root,
            local_root_canonical,
        };

        if cli_config.default_local_vault && !app.vaults.contains_key(LOCAL_VAULT_NAME) {
            let root = app
                .local_root
                .clone()
                .ok_or_else(|| anyhow!("local vault enabled but root not resolved"))?;
            let origin_config = OriginConfig::Fs { root };
            let origin = origin_config.build()?;
            let vault = Vault::from_parts(
                String::from(LOCAL_VAULT_NAME),
                Arc::from(origin),
                ObjectId::from("/"),
            )?;
            app.vaults.insert(String::from(LOCAL_VAULT_NAME), vault);
        }

        Ok(app)
    }
    pub fn save(&self) -> Result<()> {
        let state = SavedState {
            vault_configs: self.vault_configs.clone(),
        };
        let path = Self::state_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(&state)?)?;
        Ok(())
    }

    pub async fn cd(&mut self, path: Option<String>) -> Result<()> {
        let Some(path) = path else {
            self.cwd = ObjectId::from(APP_ROOT_ID);
            self.cwd_path = PathBuf::from("/");
            self.current_vault = None;
            return Ok(());
        };
        if self.current_vault.is_none() {
            let path_buf = PathBuf::from(&path);
            let mut components = path_buf.components();
            let Some(vault_name) = components.next() else {
                return Err(anyhow!("INVALID PATH PROVIDED FOR 'cd'"));
            };
            let Some(vault_name) = vault_name.as_os_str().to_str() else {
                return Err(anyhow!("INVALID PATH PROVIDED FOR 'cd'"));
            };
            self.select(String::from(vault_name))?;

            let remaining_path: PathBuf = components.collect();
            if remaining_path.as_os_str().is_empty() {
                return Ok(());
            }
            let path_buf = remaining_path.to_string_lossy().into_owned();
            return Box::pin(self.cd(Some(path_buf))).await;
        }

        let Some(current_vault) = &self.current_vault else {
            return Err(anyhow!("ERROR: CURRENT VAULT DOESN'T EXIST"));
        };
        let Some(vault) = self.vaults.get(current_vault) else {
            return Err(anyhow!("ERROR: CURRENT VAULT DOESN'T EXIST"));
        };

        let new_path = Self::resolve_relative(&self.cwd_path, path.as_ref());
        let dir = vault.find(new_path.clone()).await?;

        self.cwd = dir;
        self.cwd_path = new_path;
        Ok(())
    }

    /// Returns candidate completions for a partial `cd` argument: vault names when no
    /// vault is selected yet (or the vault's directories once a vault name and `/` have
    /// been typed), otherwise the current directory's subdirectories.
    pub async fn cd_completions(&self, word: &str) -> Vec<String> {
        match &self.current_vault {
            None => match word.split_once('/') {
                None => {
                    let mut names: Vec<String> = self
                        .vaults
                        .keys()
                        .filter(|name| name.starts_with(word))
                        .cloned()
                        .collect();
                    names.sort();
                    names
                }
                Some((vault_name, rest)) => {
                    let Some(vault) = self.vaults.get(vault_name) else {
                        return Vec::new();
                    };
                    Self::list_branch_names(vault, Path::new("/"), rest)
                        .await
                        .into_iter()
                        .map(|name| format!("{}/{}", vault_name, name))
                        .collect()
                }
            },
            Some(vault_name) => {
                let Some(vault) = self.vaults.get(vault_name) else {
                    return Vec::new();
                };
                Self::list_branch_names(vault, &self.cwd_path, word).await
            }
        }
    }

    /// Lists the subdirectory names of the directory addressed by `word`'s path segment
    /// (relative to `base`), filtered by the trailing partial name segment.
    async fn list_branch_names(vault: &Vault, base: &Path, word: &str) -> Vec<String> {
        let (dir_segment, name_prefix) = match word.rsplit_once('/') {
            Some((dir, name)) => (dir, name),
            None => ("", word),
        };
        let target_path = Self::resolve_relative(base, dir_segment);
        let Ok(dir_id) = vault.find(target_path).await else {
            return Vec::new();
        };
        let Ok(children) = vault.list(&dir_id).await else {
            return Vec::new();
        };
        let prefix = if dir_segment.is_empty() {
            String::new()
        } else {
            format!("{}/", dir_segment)
        };
        let mut names: Vec<String> = children
            .into_iter()
            .filter_map(|o| match o {
                Object::Branch { name, .. } if name.starts_with(name_prefix) => {
                    Some(format!("{}{}", prefix, name))
                }
                _ => None,
            })
            .collect();
        names.sort();
        names
    }

    fn resolve_relative(current: &Path, input: &str) -> PathBuf {
        if input.starts_with('/') {
            return Self::normalize(Path::new(input));
        }
        let mut combined = current.to_path_buf();
        combined.push(input);
        Self::normalize(&combined)
    }

    fn normalize(path: &Path) -> PathBuf {
        let mut result = PathBuf::from("/");
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    result.pop();
                }
                Component::Normal(c) => result.push(c),
                Component::RootDir | Component::CurDir => {}
                Component::Prefix(_) => {}
            }
        }
        result
    }
    fn resolve_local_path(&self, user_path: &str) -> Result<PathBuf> {
        let root_canonical = self.local_root_canonical.as_ref().ok_or_else(|| {
            anyhow!("no local vault configured (default_local_vault is disabled)")
        })?;

        let absolute =
            std::path::absolute(user_path).map_err(|e| anyhow!("INVALID PATH: {}", e))?;
        let canonical = absolute
            .canonicalize()
            .map_err(|e| anyhow!("PATH DOESN'T EXIST OR IT IS INACCESSIBLE: {}", e))?;

        if !canonical.starts_with(root_canonical) {
            return Err(anyhow!(
                "path '{}' is outside the local vault (root: {})",
                user_path,
                root_canonical.display()
            ));
        }

        let relative = canonical
            .strip_prefix(root_canonical)
            .map_err(|_| anyhow!("failed to compute relative path"))?;
        Ok(PathBuf::from("/").join(relative))
    }

    pub async fn ls(&self) -> Result<()> {
        if self.current_vault.is_none() {
            return self.vaults();
        }
        let Some(current_vault) = &self.current_vault else {
            return Err(anyhow!("UNREACHABLE"));
        };
        let Some(vault) = self.vaults.get(current_vault) else {
            return Err(anyhow!("COULDN'T GET CURRENT WORKING VAULT"));
        };
        match vault.list(&self.cwd).await {
            Ok(list) => {
                for o in list {
                    println!("{}", o.get_name());
                }
                Ok(())
            }
            Err(e) => Err(anyhow!(e)),
        }
    }
    pub fn vaults(&self) -> Result<()> {
        for k in self.vaults.keys() {
            println!("{}", k);
        }
        Ok(())
    }
    pub fn select(&mut self, vault_name: String) -> Result<()> {
        if !self.vaults.contains_key(&vault_name) {
            return Err(anyhow!("VAULT '{}' DOESN'T EXITS", vault_name));
        }
        let Some(vault) = self.vaults.get(&vault_name) else {
            return Err(anyhow!("NO VAULT NAMED {}", vault_name));
        };
        self.current_vault = Some(vault_name);
        self.cwd = vault.get_id();
        self.cwd_path = PathBuf::from("/");
        Ok(())
    }

    pub fn new_vault(&mut self, cfg: PathBuf) -> Result<()> {
        let vault = Vault::new(cfg.clone())?;
        let name = vault.get_name().clone();
        self.vault_configs.insert(name.clone(), cfg);
        self.vaults.insert(name, vault);
        self.save()
    }
    pub async fn put(
        &mut self,
        path: String,
        vault: Option<String>,
        dest: Option<String>,
    ) -> Result<()> {
        let local = self
            .vaults
            .get(LOCAL_VAULT_NAME)
            .ok_or_else(|| anyhow!("NO LOCAL VAULT CONFIGURED"))?;

        let local_path = self.resolve_local_path(&path)?; // antes: PathBuf::from(path)
        let source_id = local.find(local_path).await?;
        let mut source_obj = local.get(source_id.clone()).await?;

        let target_vault_name = match vault {
            Some(v) => v,
            None => self
                .current_vault
                .clone()
                .ok_or_else(|| anyhow!("NO VAULT SPECIFIED AND NO CURRENT VAULT SELECTED"))?,
        };
        let target_vault = self.vaults.get(&target_vault_name).ok_or_else(|| {
            anyhow!(
                "COULDN'T FIND CONFIGURATION FOR VAULT '{}'",
                target_vault_name
            )
        })?;

        let dest_path = match dest {
            Some(d) => Self::resolve_relative(&self.cwd_path, &d),
            None => self.cwd_path.clone(),
        };
        let dest_parent_id = target_vault.find(dest_path).await?;

        let placed = target_vault.put(&mut source_obj, &dest_parent_id).await?;

        if let Object::Leaf { .. } = placed {
            let bytes = local.fetch(source_id).await?;
            target_vault.send(&placed, bytes).await?;
        }

        Ok(())
    }
    pub async fn get(
        &mut self,
        path: String,
        vault: Option<String>,
        dest: Option<String>,
    ) -> Result<()> {
        let target_vault_name = match vault {
            Some(v) => v,
            None => self
                .current_vault
                .clone()
                .ok_or_else(|| anyhow!("NO VAULT SPECIFIED AND NO CURRENT VAULT SELECTED"))?,
        };
        let source_vault = self.vaults.get(&target_vault_name).ok_or_else(|| {
            anyhow!(
                "COULDN'T FIND CONFIGURATION FOR VAULT '{}'",
                target_vault_name
            )
        })?;

        let source_path = Self::resolve_relative(&self.cwd_path, &path);
        let source_id = source_vault.find(source_path).await?;
        let mut source_obj = source_vault.get(source_id.clone()).await?;

        let local = self
            .vaults
            .get(LOCAL_VAULT_NAME)
            .ok_or_else(|| anyhow!("NO LOCAL VAULT CONFIGURED"))?;

        let dest_path = match dest {
            Some(d) => self.resolve_local_path(&d)?,
            None => self.resolve_local_path(".")?,
        };
        let dest_parent_id = local.find(dest_path).await?;

        let placed = local.put(&mut source_obj, &dest_parent_id).await?;

        if let Object::Leaf { .. } = placed {
            let bytes = source_vault.fetch(source_id).await?;
            local.send(&placed, bytes).await?;
        }

        Ok(())
    }
    pub async fn cp(&mut self, name: String, destination: String, vault: Option<String>) -> Result<()> {
        let vault_name = match vault {
            Some(v) => v,
            None => self
                .current_vault
                .clone()
                .ok_or_else(|| anyhow!("NO VAULT SPECIFIED AND NO CURRENT VAULT SELECTED"))?,
        };
        let v = self
            .vaults
            .get(&vault_name)
            .ok_or_else(|| anyhow!("COULDN'T FIND CONFIGURATION FOR VAULT '{}'", vault_name))?;

        let source_path = Self::resolve_relative(&self.cwd_path, &name);
        let source_id = v.find(source_path).await?;
        let mut source_obj = v.get(source_id.clone()).await?;

        let dest_path = Self::resolve_relative(&self.cwd_path, &destination);
        let dest_parent_id = v.find(dest_path).await?;

        let placed = v.put(&mut source_obj, &dest_parent_id).await?;

        if let Object::Leaf { .. } = placed {
            let bytes = v.fetch(source_id).await?;
            v.send(&placed, bytes).await?;
        }

        Ok(())
    }
    pub async fn mv(&mut self, name: String, destination: String, vault: Option<String>) -> Result<()> {
        let vault_name = match vault {
            Some(v) => v,
            None => self
                .current_vault
                .clone()
                .ok_or_else(|| anyhow!("NO VAULT SPECIFIED AND NO CURRENT VAULT SELECTED"))?,
        };
        let v = self
            .vaults
            .get(&vault_name)
            .ok_or_else(|| anyhow!("COULDN'T FIND CONFIGURATION FOR VAULT '{}'", vault_name))?;

        let source_path = Self::resolve_relative(&self.cwd_path, &name);
        let source_id = v.find(source_path).await?;
        let mut source_obj = v.get(source_id.clone()).await?;

        let dest_path = Self::resolve_relative(&self.cwd_path, &destination);
        let dest_parent_id = v.find(dest_path).await?;

        let placed = v.put(&mut source_obj, &dest_parent_id).await?;

        if let Object::Leaf { .. } = placed {
            let bytes = v.fetch(source_id.clone()).await?;
            v.send(&placed, bytes).await?;
        }

        v.delete(&source_id).await?;

        Ok(())
    }
    pub async fn delete(&mut self, path: String, vault: Option<String>, force: bool) -> Result<()> {
        let vault_name = match vault {
            Some(v) => v,
            None => self
                .current_vault
                .clone()
                .ok_or_else(|| anyhow!("NO VAULT SPECIFIED AND NO CURRENT VAULT SELECTED"))?,
        };
        let v = self
            .vaults
            .get(&vault_name)
            .ok_or_else(|| anyhow!("COULDN'T FIND CONFIGURATION FOR VAULT '{}'", vault_name))?;

        let target_path = Self::resolve_relative(&self.cwd_path, &path);
        let target_id = v.find(target_path).await?;

        if !force {
            let children = v.list(target_id.clone()).await?;
            if !children.is_empty() {
                return Err(anyhow!(
                    "'{}' is not empty — use --force to delete recursively",
                    path
                ));
            }
        }

        v.delete(&target_id).await?;
        Ok(())
    }

    pub async fn push(&mut self, vault: Option<String>) -> Result<()> {
        let target_name = match vault {
            Some(v) => v,
            None => self
                .current_vault
                .clone()
                .ok_or_else(|| anyhow!("NO VAULT SPECIFIED AND NO CURRENT VAULT SELECTED"))?,
        };

        let target_origin = self
            .vaults
            .get(&target_name)
            .ok_or_else(|| anyhow!("COULDN'T FIND CONFIGURATION FOR VAULT '{}'", target_name))?
            .get_origin();

        let local = self
            .vaults
            .get(LOCAL_VAULT_NAME)
            .ok_or_else(|| anyhow!("NO LOCAL VAULT CONFIGURED"))?;

        let root = local.get_id();
        local.push(&root, target_origin.as_ref()).await?;

        Ok(())
    }

    pub async fn pull(&mut self, vault: Option<String>) -> Result<()> {
        let source_name = match vault {
            Some(v) => v,
            None => self
                .current_vault
                .clone()
                .ok_or_else(|| anyhow!("NO VAULT SPECIFIED AND NO CURRENT VAULT SELECTED"))?,
        };

        let source_origin = self
            .vaults
            .get(&source_name)
            .ok_or_else(|| anyhow!("COULDN'T FIND CONFIGURATION FOR VAULT '{}'", source_name))?
            .get_origin();

        let local = self
            .vaults
            .get(LOCAL_VAULT_NAME)
            .ok_or_else(|| anyhow!("NO LOCAL VAULT CONFIGURED"))?;

        let root = local.get_id();
        local.pull(&root, source_origin.as_ref()).await?;

        Ok(())
    }
    pub fn exit(&mut self) -> Result<()> {
        self.save()?;
        std::process::exit(0);
    }
}

#[cfg(test)]
#[path = "tests/app.rs"]
mod tests;
