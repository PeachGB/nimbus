use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

use crate::{
    VaultResult,
    config::VaultConfig,
    error::VaultError,
    object::{Object, ObjectId, SyncDecision},
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
impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("name", &self.name)
            .field("objects", &self.objects)
            .field("root", &self.root)
            .finish()
    }
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
    /// Like [`Self::get`], but always asks the origin and refreshes the cache with the answer.
    /// For anything that must not act on outdated metadata: sync decisions, overwrite checks,
    /// saving an edit back.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nimbus_vault::vault::Vault;
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let current = vault.get_fresh("notes.txt").await?; // never served from the cache
    /// # let _ = current;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_fresh(&self, id: impl Into<ObjectId>) -> VaultResult<Object> {
        let id = id.into();
        match self.origin.get(&id).await {
            Ok(object) => {
                self.objects.lock().await.insert(id, object.clone());
                Ok(object)
            }
            Err(e) => {
                self.objects.lock().await.remove(&id);
                Err(e)
            }
        }
    }

    /// Whether cache key `cached` is `id` itself or something under it. Ids are compared with
    /// any leading `/` stripped, since origins disagree on whether to keep one.
    fn is_same_or_descendant(cached: &ObjectId, id: &ObjectId) -> bool {
        let cached = cached.as_str().trim_start_matches('/');
        let id = id.as_str().trim_start_matches('/').trim_end_matches('/');
        id.is_empty()
            || cached == id
            || (cached.starts_with(id) && cached[id.len()..].starts_with('/'))
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
        let result = self.origin.send(object, payload).await;
        // Evicted on failure too: a failed send may still have changed the object (an origin
        // that isn't atomic), and the next `get` re-reading it is cheap insurance.
        self.objects.lock().await.remove(&object.get_id());
        result
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
    /// Moves `id` under `new_parent` as `new_name` through the origin's native
    /// [`Origin::rename`]. That's atomic where the origin supports it, for example a single
    /// `rename(2)` for `OriginFileSystem`. Returns [`VaultError::Unsupported`] when the origin
    /// can't, and callers then fall back to copy-then-delete.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nimbus_vault::{object::ObjectId, vault::Vault};
    /// # async fn example(vault: Vault) -> nimbus_vault::VaultResult<()> {
    /// let id = vault.find("draft.txt".into()).await?;
    /// let archive = vault.find("archive".into()).await?;
    /// vault.rename(&id, &archive, "final.txt").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn rename(
        &self,
        id: &ObjectId,
        new_parent: &ObjectId,
        new_name: &str,
    ) -> VaultResult<Object> {
        let moved = self.origin.rename(id, new_parent, new_name).await?;
        let mut cache = self.objects.lock().await;
        cache.retain(|cached, _| !Self::is_same_or_descendant(cached, id));
        cache.insert(moved.get_id(), moved.clone());
        Ok(moved)
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
        let mut cache = self.objects.lock().await;
        cache.retain(|cached, _| !Self::is_same_or_descendant(cached, id));
        Ok(())
    }
    /// Recursively syncs `id`'s subtree from `remote` into this vault's origin, and reports
    /// what it did.
    ///
    /// Each of `remote`'s children under `id` is compared with this vault's copy via
    /// [`Object::compare`]:
    /// - a missing or older local file is copied (`put`, then `fetch` + `send`). The copy keeps
    ///   the remote's modification time, so pulling again finds nothing to do;
    /// - a local file that is **newer** than the remote one is left alone and listed in
    ///   [`SyncReport::conflicts`] (see [`Self::pull_with`] to override);
    /// - a file on one side and a directory on the other is always a conflict;
    /// - a missing directory is created, and directories are recursed into.
    ///
    /// Names coming from `remote` are validated, so a hostile listing can't write outside the
    /// vault. Errors other than `NotFound` short-circuit the whole walk.
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
    pub async fn pull(&self, id: &ObjectId, remote: &dyn Origin) -> VaultResult<SyncReport> {
        self.pull_with(id, remote, SyncOptions::default()).await
    }
    /// [`Self::pull`] with explicit [`SyncOptions`] — e.g. `force` to overwrite local files
    /// that are newer than the remote's.
    pub async fn pull_with(
        &self,
        id: &ObjectId,
        remote: &dyn Origin,
        options: SyncOptions,
    ) -> VaultResult<SyncReport> {
        let mut report = SyncReport::default();
        let result = sync_tree(remote, self.origin.as_ref(), id, options, &mut report).await;
        // Writes went straight to the origin; nothing cached about this vault can be trusted.
        self.objects.lock().await.clear();
        result.map(|()| report)
    }
    /// The mirror image of [`Self::pull`]: recursively syncs `id`'s subtree from this vault's
    /// origin out to `remote`. Same comparison, conflict and recursion rules: a file that is
    /// newer on `remote` is reported, not overwritten (see [`Self::push_with`]).
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
    pub async fn push(&self, id: &ObjectId, remote: &dyn Origin) -> VaultResult<SyncReport> {
        self.push_with(id, remote, SyncOptions::default()).await
    }
    /// [`Self::push`] with explicit [`SyncOptions`] — e.g. `force` to overwrite remote files
    /// that are newer than this vault's.
    pub async fn push_with(
        &self,
        id: &ObjectId,
        remote: &dyn Origin,
        options: SyncOptions,
    ) -> VaultResult<SyncReport> {
        let mut report = SyncReport::default();
        sync_tree(self.origin.as_ref(), remote, id, options, &mut report).await?;
        Ok(report)
    }
}

/// Options for [`Vault::pull_with`] / [`Vault::push_with`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncOptions {
    /// Overwrite destination files that are newer than the source instead of reporting them as
    /// conflicts. A file/directory mismatch is still never overwritten.
    pub force: bool,
}

/// One object a sync left alone because copying it could lose data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConflict {
    /// The object's id (the same on both sides).
    pub id: ObjectId,
    /// Why it was skipped, e.g. `"newer at the destination"`.
    pub reason: String,
    /// Whether [`SyncOptions::force`] would have copied it anyway. Only a destination that is
    /// newer can be forced; a file/directory mismatch or an entry that leaves the vault never is.
    pub can_force: bool,
}

/// What a [`Vault::pull`] / [`Vault::push`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Files copied to the destination, and directories created there.
    pub copied: Vec<ObjectId>,
    /// Files that were already identical, so nothing was transferred.
    pub unchanged: usize,
    /// Objects skipped to avoid losing data at the destination.
    pub conflicts: Vec<SyncConflict>,
}

impl SyncReport {
    /// Whether rerunning with [`SyncOptions::force`] would copy any of the conflicts.
    pub fn has_forceable_conflicts(&self) -> bool {
        self.conflicts.iter().any(|c| c.can_force)
    }
}

impl std::fmt::Display for SyncReport {
    /// A one-line summary, followed by one line per conflict:
    /// `copied 2, unchanged 5, 1 conflict` / `  notes.txt: newer at the destination`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.conflicts.len();
        write!(
            f,
            "copied {}, unchanged {}, {n} conflict{}",
            self.copied.len(),
            self.unchanged,
            if n == 1 { "" } else { "s" }
        )?;
        for conflict in &self.conflicts {
            write!(f, "\n  {}: {}", conflict.id, conflict.reason)?;
        }
        Ok(())
    }
}

/// The walk behind both `pull` and `push`: copies `id`'s subtree from `src` to `dst`,
/// following [`Object::compare`], and records everything in `report`.
async fn sync_tree(
    src: &dyn Origin,
    dst: &dyn Origin,
    id: &ObjectId,
    options: SyncOptions,
    report: &mut SyncReport,
) -> VaultResult<()> {
    let children = src.list(id).await?;
    sync_children(src, dst, id, children, options, report).await
}

/// Syncs each of `children` (already listed from `src` under `parent`). An entry that resolves
/// outside its vault on either side, such as a symlink in `$HOME` pointing elsewhere, comes
/// back as `VaultError::InvalidId`. That entry is skipped and reported as a conflict, and the
/// rest of the walk carries on. Any other error still ends the sync.
async fn sync_children(
    src: &dyn Origin,
    dst: &dyn Origin,
    parent: &ObjectId,
    children: Vec<Object>,
    options: SyncOptions,
    report: &mut SyncReport,
) -> VaultResult<()> {
    for mut src_obj in children {
        let src_id = src_obj.get_id();
        // Also enforced by every origin's `put`; checked here so the error names the culprit
        // before anything is written.
        crate::object::validate_name(&src_obj.get_name())?;
        match sync_entry(src, dst, parent, &mut src_obj, options, report).await {
            Ok(()) => {}
            Err(VaultError::InvalidId(_)) => report.conflicts.push(SyncConflict {
                id: src_id,
                reason: "leaves the vault (a symlink pointing outside it); skipped".to_string(),
                can_force: false,
            }),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Syncs one source entry into `parent` on `dst`. Everything that can be refused (listing a
/// directory, opening a file) happens *before* anything is created on the destination, so a
/// skipped entry leaves nothing behind there.
async fn sync_entry(
    src: &dyn Origin,
    dst: &dyn Origin,
    parent: &ObjectId,
    src_obj: &mut Object,
    options: SyncOptions,
    report: &mut SyncReport,
) -> VaultResult<()> {
    let src_id = src_obj.get_id();
    let dst_obj = match dst.get(&src_id).await {
        Ok(object) => Some(object),
        Err(VaultError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let is_dir = matches!(src_obj, Object::Branch { .. } | Object::Root { .. });
    let decision = match &dst_obj {
        None => SyncDecision::Unknown,
        Some(existing) => src_obj.compare(existing),
    };
    let conflict = |reason: &str, can_force: bool| SyncConflict {
        id: src_id.clone(),
        reason: reason.to_string(),
        can_force,
    };

    match decision {
        SyncDecision::KindMismatch => {
            report.conflicts.push(conflict(
                if is_dir {
                    "a directory here, a file at the destination"
                } else {
                    "a file here, a directory at the destination"
                },
                false,
            ));
            return Ok(());
        }
        SyncDecision::DestNewer if !options.force => {
            report
                .conflicts
                .push(conflict("newer at the destination", true));
            return Ok(());
        }
        SyncDecision::Same if !is_dir => {
            report.unchanged += 1;
            return Ok(());
        }
        _ => {}
    }

    if is_dir {
        let children = src.list(&src_id).await?;
        if dst_obj.is_none() {
            dst.put(src_obj, parent).await?;
            report.copied.push(src_id.clone());
        }
        return Box::pin(sync_children(src, dst, &src_id, children, options, report)).await;
    }

    let payload = src.fetch(&src_id).await?;
    let source_meta = src_obj.get_meta();
    let mut stored = dst.put(src_obj, parent).await?;
    // Write with the *source's* metadata: the destination then takes the source's
    // modification time, and the next sync sees the two as identical.
    if let Some(meta) = source_meta {
        stored.set_meta(meta);
    }
    dst.send(&stored, payload).await?;
    report.copied.push(stored.get_id());
    Ok(())
}

#[cfg(test)]
#[path = "tests/vault.rs"]
mod tests;
