use futures::StreamExt;
use std::path::PathBuf;
use tokio::fs::{self, OpenOptions};
use tokio_util::io::ReaderStream;

use crate::{
    VaultResult,
    error::VaultError,
    object::{Metadata, Object, ObjectId},
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

    fn path(&self, id: &ObjectId) -> PathBuf {
        let relative = Self::relative(id);
        if relative.is_empty() {
            self.root.clone()
        } else {
            self.root.join(relative)
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
        let path = self.path(id);
        let meta = match fs::metadata(&path).await {
            Ok(meta) => meta,
            Err(_) => fs::symlink_metadata(&path)
                .await
                .map_err(|_| VaultError::NotFound(id.to_string()))?,
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
        let path = self.path(id);
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
            let child_id = Self::child_id(id, &name);
            objects.push(self.get(&child_id).await?);
        }
        Ok(objects)
    }

    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        let path = self.path(id);
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
        let parent_path = self.path(destination);
        let path = parent_path.join(object.get_name());
        let new_id = Self::child_id(destination, &object.get_name());
        let _ = object.with_id(new_id);

        match object {
            Object::Branch { .. } => {
                fs::create_dir_all(&path).await.map_err(|e| {
                    VaultError::OriginError(format!("Failed to create directory: {}", e))
                })?;
            }
            Object::Leaf { .. } => {
                fs::File::create(&path).await.map_err(|e| {
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
        let path = self.path(&id);

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .await
            .map_err(|e| {
                VaultError::OriginError(format!("Failed to open file for writing: {}", e))
            })?;

        while let Some(chunk) = payload.next().await {
            let bytes = chunk?;
            file.write_all(&bytes)
                .await
                .map_err(|e| VaultError::OriginError(format!("Failed to write to file: {}", e)))?;
        }
        Ok(())
    }

    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        let path = self.path(id);

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
