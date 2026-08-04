use anyhow::{Result, anyhow};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

use bytes::Bytes;
use nimbus_vault::{
    config::OriginConfig,
    object::{Metadata, Object, ObjectId},
    origin::ByteStream,
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

#[derive(Debug)]
pub struct App {
    vaults: HashMap<String, Vault>,
    vault_configs: HashMap<String, PathBuf>,
    cwd: ObjectId,
    cwd_path: PathBuf,
    current_vault: Option<String>,
    local_root: Option<PathBuf>,
    local_root_canonical: Option<PathBuf>,
    /// Where [`Self::save`] persists the vault registry. A field rather than a constant so
    /// tests can point it at a scratch file instead of clobbering the real session state.
    state_path: PathBuf,
}

/// Where a resolved `cp`/`mv` destination points.
enum Destination {
    /// An existing directory: the object lands inside it, keeping its name.
    Into(ObjectId),
    /// A path that doesn't exist yet: the object lands in this parent under this new name.
    As(ObjectId, String),
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
            state_path: Self::default_state_path(),
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
    fn default_state_path() -> PathBuf {
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

        let state_path = Self::default_state_path();
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
            state_path,
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
        let path = &self.state_path;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(&state)?)?;
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
        for o in self.list_cwd().await? {
            println!("{}", o.get_name());
        }
        Ok(())
    }
    pub fn vaults(&self) -> Result<()> {
        for k in self.vault_names() {
            println!("{}", k);
        }
        Ok(())
    }
    /// Returns the sorted names of all configured vaults.
    pub fn vault_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.vaults.keys().cloned().collect();
        names.sort();
        names
    }
    /// Lists the objects in the current vault's current working directory.
    pub async fn list_cwd(&self) -> Result<Vec<Object>> {
        let Some(current_vault) = &self.current_vault else {
            return Err(anyhow!("NO VAULT SELECTED"));
        };
        let Some(vault) = self.vaults.get(current_vault) else {
            return Err(anyhow!("COULDN'T GET CURRENT WORKING VAULT"));
        };
        Ok(vault.list(&self.cwd).await?)
    }
    /// Fetches the full payload of `id` (a child of `list_cwd`) from the current vault.
    pub async fn fetch_object_bytes(&self, id: ObjectId) -> Result<Vec<u8>> {
        let Some(current_vault) = &self.current_vault else {
            return Err(anyhow!("NO VAULT SELECTED"));
        };
        let Some(vault) = self.vaults.get(current_vault) else {
            return Err(anyhow!("COULDN'T GET CURRENT WORKING VAULT"));
        };
        let mut stream = vault.fetch(id).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk?);
        }
        Ok(bytes)
    }
    /// Writes `bytes` back as the contents of `id` in the current vault — the return trip for
    /// [`Self::fetch_object_bytes`], so an object edited outside the app can be saved back to
    /// wherever its origin actually keeps it.
    pub async fn write_object_bytes(&self, id: ObjectId, bytes: Vec<u8>) -> Result<()> {
        let Some(current_vault) = &self.current_vault else {
            return Err(anyhow!("NO VAULT SELECTED"));
        };
        let Some(vault) = self.vaults.get(current_vault) else {
            return Err(anyhow!("COULDN'T GET CURRENT WORKING VAULT"));
        };
        let object = vault.get(id).await?;
        let payload: ByteStream =
            Box::pin(futures::stream::once(async move { Ok(Bytes::from(bytes)) }));
        vault.send(&object, payload).await?;
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

    /// Registers the vault described by the config file at `cfg`.
    ///
    /// Re-registering the *same* config path is allowed and reloads it, which is how an edit to
    /// a config file is picked up. A different config claiming a name that's already taken is
    /// refused: silently replacing the entry would leave the original vault unreachable, with
    /// nothing pointing at its data any more. [`Self::forget_vault`] is the way to free a name.
    pub fn new_vault(&mut self, cfg: PathBuf) -> Result<()> {
        let vault = Vault::new(cfg.clone())?;
        let name = vault.get_name().clone();

        if let Some(existing) = self.vault_configs.get(&name)
            && existing != &cfg
        {
            return Err(anyhow!(
                "VAULT '{}' IS ALREADY REGISTERED (config: {}) — FORGET IT FIRST OR PICK ANOTHER NAME",
                name,
                existing.display()
            ));
        }

        self.vault_configs.insert(name.clone(), cfg);
        self.vaults.insert(name, vault);
        self.save()
    }

    /// Drops `name` from the registry, so nimbus stops tracking it. Purely a bookkeeping
    /// operation: the vault's config file and everything in its origin are left untouched, and
    /// `new` with the same config re-registers it.
    ///
    /// `LOCAL` can't be forgotten — [`Self::init`] recreates it from `cli_config` on the next
    /// run, so it would silently come back; turn off `default_local_vault` instead.
    pub fn forget_vault(&mut self, name: String) -> Result<()> {
        if name == LOCAL_VAULT_NAME {
            return Err(anyhow!(
                "'{LOCAL_VAULT_NAME}' IS MANAGED BY cli_config — SET default_local_vault = false TO DROP IT"
            ));
        }
        if !self.vaults.contains_key(&name) {
            return Err(anyhow!("VAULT '{}' DOESN'T EXITS", name));
        }
        self.vaults.remove(&name);
        self.vault_configs.remove(&name);
        // Leaving `current_vault` pointing at a vault that's gone would make every later
        // resolution fail with a confusing "couldn't find configuration" error.
        if self.current_vault.as_deref() == Some(name.as_str()) {
            self.cwd = ObjectId::from(APP_ROOT_ID);
            self.cwd_path = PathBuf::from("/");
            self.current_vault = None;
        }
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

        // Omitting the destination means the local vault's root, not the process's working
        // directory: `.` is only a valid destination when you happen to be standing inside the
        // local root, which makes the no-argument form fail everywhere else.
        let dest_path = match dest {
            Some(d) => self.resolve_local_path(&d)?,
            None => PathBuf::from("/"),
        };
        let dest_parent_id = local.find(dest_path).await?;

        let placed = local.put(&mut source_obj, &dest_parent_id).await?;

        if let Object::Leaf { .. } = placed {
            let bytes = source_vault.fetch(source_id).await?;
            local.send(&placed, bytes).await?;
        }

        Ok(())
    }
    /// Splits a `vault:path` spec into the vault it names and the path within it. The `vault:`
    /// prefix is what distinguishes a vault from a same-named sibling directory, so `docs` is
    /// always a relative path while `docs:` always addresses the vault called `docs`. A spec
    /// with no recognised `vault:` prefix is left whole as a path (so a stray colon in an
    /// object's name doesn't get mistaken for a vault prefix).
    fn split_vault_spec(&self, spec: &str) -> (Option<String>, String) {
        match spec.split_once(':') {
            Some((prefix, rest)) if self.vaults.contains_key(prefix) => {
                (Some(prefix.to_string()), rest.to_string())
            }
            _ => (None, spec.to_string()),
        }
    }

    /// Resolves a `vault:path` spec to the vault it lives in and its absolute path there.
    /// A vault-qualified spec resolves from *that* vault's root (there is no cwd in a vault
    /// you aren't standing in); an unqualified one resolves relative to `cwd_path` inside
    /// `fallback_vault`, or the current vault when that's `None`.
    fn resolve_spec(&self, spec: &str, fallback_vault: Option<&str>) -> Result<(String, PathBuf)> {
        match self.split_vault_spec(spec) {
            (Some(vault), path) => Ok((vault, Self::resolve_relative(Path::new("/"), &path))),
            (None, path) => {
                let vault = fallback_vault
                    .map(String::from)
                    .or_else(|| self.current_vault.clone())
                    .ok_or_else(|| anyhow!("NO VAULT SPECIFIED AND NO CURRENT VAULT SELECTED"))?;
                Ok((vault, Self::resolve_relative(&self.cwd_path, &path)))
            }
        }
    }

    /// Copies the object at `name` to `destination`. Both are `vault:path` specs (see
    /// `resolve_spec`): unqualified they resolve inside `vault` — or the current vault
    /// when that's omitted — relative to `cwd_path`, while a `vault:` prefix addresses another
    /// vault from its root, which is how a copy crosses vaults.
    ///
    /// The destination may be either an existing directory (the object lands inside it, keeping
    /// its name) or a path that doesn't exist yet (the object lands in its parent under that new
    /// name) — so `cp notes.txt backup:/inbox` and `cp notes.txt backup:/inbox/copy.txt` both
    /// work. See `resolve_destination`.
    ///
    /// The two sides may be backed by different origin types (e.g. an `fs` vault and one backed
    /// by another vault via `OriginVault`), since every step below (`find`/`get`/`put`/`fetch`/
    /// `send`) goes through the `Vault`/`Origin` trait rather than anything origin-specific. A
    /// directory is copied recursively via `deep_copy`, which writes each descendant
    /// straight to the destination's origin as it goes — there is no separate "sync" step, since
    /// `Vault::put` and `send` already write through to `Origin::put`/`send`.
    pub async fn cp(
        &mut self,
        name: String,
        destination: String,
        vault: Option<String>,
    ) -> Result<()> {
        self.transfer(name, destination, vault, false).await
    }

    /// Same as [`Self::cp`], but deletes the source (recursively, for a directory) once the copy
    /// has completed.
    pub async fn mv(
        &mut self,
        name: String,
        destination: String,
        vault: Option<String>,
    ) -> Result<()> {
        self.transfer(name, destination, vault, true).await
    }

    /// Shared body of [`Self::cp`] and [`Self::mv`]; `delete_source` is the only difference.
    async fn transfer(
        &mut self,
        name: String,
        destination: String,
        vault: Option<String>,
        delete_source: bool,
    ) -> Result<()> {
        let (source_vault_name, source_path) = self.resolve_spec(&name, vault.as_deref())?;
        let (dest_vault_name, dest_path) = self.resolve_spec(&destination, vault.as_deref())?;
        let same_vault = source_vault_name == dest_vault_name;

        // Note there is no exemption for `dest_path == source_path`: pasting a directory into
        // itself resolves to exactly that, and `deep_copy` would recurse into the copy it had
        // just made, forever. `starts_with` is component-wise, so `cp a.txt a.txt.bak` and
        // `cp docs docs2` are unaffected.
        if same_vault && dest_path.starts_with(&source_path) {
            return Err(anyhow!(
                "'{}' is inside '{}' — cannot copy or move an object into itself",
                dest_path.display(),
                source_path.display()
            ));
        }

        let source = self.vaults.get(&source_vault_name).ok_or_else(|| {
            anyhow!(
                "COULDN'T FIND CONFIGURATION FOR VAULT '{}'",
                source_vault_name
            )
        })?;
        let dest = self.vaults.get(&dest_vault_name).ok_or_else(|| {
            anyhow!(
                "COULDN'T FIND CONFIGURATION FOR VAULT '{}'",
                dest_vault_name
            )
        })?;

        let target = Self::resolve_destination(dest, &dest_path).await?;

        // Only meaningful when the object keeps its name: `cp a.txt .` would have `put`
        // truncate the file before `send` could read it, but `cp a.txt copy.txt` is fine.
        if same_vault
            && matches!(target, Destination::Into(_))
            && source_path.parent() == Some(dest_path.as_path())
        {
            return Err(anyhow!(
                "'{}' is already in '{}'",
                source_path.display(),
                dest_path.display()
            ));
        }

        let source_id = source.find(source_path).await?;
        let (dest_parent_id, rename_to) = match &target {
            Destination::Into(id) => (id, None),
            Destination::As(id, new_name) => (id, Some(new_name.as_str())),
        };

        Self::deep_copy(source, dest, source_id.clone(), dest_parent_id, rename_to).await?;
        if delete_source {
            source.delete(&source_id).await?;
        }
        Ok(())
    }

    /// Works out what a `cp`/`mv` destination path means: an existing directory to copy *into*,
    /// or a not-yet-existing path to copy *as*.
    ///
    /// An existing file is rejected rather than treated as a target to overwrite — `put`
    /// truncates, so a mistyped destination would silently destroy whatever was already there.
    async fn resolve_destination(dest: &Vault, dest_path: &Path) -> Result<Destination> {
        if let Ok(id) = dest.find(dest_path.to_path_buf()).await {
            return match dest.get(id.clone()).await? {
                Object::Branch { .. } | Object::Root { .. } => Ok(Destination::Into(id)),
                _ => Err(anyhow!("'{}' already exists", dest_path.display())),
            };
        }

        let parent = dest_path
            .parent()
            .ok_or_else(|| anyhow!("INVALID DESTINATION '{}'", dest_path.display()))?;
        let new_name = dest_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("INVALID DESTINATION '{}'", dest_path.display()))?
            .to_string();

        let parent_id = dest
            .find(parent.to_path_buf())
            .await
            .map_err(|_| anyhow!("'{}' doesn't exist", parent.display()))?;
        match dest.get(parent_id.clone()).await? {
            Object::Branch { .. } | Object::Root { .. } => Ok(Destination::As(parent_id, new_name)),
            _ => Err(anyhow!("'{}' is not a directory", parent.display())),
        }
    }

    /// Creates a directory at `path` (a `vault:path` spec, see `resolve_spec`),
    /// failing rather than clobbering anything already there.
    pub async fn mkdir(&mut self, path: String, vault: Option<String>) -> Result<()> {
        self.create(path, vault, |name| Object::Branch {
            name: name.to_string(),
            id: ObjectId::from(name),
            meta: Metadata::new(),
            children: None,
        })
        .await
    }

    /// Creates an empty file at `path` (a `vault:path` spec), failing rather than clobbering
    /// anything already there — which is why this isn't `touch`'s usual "create or update the
    /// timestamp": an origin's `put` truncates, so silently succeeding on an existing path
    /// would empty it.
    pub async fn touch(&mut self, path: String, vault: Option<String>) -> Result<()> {
        self.create(path, vault, |name| Object::Leaf {
            name: name.to_string(),
            id: ObjectId::from(name),
            meta: Metadata::new(),
        })
        .await
    }

    /// Shared body of [`Self::mkdir`] and [`Self::touch`]: resolves `path`, refuses to overwrite,
    /// and `put`s whatever `build` makes of the final path component into the parent directory.
    async fn create(
        &mut self,
        path: String,
        vault: Option<String>,
        build: impl Fn(&str) -> Object,
    ) -> Result<()> {
        let (vault_name, full_path) = self.resolve_spec(&path, vault.as_deref())?;
        let name = full_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("INVALID PATH '{}'", full_path.display()))?
            .to_string();
        let parent = full_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));

        let v = self
            .vaults
            .get(&vault_name)
            .ok_or_else(|| anyhow!("COULDN'T FIND CONFIGURATION FOR VAULT '{}'", vault_name))?;

        if v.find(full_path.clone()).await.is_ok() {
            return Err(anyhow!("'{}' already exists", full_path.display()));
        }
        let parent_id = v.find(parent).await?;

        // The id is a placeholder: `Origin::put` assigns the real one from the destination
        // parent and this object's name.
        let mut object = build(&name);
        v.put(&mut object, &parent_id).await?;
        Ok(())
    }

    /// Renames the object at `path` (a `vault:path` spec) to `new_name`, in place.
    ///
    /// `Origin` has no rename primitive — `put` always writes an object under its own
    /// `get_name()` — so this is a copy under the new name followed by deleting the original.
    /// That's correct for every origin but costs a full data copy, which is worth knowing for
    /// large objects on a remote origin.
    pub async fn rename(
        &mut self,
        path: String,
        new_name: String,
        vault: Option<String>,
    ) -> Result<()> {
        if new_name.is_empty() || new_name.contains('/') {
            return Err(anyhow!(
                "INVALID NAME '{}': a name cannot be empty or contain '/'",
                new_name
            ));
        }

        let (vault_name, source_path) = self.resolve_spec(&path, vault.as_deref())?;
        let parent = source_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow!("CANNOT RENAME THE VAULT ROOT"))?;

        let v = self
            .vaults
            .get(&vault_name)
            .ok_or_else(|| anyhow!("COULDN'T FIND CONFIGURATION FOR VAULT '{}'", vault_name))?;

        let target_path = parent.join(&new_name);
        if v.find(target_path.clone()).await.is_ok() {
            return Err(anyhow!("'{}' already exists", target_path.display()));
        }

        let source_id = v.find(source_path).await?;
        let parent_id = v.find(parent).await?;

        Self::deep_copy(v, v, source_id.clone(), &parent_id, Some(&new_name)).await?;
        v.delete(&source_id).await?;
        Ok(())
    }

    /// Recursively copies `source_id`'s subtree from `source` into `dest_parent_id` within
    /// `dest`, optionally landing the top-level object under a different name (`rename_to`,
    /// which is how [`Self::rename`] reuses this). A `Leaf` is put then its bytes fetched/sent;
    /// a `Branch` is put (creating the directory/entry at the destination) and then each of its
    /// children is copied in turn — `Vault::put` on a origin only creates the entry itself, it
    /// doesn't know about children, so recursion here is what actually moves a directory's
    /// contents rather than leaving an empty stub at the destination.
    async fn deep_copy(
        source: &Vault,
        dest: &Vault,
        source_id: ObjectId,
        dest_parent_id: &ObjectId,
        rename_to: Option<&str>,
    ) -> Result<Object> {
        let mut source_obj = source.get(source_id.clone()).await?;
        if let Some(new_name) = rename_to {
            source_obj.with_name(new_name.to_string());
        }
        let placed = dest.put(&mut source_obj, dest_parent_id).await?;

        match &placed {
            Object::Leaf { .. } => {
                let bytes = source.fetch(source_id).await?;
                dest.send(&placed, bytes).await?;
            }
            Object::Branch { .. } => {
                let placed_id = placed.get_id();
                for child in source.list(source_id).await? {
                    // Descendants keep their own names — only the top level is renamed.
                    Box::pin(Self::deep_copy(
                        source,
                        dest,
                        child.get_id(),
                        &placed_id,
                        None,
                    ))
                    .await?;
                }
            }
            Object::Root { .. } => {}
        }

        Ok(placed)
    }
    /// Deletes the object at `path` (a `vault:path` spec, see `resolve_spec`).
    ///
    /// Without `force`, a directory with anything in it is refused, so a mistyped path can't
    /// take a subtree down with it. That check only applies to directories: asking an origin to
    /// list a leaf is an error rather than an empty listing, so running it unconditionally would
    /// make every file undeletable without `force`.
    pub async fn delete(&mut self, path: String, vault: Option<String>, force: bool) -> Result<()> {
        let (vault_name, target_path) = self.resolve_spec(&path, vault.as_deref())?;
        if target_path == Path::new("/") {
            return Err(anyhow!("CANNOT DELETE THE VAULT ROOT"));
        }

        let v = self
            .vaults
            .get(&vault_name)
            .ok_or_else(|| anyhow!("COULDN'T FIND CONFIGURATION FOR VAULT '{}'", vault_name))?;

        let target_id = v.find(target_path.clone()).await?;

        if !force
            && matches!(
                v.get(target_id.clone()).await?,
                Object::Branch { .. } | Object::Root { .. }
            )
            && !v.list(target_id.clone()).await?.is_empty()
        {
            return Err(anyhow!(
                "'{}' is not empty — use --force to delete recursively",
                target_path.display()
            ));
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
