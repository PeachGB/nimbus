use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

use nimbus_vault::{
    config::OriginConfig,
    object::{Object, ObjectId},
    vault::Vault,
};
const APP_ROOT_ID: &str = "#APP_ROOT#";
use std::path::{Component, Path, PathBuf};

use crate::{Cli, Commands, LOCAL_VAULT_NAME, config::CliConfig};

#[derive(Serialize, Deserialize, Default)]
struct SavedState {
    vault_configs: HashMap<String, PathBuf>,
    current_vault: Option<String>,
    cwd: ObjectId,
    cwd_path: PathBuf,
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

impl App {
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
        for (name, cfg_path) in &saved.vault_configs {
            let vault = Vault::new(cfg_path.clone())?;
            vaults.insert(name.clone(), vault);
        }

        let mut app = App {
            vaults,
            vault_configs: saved.vault_configs,
            cwd: if saved.cwd.as_str().is_empty() {
                ObjectId::from(APP_ROOT_ID)
            } else {
                saved.cwd
            },
            cwd_path: if saved.cwd_path.as_os_str().is_empty() {
                PathBuf::from("/")
            } else {
                saved.cwd_path
            },
            current_vault: saved.current_vault,
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
            current_vault: self.current_vault.clone(),
            cwd: self.cwd.clone(),
            cwd_path: self.cwd_path.clone(),
        };
        let path = Self::state_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(&state)?)?;
        Ok(())
    }
    pub async fn parse(&mut self, cli: Cli) -> Result<()> {
        match cli.command {
            Commands::Ls => self.ls().await,
            Commands::Vaults => self.vaults(),
            Commands::Select { vault } => self.select(vault),
            Commands::New { path } => self.new(PathBuf::from(path)),
            Commands::Cd { path } => self.cd(path).await,
            Commands::Put { path, vault, dest } => self.put(path, vault, dest).await,
            Commands::Get { path, vault, dest } => self.get(path, vault, dest).await,
            Commands::Cp {
                path,
                destination,
                vault,
            } => self.cp(path, destination, vault).await,
            Commands::Mv {
                path,
                destination,
                vault,
            } => self.mv(path, destination, vault).await,
            Commands::Delete { path, vault, force } => self.delete(path, vault, force).await,
            Commands::Push { vault } => self.push(vault).await,
            Commands::Pull { vault } => self.pull(vault).await,
        }
    }

    pub async fn cd(&mut self, path: String) -> Result<()> {
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
            return Box::pin(self.cd(path_buf)).await;
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
        for (k, _) in &self.vaults {
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
        self.current_vault = Some(String::from(vault_name));
        self.cwd = vault.get_id();
        self.cwd_path = PathBuf::from("/");
        Ok(())
    }

    pub fn new(&mut self, cfg: PathBuf) -> Result<()> {
        let vault = Vault::new(cfg.clone())?;
        let name = vault.get_name().clone();
        self.vault_configs.insert(name.clone(), cfg);
        self.vaults.insert(name, vault);
        Ok(())
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
    pub async fn cp(
        &mut self,
        name: String,
        destination: String,
        vault: Option<String>,
    ) -> Result<()> {
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
    pub async fn mv(
        &mut self,
        name: String,
        destination: String,
        vault: Option<String>,
    ) -> Result<()> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimbus_vault::origin::fs::OriginFileSystem;

    fn fs_vault(name: &str, root: PathBuf) -> Vault {
        let origin = OriginFileSystem::new(root);
        Vault::from_parts(name.to_string(), Arc::new(origin), ObjectId::from("")).unwrap()
    }

    fn make_app(vaults: HashMap<String, Vault>) -> App {
        App {
            vaults,
            vault_configs: HashMap::new(),
            cwd: ObjectId::from(APP_ROOT_ID),
            cwd_path: PathBuf::from("/"),
            current_vault: None,
            local_root: None,
            local_root_canonical: None,
        }
    }

    // --- resolve_relative / normalize ---

    #[test]
    fn resolve_relative_absolute_input_ignores_current() {
        let result = App::resolve_relative(Path::new("/some/where"), "/other/place");
        assert_eq!(result, PathBuf::from("/other/place"));
    }

    #[test]
    fn resolve_relative_relative_input_appends_to_current() {
        let result = App::resolve_relative(Path::new("/docs"), "notes");
        assert_eq!(result, PathBuf::from("/docs/notes"));
    }

    #[test]
    fn resolve_relative_handles_parent_dir() {
        let result = App::resolve_relative(Path::new("/docs/2024"), "..");
        assert_eq!(result, PathBuf::from("/docs"));
    }

    #[test]
    fn resolve_relative_parent_dir_at_root_stays_at_root() {
        let result = App::resolve_relative(Path::new("/"), "..");
        assert_eq!(result, PathBuf::from("/"));
    }

    #[test]
    fn resolve_relative_handles_current_dir_component() {
        let result = App::resolve_relative(Path::new("/docs"), "./notes");
        assert_eq!(result, PathBuf::from("/docs/notes"));
    }

    // --- resolve_local_path ---

    #[test]
    fn resolve_local_path_within_root_returns_relative_vault_path() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("notes.txt"), b"hi").unwrap();
        let mut app = make_app(HashMap::new());
        app.local_root_canonical = Some(root.path().canonicalize().unwrap());

        let result = app
            .resolve_local_path(root.path().join("notes.txt").to_str().unwrap())
            .unwrap();
        assert_eq!(result, PathBuf::from("/notes.txt"));
    }

    #[test]
    fn resolve_local_path_outside_root_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"hi").unwrap();
        let mut app = make_app(HashMap::new());
        app.local_root_canonical = Some(root.path().canonicalize().unwrap());

        let result = app.resolve_local_path(outside.path().join("secret.txt").to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn resolve_local_path_nonexistent_path_errors() {
        let root = tempfile::tempdir().unwrap();
        let mut app = make_app(HashMap::new());
        app.local_root_canonical = Some(root.path().canonicalize().unwrap());

        let result = app.resolve_local_path(root.path().join("missing.txt").to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn resolve_local_path_errors_when_no_local_vault_configured() {
        let app = make_app(HashMap::new());
        let result = app.resolve_local_path("/tmp");
        assert!(result.is_err());
    }

    // --- select ---

    #[test]
    fn select_sets_current_vault_and_resets_cwd() {
        let root = tempfile::tempdir().unwrap();
        let mut vaults = HashMap::new();
        vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
        let mut app = make_app(vaults);

        app.select("v1".to_string()).unwrap();
        assert_eq!(app.current_vault.as_deref(), Some("v1"));
        assert_eq!(app.cwd_path, PathBuf::from("/"));
        assert_eq!(app.cwd.as_str(), "");
    }

    #[test]
    fn select_unknown_vault_errors() {
        let mut app = make_app(HashMap::new());
        assert!(app.select("missing".to_string()).is_err());
    }

    // --- new ---

    #[test]
    fn new_registers_vault_from_config_file() {
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let config_path = config_dir.path().join("vault.toml");
        std::fs::write(
            &config_path,
            format!(
                "name = \"my-vault\"\n\n[origin_config]\ntype = \"fs\"\nroot = \"{}\"\n",
                data_dir.path().display()
            ),
        )
        .unwrap();

        let mut app = make_app(HashMap::new());
        app.new(config_path.clone()).unwrap();

        assert!(app.vaults.contains_key("my-vault"));
        assert_eq!(app.vault_configs.get("my-vault"), Some(&config_path));
    }

    // --- cd ---

    #[tokio::test]
    async fn cd_at_root_level_selects_vault_and_recurses_into_remaining_path() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        let mut vaults = HashMap::new();
        vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
        let mut app = make_app(vaults);

        app.cd("v1/docs".to_string()).await.unwrap();

        assert_eq!(app.current_vault.as_deref(), Some("v1"));
        assert_eq!(app.cwd_path, PathBuf::from("/docs"));
    }

    #[tokio::test]
    async fn cd_within_vault_resolves_relative_to_cwd() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        let mut vaults = HashMap::new();
        vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
        let mut app = make_app(vaults);
        app.select("v1".to_string()).unwrap();
        app.cd("docs".to_string()).await.unwrap();
        assert_eq!(app.cwd_path, PathBuf::from("/docs"));

        app.cd("..".to_string()).await.unwrap();
        assert_eq!(app.cwd_path, PathBuf::from("/"));
    }

    #[tokio::test]
    async fn cd_unknown_path_component_errors() {
        let root = tempfile::tempdir().unwrap();
        let mut vaults = HashMap::new();
        vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
        let mut app = make_app(vaults);
        app.select("v1".to_string()).unwrap();

        let result = app.cd("missing".to_string()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cd_at_root_level_with_unknown_vault_errors() {
        let mut app = make_app(HashMap::new());
        let result = app.cd("missing".to_string()).await;
        assert!(result.is_err());
    }

    // --- ls ---

    #[tokio::test]
    async fn ls_with_no_current_vault_delegates_to_vaults() {
        let app = make_app(HashMap::new());
        assert!(app.ls().await.is_ok());
    }

    #[tokio::test]
    async fn ls_lists_current_vault_contents() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), b"hi").unwrap();
        let mut vaults = HashMap::new();
        vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
        let mut app = make_app(vaults);
        app.select("v1".to_string()).unwrap();

        assert!(app.ls().await.is_ok());
    }

    // --- put / get ---

    #[tokio::test]
    async fn put_copies_local_file_into_target_vault() {
        let local_root = tempfile::tempdir().unwrap();
        std::fs::write(local_root.path().join("note.txt"), b"hello").unwrap();
        let target_root = tempfile::tempdir().unwrap();

        let mut vaults = HashMap::new();
        vaults.insert(
            LOCAL_VAULT_NAME.to_string(),
            fs_vault(LOCAL_VAULT_NAME, local_root.path().to_path_buf()),
        );
        vaults.insert(
            "v1".to_string(),
            fs_vault("v1", target_root.path().to_path_buf()),
        );
        let mut app = make_app(vaults);
        app.local_root_canonical = Some(local_root.path().canonicalize().unwrap());

        let source = local_root.path().join("note.txt");
        app.put(
            source.to_str().unwrap().to_string(),
            Some("v1".to_string()),
            None,
        )
        .await
        .unwrap();

        let contents = std::fs::read(target_root.path().join("note.txt")).unwrap();
        assert_eq!(contents, b"hello");
    }

    #[tokio::test]
    async fn put_without_local_vault_errors() {
        let target_root = tempfile::tempdir().unwrap();
        let mut vaults = HashMap::new();
        vaults.insert(
            "v1".to_string(),
            fs_vault("v1", target_root.path().to_path_buf()),
        );
        let mut app = make_app(vaults);

        let result = app
            .put("/nope".to_string(), Some("v1".to_string()), None)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_copies_vault_file_into_local_root() {
        let source_root = tempfile::tempdir().unwrap();
        std::fs::write(source_root.path().join("note.txt"), b"remote-data").unwrap();
        let local_root = tempfile::tempdir().unwrap();

        let mut vaults = HashMap::new();
        vaults.insert(
            "v1".to_string(),
            fs_vault("v1", source_root.path().to_path_buf()),
        );
        vaults.insert(
            LOCAL_VAULT_NAME.to_string(),
            fs_vault(LOCAL_VAULT_NAME, local_root.path().to_path_buf()),
        );
        let mut app = make_app(vaults);
        app.local_root_canonical = Some(local_root.path().canonicalize().unwrap());

        app.get(
            "note.txt".to_string(),
            Some("v1".to_string()),
            Some(local_root.path().to_str().unwrap().to_string()),
        )
        .await
        .unwrap();

        let contents = std::fs::read(local_root.path().join("note.txt")).unwrap();
        assert_eq!(contents, b"remote-data");
    }

    // --- cp / mv ---

    #[tokio::test]
    async fn cp_duplicates_object_within_same_vault() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), b"data").unwrap();
        std::fs::create_dir_all(root.path().join("dir")).unwrap();
        let mut vaults = HashMap::new();
        vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
        let mut app = make_app(vaults);
        app.select("v1".to_string()).unwrap();

        app.cp(
            "a.txt".to_string(),
            "dir".to_string(),
            Some("v1".to_string()),
        )
        .await
        .unwrap();

        assert!(root.path().join("a.txt").exists());
        assert_eq!(
            std::fs::read(root.path().join("dir").join("a.txt")).unwrap(),
            b"data"
        );
    }

    #[tokio::test]
    async fn mv_moves_object_and_deletes_source() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), b"data").unwrap();
        std::fs::create_dir_all(root.path().join("dir")).unwrap();
        let mut vaults = HashMap::new();
        vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
        let mut app = make_app(vaults);
        app.select("v1".to_string()).unwrap();

        app.mv(
            "a.txt".to_string(),
            "dir".to_string(),
            Some("v1".to_string()),
        )
        .await
        .unwrap();

        assert!(!root.path().join("a.txt").exists());
        assert_eq!(
            std::fs::read(root.path().join("dir").join("a.txt")).unwrap(),
            b"data"
        );
    }

    // --- delete ---

    #[tokio::test]
    async fn delete_removes_empty_directory_without_force() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("dir")).unwrap();
        let mut vaults = HashMap::new();
        vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
        let mut app = make_app(vaults);
        app.select("v1".to_string()).unwrap();

        app.delete("dir".to_string(), Some("v1".to_string()), false)
            .await
            .unwrap();
        assert!(!root.path().join("dir").exists());
    }

    #[tokio::test]
    async fn delete_non_empty_directory_without_force_errors() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("dir")).unwrap();
        std::fs::write(root.path().join("dir").join("a.txt"), b"data").unwrap();
        let mut vaults = HashMap::new();
        vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
        let mut app = make_app(vaults);
        app.select("v1".to_string()).unwrap();

        let result = app
            .delete("dir".to_string(), Some("v1".to_string()), false)
            .await;
        assert!(result.is_err());
        assert!(root.path().join("dir").exists());
    }

    #[tokio::test]
    async fn delete_non_empty_directory_with_force_succeeds() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("dir")).unwrap();
        std::fs::write(root.path().join("dir").join("a.txt"), b"data").unwrap();
        let mut vaults = HashMap::new();
        vaults.insert("v1".to_string(), fs_vault("v1", root.path().to_path_buf()));
        let mut app = make_app(vaults);
        app.select("v1".to_string()).unwrap();

        app.delete("dir".to_string(), Some("v1".to_string()), true)
            .await
            .unwrap();
        assert!(!root.path().join("dir").exists());
    }

    // --- push / pull ---

    #[tokio::test]
    async fn push_syncs_local_vault_contents_into_target_vault() {
        let local_root = tempfile::tempdir().unwrap();
        std::fs::write(local_root.path().join("a.txt"), b"local-data").unwrap();
        let target_root = tempfile::tempdir().unwrap();

        let mut vaults = HashMap::new();
        vaults.insert(
            LOCAL_VAULT_NAME.to_string(),
            fs_vault(LOCAL_VAULT_NAME, local_root.path().to_path_buf()),
        );
        vaults.insert(
            "v1".to_string(),
            fs_vault("v1", target_root.path().to_path_buf()),
        );
        let mut app = make_app(vaults);

        app.push(Some("v1".to_string())).await.unwrap();

        assert_eq!(
            std::fs::read(target_root.path().join("a.txt")).unwrap(),
            b"local-data"
        );
    }

    #[tokio::test]
    async fn pull_syncs_source_vault_contents_into_local_vault() {
        let source_root = tempfile::tempdir().unwrap();
        std::fs::write(source_root.path().join("a.txt"), b"remote-data").unwrap();
        let local_root = tempfile::tempdir().unwrap();

        let mut vaults = HashMap::new();
        vaults.insert(
            "v1".to_string(),
            fs_vault("v1", source_root.path().to_path_buf()),
        );
        vaults.insert(
            LOCAL_VAULT_NAME.to_string(),
            fs_vault(LOCAL_VAULT_NAME, local_root.path().to_path_buf()),
        );
        let mut app = make_app(vaults);

        app.pull(Some("v1".to_string())).await.unwrap();

        assert_eq!(
            std::fs::read(local_root.path().join("a.txt")).unwrap(),
            b"remote-data"
        );
    }
}
