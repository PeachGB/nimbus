use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

use crate::{
    VaultResult,
    config::VaultConfig,
    error::VaultError,
    object::{Object, ObjectId},
    origin::{ByteStream, Origin},
};

/// A tree-like view over an `Origin`, keyed from a single `root` `ObjectId`.
/// Caches fetched `Object`s in memory so repeat `get`/`list` calls can skip the origin.
///
/// # Examples
///
/// Open a vault from a config file, resolve a path, and list its contents:
///
/// ```no_run
/// use nimbus_vault::vault::Vault;
///
/// # async fn example() -> nimbus_vault::VaultResult<()> {
/// let vault = Vault::new("vault.toml".into())?;
///
/// let id = vault.find("photos/2024".into()).await?;
/// for child in vault.list(id).await? {
///     println!("{}", child.get_name());
/// }
/// # Ok(())
/// # }
/// ```
///
/// Read an object's payload and write it back under a new name:
///
/// ```no_run
/// use futures::StreamExt;
/// use nimbus_vault::{object::{Metadata, Object, ObjectId}, vault::Vault};
///
/// # async fn example() -> nimbus_vault::VaultResult<()> {
/// let vault = Vault::new("vault.toml".into())?;
///
/// // read
/// let object = vault.get("notes.txt").await?;
/// let mut stream = vault.fetch(object.get_id()).await?;
/// let mut bytes = Vec::new();
/// while let Some(chunk) = stream.next().await {
///     bytes.extend_from_slice(&chunk?);
/// }
///
/// // write under a new id
/// let mut copy = Object::Leaf {
///     name: "notes-copy.txt".to_string(),
///     id: ObjectId::from("notes-copy.txt"),
///     meta: Metadata::new(),
/// };
/// let root = vault.find("".into()).await?;
/// vault.put(&mut copy, &root).await?;
/// # Ok(())
/// # }
/// ```
///
/// Sync a vault with a remote origin in both directions:
///
/// ```no_run
/// use nimbus_vault::{config::OriginConfig, vault::Vault};
///
/// # async fn example() -> nimbus_vault::VaultResult<()> {
/// let vault = Vault::new("vault.toml".into())?;
/// let remote = OriginConfig::from_file("remote.toml".into())?;
/// let root = vault.find("".into()).await?;
///
/// vault.pull(&root, remote.as_ref()).await?; // bring local up to date with remote
/// vault.push(&root, remote.as_ref()).await?; // push local changes back out
/// # Ok(())
/// # }
/// ```
pub struct Vault {
    name: String,
    origin: Arc<dyn Origin>,
    objects: Mutex<HashMap<ObjectId, Object>>,
    root: ObjectId,
}

impl Vault {
    ///public constructor by field for vault
    pub fn from_parts(name: String, origin: Arc<dyn Origin>, root: ObjectId) -> VaultResult<Self> {
        Ok(Vault {
            name,
            origin,
            objects: Mutex::new(HashMap::new()),
            root,
        })
    }
    /// Reads a `VaultConfig` from the TOML file at `from` and builds the `Vault` it describes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use nimbus_vault::vault::Vault;
    ///
    /// let vault = Vault::new("vault.toml".into())?;
    /// println!("opened {}", vault.get_name());
    /// # Ok::<(), nimbus_vault::error::VaultError>(())
    /// ```
    pub fn new(from: PathBuf) -> VaultResult<Self> {
        let (name, root_id, origin) = VaultConfig::build(from)?;
        Self::from_parts(name, Arc::from(origin), root_id)
    }

    #[cfg(test)]
    fn from_origin(name: String, root_id: ObjectId, origin: Arc<dyn Origin>) -> Self {
        Vault {
            name,
            origin,
            root: root_id,
            objects: Mutex::new(HashMap::new()),
        }
    }
    /// Returns the vault's configured name.
    pub fn get_name(&self) -> &String {
        &self.name
    }
    /// Returns the vault root id
    pub fn get_id(&self) -> ObjectId {
        self.root.clone()
    }
    /// Returns a clone of the `Arc` to the vault's `Origin`.
    pub fn get_origin(&self) -> Arc<dyn Origin> {
        self.origin.clone()
    }
    /// Resolves a filesystem-style `path` to an `ObjectId`, starting at `root` and walking
    /// each component via `list`, matching on child name. Errors with `VaultError::NotFound`
    /// as soon as a component has no matching child.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nimbus_vault::vault::Vault;
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let id = vault.find("photos/2024/summer.jpg".into()).await?;
    /// let root = vault.find("".into()).await?; // empty path resolves to the vault's root
    /// # Ok(())
    /// # }
    /// ```
    pub async fn find(&self, path: PathBuf) -> VaultResult<ObjectId> {
        let mut current = self.root.clone();

        for component in path.components() {
            let Some(name) = component.as_os_str().to_str() else {
                return Err(VaultError::Generic("invalid path component".into()));
            };
            if name == "/" {
                continue;
            }

            let children = self.list(current.clone()).await?;

            let child = children
                .iter()
                .find(|c| c.get_name() == name)
                .ok_or_else(|| VaultError::NotFound(name.to_string()))?;

            current = child.get_id();
        }

        Ok(current)
    }
    /// Streams the payload for `id` straight from the origin. Not cached, since payloads
    /// aren't kept in the in-memory `Object` cache.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use futures::StreamExt;
    /// # use nimbus_vault::vault::Vault;
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let mut stream = vault.fetch("notes.txt").await?;
    /// let mut bytes = Vec::new();
    /// while let Some(chunk) = stream.next().await {
    ///     bytes.extend_from_slice(&chunk?);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch(&self, id: impl Into<ObjectId>) -> VaultResult<ByteStream> {
        let id = id.into();
        self.origin.fetch(&id).await
    }
    /// Lists `id`'s children via the origin, caching each child and recording the resulting
    /// child id list on `id`'s own cache entry (if it's already cached as a `Branch`/`Root`).
    /// Always hits the origin — unlike `get`, this does not read from the cache.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nimbus_vault::vault::Vault;
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// for child in vault.list("photos").await? {
    ///     println!("{} ({:?} bytes)", child.get_name(), child.get_meta().and_then(|m| m.size));
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list(&self, id: impl Into<ObjectId>) -> VaultResult<Vec<Object>> {
        let id = id.into();
        let children = self.origin.list(&id).await?;
        let child_ids: Vec<ObjectId> = children.iter().map(|child| child.get_id()).collect();
        let mut cache = self.objects.lock().await;
        for child in &children {
            cache.insert(child.get_id(), child.clone());
        }
        if let Some(Object::Branch { children: c, .. }) | Some(Object::Root { children: c, .. }) =
            cache.get_mut(&id)
        {
            *c = Some(child_ids)
        }
        Ok(children)
    }
    /// Returns `id`'s `Object`, serving it from the in-memory cache when present and otherwise
    /// fetching it from the origin and caching the result.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nimbus_vault::vault::Vault;
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let object = vault.get("notes.txt").await?; // hits the origin
    /// let same = vault.get("notes.txt").await?;   // served from the cache
    /// # let _ = same;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get(&self, id: impl Into<ObjectId>) -> VaultResult<Object> {
        let id = id.into();

        let cache = self.objects.lock().await;
        if let Some(object) = cache.get(&id) {
            return Ok(object.clone());
        }
        drop(cache);
        let object = self.origin.get(&id).await?;
        let mut cache = self.objects.lock().await;
        cache.insert(id, object.clone());
        Ok(object)
    }
    /// Streams `payload` to the origin as `object`'s contents. Not cached, mirroring `fetch`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use bytes::Bytes;
    /// use futures::stream;
    /// # use nimbus_vault::{object::{Metadata, Object, ObjectId}, vault::Vault};
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let mut object = Object::Leaf {
    ///     name: "notes.txt".to_string(),
    ///     id: ObjectId::from("notes.txt"),
    ///     meta: Metadata::new(),
    /// };
    /// let root = vault.find("".into()).await?;
    /// vault.put(&mut object, &root).await?; // create the object first
    ///
    /// let payload = Box::pin(stream::once(async { Ok(Bytes::from_static(b"hello")) }));
    /// vault.send(&object, payload).await?; // then write its contents
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()> {
        self.origin.send(object, payload).await
    }
    /// Writes `object` to the origin and caches it under its own id.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nimbus_vault::{object::{Metadata, Object, ObjectId}, vault::Vault};
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let mut folder = Object::Branch {
    ///     name: "photos".to_string(),
    ///     id: ObjectId::from("photos"),
    ///     meta: Metadata::new(),
    ///     children: None,
    /// };
    /// let root = vault.find("".into()).await?;
    /// vault.put(&mut folder, &root).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        let stored = self.origin.put(object, destination).await?;
        self.objects
            .lock()
            .await
            .insert(stored.get_id(), stored.clone());
        Ok(stored)
    }
    /// Deletes `id` from the origin and evicts it from the cache.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nimbus_vault::{object::ObjectId, vault::Vault};
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let id = vault.find("notes.txt".into()).await?;
    /// vault.delete(&id).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        self.origin.delete(id).await?;
        self.objects.lock().await.remove(id);
        Ok(())
    }
    /// Recursively syncs `id`'s subtree from `remote` into this vault's origin.
    /// For each of `remote`'s children under `id`: fetches this vault's copy (if any) and
    /// compares metadata via `Object::changed`; a missing local copy or diverging metadata
    /// triggers a `put` (and, for `Leaf`s, a `fetch`+`send` of the payload). `Branch`/`Root`
    /// children are then recursed into regardless of whether they themselves changed, so their
    /// descendants are still visited. Errors from `remote`/`self` other than `NotFound`
    /// short-circuit the whole walk.
    ///
    /// # Examples
    ///
    /// Pull from a config-defined origin:
    ///
    /// ```no_run
    /// # use nimbus_vault::{config::OriginConfig, vault::Vault};
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let remote = OriginConfig::from_file("remote.toml".into())?;
    /// let root = vault.find("".into()).await?;
    /// vault.pull(&root, remote.as_ref()).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Pull from another vault, via `OriginVault`:
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use nimbus_vault::{origin::vault::OriginVault, vault::Vault};
    ///
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let upstream = Arc::new(Vault::new("upstream.toml".into())?);
    /// let upstream_as_origin = OriginVault::new(upstream);
    ///
    /// let root = vault.find("".into()).await?;
    /// vault.pull(&root, &upstream_as_origin).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn pull(&self, id: &ObjectId, remote: &dyn Origin) -> VaultResult<()> {
        let remote_children = remote.list(id).await?;
        for mut remote_obj in remote_children {
            let remote_id = remote_obj.get_id();
            let needs_sync = match self.get(remote_id.clone()).await {
                Ok(local_obj) => local_obj.changed(&remote_obj),
                Err(VaultError::NotFound(_)) => true,
                Err(e) => return Err(e),
            };

            if needs_sync {
                let stored = self.put(&mut remote_obj, id).await?;
                if let Object::Leaf { .. } = stored {
                    let bytes = remote.fetch(&remote_id).await?;
                    self.send(&stored, bytes).await?;
                }
            }
            if matches!(remote_obj, Object::Branch { .. } | Object::Root { .. }) {
                Box::pin(self.pull(&remote_id, remote)).await?;
            }
        }
        Ok(())
    }
    /// The mirror image of `pull`: recursively syncs `id`'s subtree from this vault's origin
    /// out to `remote`, walking children via `self.list` instead of `remote.list` and writing
    /// through `remote.put`/`remote.send` instead of `self.put`/`self.send`. Same change
    /// detection and recursion rules as `pull` apply.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nimbus_vault::{config::OriginConfig, vault::Vault};
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let backup = OriginConfig::from_file("backup.toml".into())?;
    /// let root = vault.find("".into()).await?;
    /// vault.push(&root, backup.as_ref()).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn push(&self, id: &ObjectId, remote: &dyn Origin) -> VaultResult<()> {
        let local_children = self.list(id.clone()).await?;

        for mut local_obj in local_children {
            let local_id = local_obj.get_id();

            let needs_sync = match remote.get(&local_id).await {
                Ok(remote_obj) => local_obj.changed(&remote_obj),
                Err(VaultError::NotFound(_)) => true,
                Err(e) => return Err(e),
            };

            if needs_sync {
                let stored = remote.put(&mut local_obj, id).await?;
                if let Object::Leaf { .. } = stored {
                    let bytes = self.fetch(local_id.clone()).await?;
                    remote.send(&stored, bytes).await?;
                }
            }

            if matches!(local_obj, Object::Branch { .. } | Object::Root { .. }) {
                Box::pin(self.push(&local_id, remote)).await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/vault.rs"]
mod tests;
