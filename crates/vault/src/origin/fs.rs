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
/// let file = Object::Leaf {
///     name: "notes.txt".to_string(),
///     id: ObjectId::from("notes.txt"),
///     meta: Metadata::new(),
/// };
/// origin.put(&file).await?; // creates the file
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
    fn path(&self, id: &ObjectId) -> PathBuf {
        self.root.join(id.to_string())
    }
}

#[async_trait::async_trait]
impl Origin for OriginFileSystem {
    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        let path = self.path(id);
        let meta = fs::metadata(&path)
            .await
            .map_err(|_| VaultError::NotFound(id.to_string()))?;
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
            let child_id = ObjectId::from(entry.file_name().to_string_lossy().into_owned());
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

    async fn put(&self, object: &Object) -> VaultResult<()> {
        let path = self.path(&object.get_id());

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
        Ok(())
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
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn origin_at(root: PathBuf) -> OriginFileSystem {
        OriginFileSystem { root }
    }

    #[tokio::test]
    async fn get_returns_not_found_for_missing_path() {
        let dir = tempdir().unwrap();
        let origin = origin_at(dir.path().to_path_buf());
        let result = origin.get(&ObjectId::from("missing")).await;
        assert!(matches!(result, Err(VaultError::NotFound(_))));
    }

    #[tokio::test]
    async fn get_returns_leaf_for_file() {
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("file.txt"), b"content")
            .await
            .unwrap();
        let origin = origin_at(dir.path().to_path_buf());

        let object = origin.get(&ObjectId::from("file.txt")).await.unwrap();
        match object {
            Object::Leaf { name, id, meta } => {
                assert_eq!(name, "file.txt");
                assert_eq!(id.as_str(), "file.txt");
                assert_eq!(meta.size, Some(7));
            }
            _ => panic!("expected leaf"),
        }
    }

    #[tokio::test]
    async fn get_returns_branch_for_directory() {
        let dir = tempdir().unwrap();
        tokio::fs::create_dir(dir.path().join("sub")).await.unwrap();
        let origin = origin_at(dir.path().to_path_buf());

        let object = origin.get(&ObjectId::from("sub")).await.unwrap();
        assert!(matches!(object, Object::Branch { .. }));
    }

    #[tokio::test]
    async fn list_returns_all_directory_entries() {
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), b"a")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.txt"), b"bb")
            .await
            .unwrap();
        let origin = origin_at(dir.path().to_path_buf());

        let mut objects = origin.list(&ObjectId::from("")).await.unwrap();
        objects.sort_by_key(|o| o.get_name());

        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].get_name(), "a.txt");
        assert_eq!(objects[1].get_name(), "b.txt");
    }

    #[tokio::test]
    async fn list_returns_not_found_for_missing_directory() {
        let dir = tempdir().unwrap();
        let origin = origin_at(dir.path().to_path_buf());
        let result = origin.list(&ObjectId::from("missing")).await;
        assert!(matches!(result, Err(VaultError::NotFound(_))));
    }

    #[tokio::test]
    async fn fetch_streams_file_contents() {
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("file.txt"), b"hello world")
            .await
            .unwrap();
        let origin = origin_at(dir.path().to_path_buf());

        let mut stream = origin.fetch(&ObjectId::from("file.txt")).await.unwrap();
        let mut collected = Vec::new();
        while let Some(chunk) = stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected, b"hello world");
    }

    #[tokio::test]
    async fn fetch_returns_not_found_for_missing_file() {
        let dir = tempdir().unwrap();
        let origin = origin_at(dir.path().to_path_buf());
        let result = origin.fetch(&ObjectId::from("missing")).await;
        assert!(matches!(result, Err(VaultError::NotFound(_))));
    }

    #[tokio::test]
    async fn put_creates_file_for_leaf() {
        let dir = tempdir().unwrap();
        let origin = origin_at(dir.path().to_path_buf());
        let object = Object::Leaf {
            name: "file.txt".to_string(),
            id: ObjectId::from("file.txt"),
            meta: Metadata::new(),
        };
        origin.put(&object).await.unwrap();
        assert!(dir.path().join("file.txt").is_file());
    }

    #[tokio::test]
    async fn put_creates_directory_for_branch() {
        let dir = tempdir().unwrap();
        let origin = origin_at(dir.path().to_path_buf());
        let object = Object::Branch {
            name: "sub".to_string(),
            id: ObjectId::from("sub"),
            meta: Metadata::new(),
            children: None,
        };
        origin.put(&object).await.unwrap();
        assert!(dir.path().join("sub").is_dir());
    }

    #[tokio::test]
    async fn put_rejects_root_object() {
        let dir = tempdir().unwrap();
        let origin = origin_at(dir.path().to_path_buf());
        let object = Object::root();
        let result = origin.put(&object).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_writes_stream_to_file() {
        use futures::stream;

        let dir = tempdir().unwrap();
        let origin = origin_at(dir.path().to_path_buf());
        let object = Object::Leaf {
            name: "file.txt".to_string(),
            id: ObjectId::from("file.txt"),
            meta: Metadata::new(),
        };
        let payload: ByteStream = Box::pin(stream::iter(vec![
            Ok(bytes::Bytes::from_static(b"hello ")),
            Ok(bytes::Bytes::from_static(b"world")),
        ]));

        origin.send(&object, payload).await.unwrap();

        let contents = tokio::fs::read(dir.path().join("file.txt")).await.unwrap();
        assert_eq!(contents, b"hello world");
    }

    #[tokio::test]
    async fn delete_removes_file() {
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("file.txt"), b"x")
            .await
            .unwrap();
        let origin = origin_at(dir.path().to_path_buf());

        origin.delete(&ObjectId::from("file.txt")).await.unwrap();
        assert!(!dir.path().join("file.txt").exists());
    }

    #[tokio::test]
    async fn delete_removes_directory_recursively() {
        let dir = tempdir().unwrap();
        tokio::fs::create_dir(dir.path().join("sub")).await.unwrap();
        tokio::fs::write(dir.path().join("sub/file.txt"), b"x")
            .await
            .unwrap();
        let origin = origin_at(dir.path().to_path_buf());

        origin.delete(&ObjectId::from("sub")).await.unwrap();
        assert!(!dir.path().join("sub").exists());
    }

    #[tokio::test]
    async fn delete_returns_not_found_for_missing_path() {
        let dir = tempdir().unwrap();
        let origin = origin_at(dir.path().to_path_buf());
        let result = origin.delete(&ObjectId::from("missing")).await;
        assert!(matches!(result, Err(VaultError::NotFound(_))));
    }
}
