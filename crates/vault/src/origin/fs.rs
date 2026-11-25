use futures::StreamExt;
use std::path::{Component, Path, PathBuf};
use tokio::fs::{self, OpenOptions};
use tokio_util::io::ReaderStream;

use crate::{
    VaultResult,
    error::VaultError,
    fsutil::{TEMP_PREFIX, sync_parent_dir, temp_sibling},
    object::{Metadata, Object, ObjectId, validate_name},
    origin::{ByteStream, Origin},
};

/// An [`crate::origin::Origin`] backed by a directory on disk, via `tokio::fs`. `ObjectId`s
/// are relative paths resolved against `root`.
///
/// # Examples
///
/// ```
/// use nimbus_vault::{
///     object::{Metadata, Object, ObjectId},
///     origin::{Origin, fs::OriginFileSystem},
/// };
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// let origin = OriginFileSystem::new(dir.path().to_path_buf());
///
/// let mut file = Object::Leaf {
///     name: "notes.txt".to_string(),
///     id: ObjectId::from("notes.txt"),
///     meta: Metadata::new(),
/// };
/// origin.put(&mut file, &ObjectId::from("")).await?; // creates the file
/// assert!(dir.path().join("notes.txt").is_file());
///
/// let fetched = origin.get(&ObjectId::from("notes.txt")).await?;
/// assert!(matches!(fetched, Object::Leaf { .. }));
/// # Ok(())
/// # }
/// ```
///
/// Declaratively, via `[origin_config]` in a vault's TOML config:
///
/// ```toml
/// [origin_config]
/// type = "fs"
/// root = "/srv/data"
/// ```
pub struct OriginFileSystem {
    root: PathBuf,
}
impl OriginFileSystem {
    /// Builds an `OriginFileSystem` rooted at `root`; every `ObjectId` is resolved relative
    /// to it.
    pub fn new(root: PathBuf) -> Self {
        OriginFileSystem { root }
    }
    fn relative(id: &ObjectId) -> String {
        let id_str = id.to_string();
        id_str
            .strip_prefix('/')
            .unwrap_or(&id_str)
            .trim_end_matches('/')
            .to_string()
    }

    /// Resolves `id` to a path under `root`, refusing anything that would leave it: `..`
    /// segments, an absolute path (`//etc` once the leading `/` is stripped), or a NUL byte.
    /// Every operation goes through here, so a hostile id — from a daemon client, or from the
    /// remote side of a `pull` — can't name a file outside the vault.
    fn path(&self, id: &ObjectId) -> VaultResult<PathBuf> {
        let relative = Self::relative(id);
        let escapes = || VaultError::InvalidId(id.to_string());
        if relative.contains('\0') {
            return Err(escapes());
        }
        for component in Path::new(&relative).components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(escapes());
                }
            }
        }
        Ok(if relative.is_empty() {
            self.root.clone()
        } else {
            self.root.join(relative)
        })
    }

    /// Refuses `path` when resolving symlinks takes it outside `root`. [`Self::path`] only
    /// checks the id's text, so a symlink stored in the vault (`docs -> /etc`) would otherwise
    /// let `docs/passwd` be read, overwritten or deleted.
    ///
    /// With `follow_final` the operation goes *through* the last component (reading a file,
    /// listing a directory, creating something inside it), so the fully resolved path must
    /// stay inside. A dangling symlink there is refused: writing through it would create its
    /// target wherever it points. Without `follow_final` the operation acts on the entry
    /// itself (`get` describes a link, `delete` removes the link, `send` and `rename` replace
    /// it), so only its parent must resolve inside. Paths that don't exist yet are judged by
    /// their nearest existing ancestor.
    fn ensure_contained(&self, id: &ObjectId, path: &Path, follow_final: bool) -> VaultResult<()> {
        let escapes = || VaultError::InvalidId(id.to_string());
        // The root itself is inside by definition (its parent, checked below for entries,
        // isn't).
        if path == self.root {
            return Ok(());
        }
        // A missing root fails every operation with `NotFound` on its own.
        let Ok(root) = std::fs::canonicalize(&self.root) else {
            return Ok(());
        };
        let target = match follow_final {
            true => path,
            false => match path.parent() {
                Some(parent) => parent,
                None => return Ok(()),
            },
        };
        if follow_final
            && std::fs::symlink_metadata(target).is_ok_and(|m| m.file_type().is_symlink())
            && std::fs::canonicalize(target).is_err()
        {
            return Err(escapes());
        }
        let mut probe = target.to_path_buf();
        let resolved = loop {
            match std::fs::canonicalize(&probe) {
                Ok(resolved) => break resolved,
                Err(_) => match probe.parent() {
                    Some(parent) => probe = parent.to_path_buf(),
                    None => return Ok(()),
                },
            }
        };
        match resolved.starts_with(&root) {
            true => Ok(()),
            false => Err(escapes()),
        }
    }

    fn child_id(parent: &ObjectId, name: &str) -> ObjectId {
        let parent_relative = Self::relative(parent);
        if parent_relative.is_empty() {
            ObjectId::from(name)
        } else {
            ObjectId::from(format!("{parent_relative}/{name}"))
        }
    }
}

#[async_trait::async_trait]
impl Origin for OriginFileSystem {
    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        let path = self.path(id)?;
        self.ensure_contained(id, &path, false)?;
        let entry = fs::symlink_metadata(&path)
            .await
            .map_err(|_| VaultError::NotFound(id.to_string()))?;
        // A symlink is described by what it points at, except when that is outside the root
        // (or nowhere): then it is described as itself, a plain entry, so its target's size and
        // time don't leak and nothing treats it as a directory to walk into. Reading through it
        // is refused by `fetch`/`list`.
        let follow =
            !entry.file_type().is_symlink() || self.ensure_contained(id, &path, true).is_ok();
        let meta = match follow {
            true => fs::metadata(&path).await.unwrap_or(entry),
            false => entry,
        };
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| id.to_string());
        let modified = meta
            .modified()
            .map(chrono::DateTime::<chrono::Utc>::from)
            .map_err(|_| VaultError::OriginError("Failed to get modified time".to_string()))?;
        let metadata = Metadata {
            size: Some(meta.len()),
            content_type: None,
            modified: Some(modified),
            extra: Default::default(),
        };
        let object = match meta.is_dir() {
            true => Object::Branch {
                id: id.clone(),
                name,
                meta: metadata,
                children: None,
            },
            false => Object::Leaf {
                id: id.clone(),
                name,
                meta: metadata,
            },
        };

        Ok(object)
    }

    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>> {
        let path = self.path(id)?;
        self.ensure_contained(id, &path, true)?;
        let mut entries = fs::read_dir(&path)
            .await
            .map_err(|_| VaultError::NotFound(id.to_string()))?;

        let mut objects: Vec<Object> = Vec::new();

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| VaultError::OriginError("Failed to read directory entry".to_string()))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(TEMP_PREFIX) {
                continue;
            }
            let child_id = Self::child_id(id, &name);
            objects.push(self.get(&child_id).await?);
        }
        Ok(objects)
    }

    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        let path = self.path(id)?;
        self.ensure_contained(id, &path, true)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .open(&path)
            .await
            .map_err(|_| VaultError::NotFound(id.to_string()))?;
        let stream = ReaderStream::new(file).map(|chunk| {
            chunk.map_err(|e| VaultError::OriginError(format!("Failed to read file: {}", e)))
        });
        Ok(Box::pin(stream))
    }

    async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        // The name is the one part of a `put` the caller doesn't get checked by `path`: refuse
        // `../x`, `/abs` and `a/b` before joining, or the object lands outside `destination`.
        validate_name(&object.get_name())?;
        let parent_path = self.path(destination)?;
        let path = parent_path.join(object.get_name());
        let new_id = Self::child_id(destination, &object.get_name());
        self.ensure_contained(destination, &parent_path, true)?;
        self.ensure_contained(&new_id, &path, true)?;
        let _ = object.with_id(new_id);

        match object {
            Object::Branch { .. } => {
                fs::create_dir_all(&path).await.map_err(|e| {
                    VaultError::OriginError(format!("Failed to create directory: {}", e))
                })?;
            }
            Object::Leaf { .. } => {
                // Create the file if missing, but never truncate an existing one: contents are
                // replaced atomically by `send`, so an overwrite must keep serving the old
                // bytes until the new ones are renamed into place.
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&path)
                    .await
                    .map_err(|e| {
                        VaultError::OriginError(format!("Failed to create file: {}", e))
                    })?;
            }
            Object::Root { .. } => Err(VaultError::OriginError(
                "Cannot put root object".to_string(),
            ))?,
        }
        Ok(object.clone())
    }

    async fn send(&self, object: &Object, mut payload: ByteStream) -> VaultResult<()> {
        use tokio::io::AsyncWriteExt;
        let id = object.get_id();
        let path = self.path(&id)?;
        self.ensure_contained(&id, &path, false)?;
        let tmp_path = temp_sibling(&path)?;

        // Write into a sibling temp file and only `rename` it over the destination once every
        // byte is durably on disk. `rename` within one directory is atomic, so readers see
        // either the old contents or the complete new contents — never a truncated or partial
        // file, even if the stream errors or the process dies mid-transfer.
        let write = async {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)
                .await
                .map_err(|e| {
                    VaultError::OriginError(format!("Failed to open temp file for writing: {}", e))
                })?;

            // Keep the destination's permissions when overwriting an existing file.
            if let Ok(existing) = fs::metadata(&path).await {
                let _ = file.set_permissions(existing.permissions()).await;
            }

            while let Some(chunk) = payload.next().await {
                let bytes = chunk?;
                file.write_all(&bytes).await.map_err(|e| {
                    VaultError::OriginError(format!("Failed to write to file: {}", e))
                })?;
            }

            // `tokio::fs::File` buffers internally; flush it, then fsync so the data is on disk
            // before the rename makes it visible.
            file.flush()
                .await
                .map_err(|e| VaultError::OriginError(format!("Failed to flush file: {}", e)))?;
            let file = file.into_std().await;
            // A copy takes its source's modification time (see `Origin::send`), which is what
            // lets the next push/pull recognise it as identical instead of re-sending it.
            if let Some(modified) = object.get_meta().and_then(|meta| meta.modified) {
                file.set_modified(modified.into()).map_err(|e| {
                    VaultError::OriginError(format!("Failed to set modification time: {}", e))
                })?;
            }
            file.sync_all()
                .map_err(|e| VaultError::OriginError(format!("Failed to sync file: {}", e)))?;
            drop(file);

            fs::rename(&tmp_path, &path).await.map_err(|e| {
                VaultError::OriginError(format!("Failed to move file into place: {}", e))
            })?;
            Ok::<(), VaultError>(())
        };

        if let Err(e) = write.await {
            let _ = fs::remove_file(&tmp_path).await;
            return Err(e);
        }

        // Persist the rename itself (the directory entry).
        tokio::task::spawn_blocking(move || sync_parent_dir(&path))
            .await
            .map_err(|e| VaultError::OriginError(format!("Failed to sync directory: {e}")))?;

        Ok(())
    }

    async fn rename(
        &self,
        id: &ObjectId,
        new_parent: &ObjectId,
        new_name: &str,
    ) -> VaultResult<Object> {
        validate_name(new_name)?;
        let from = self.path(id)?;
        let to = self.path(new_parent)?.join(new_name);
        self.ensure_contained(id, &from, false)?;
        self.ensure_contained(new_parent, &to, false)?;
        if from == self.root {
            return Err(VaultError::OriginError(
                "Cannot rename the vault root".into(),
            ));
        }
        if to.starts_with(&from) {
            return Err(VaultError::OriginError(format!(
                "Cannot move '{id}' into itself"
            )));
        }
        let source = fs::symlink_metadata(&from)
            .await
            .map_err(|_| VaultError::NotFound(id.to_string()))?;
        if let Ok(existing) = fs::symlink_metadata(&to).await
            && (existing.is_dir() || source.is_dir())
        {
            return Err(VaultError::OriginError(format!(
                "'{}' already exists",
                to.display()
            )));
        }

        // One `rename(2)`: atomic, and instant however large the tree is.
        match fs::rename(&from, &to).await {
            Ok(()) => {}
            // `root` spans more than one filesystem (a mount inside the vault): there is no
            // atomic move, so let the caller fall back to copy-then-delete.
            Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
                return Err(VaultError::Unsupported(format!(
                    "rename across filesystems: {e}"
                )));
            }
            Err(e) => {
                return Err(VaultError::OriginError(format!("Failed to rename: {e}")));
            }
        }
        tokio::task::spawn_blocking(move || {
            sync_parent_dir(&to);
            sync_parent_dir(&from);
        })
        .await
        .map_err(|e| VaultError::OriginError(format!("Failed to sync directory: {e}")))?;

        self.get(&Self::child_id(new_parent, new_name)).await
    }

    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        let path = self.path(id)?;
        self.ensure_contained(id, &path, false)?;

        let meta = fs::metadata(&path)
            .await
            .map_err(|_| VaultError::NotFound(id.to_string()))?;

        match meta.is_dir() {
            true => fs::remove_dir_all(&path)
                .await
                .map_err(|e| VaultError::OriginError(format!("Failed to remove directory: {}", e))),
            false => fs::remove_file(&path)
                .await
                .map_err(|e| VaultError::OriginError(format!("Failed to remove file: {}", e))),
        }
    }
}

#[cfg(test)]
#[path = "tests/fs.rs"]
mod tests;
