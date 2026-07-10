use std::sync::Arc;

use crate::{
    VaultResult,
    object::{Object, ObjectId},
    origin::{ByteStream, Origin},
    vault::Vault,
};

/// Adapts a `Vault` to the `Origin` trait by forwarding each method to the wrapped vault's own
/// method of the same name. Lets a `Vault` be used anywhere an `&dyn Origin` is expected — most
/// notably as the `remote` argument to another `Vault`'s `push`/`pull` — so vaults can sync
/// directly with one another without either side needing to know the other is a `Vault` rather
/// than a plain origin. Also the type built by `OriginConfig::Vault`, letting a vault config
/// reference another vault's config file as its origin.
///
/// # Examples
///
/// ```
/// use std::{fs, sync::Arc};
/// use nimbus_vault::{origin::vault::OriginVault, vault::Vault};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// // two independent vaults, each backed by its own directory
/// let source_dir = tempfile::tempdir()?;
/// let source_data = source_dir.path().join("data");
/// fs::create_dir_all(&source_data)?;
/// fs::write(source_data.join("notes.txt"), b"hello")?;
/// let source_config = source_dir.path().join("vault.toml");
/// fs::write(&source_config, format!(
///     "name = \"source\"\n\n[origin_config]\ntype = \"fs\"\nroot = \"{}\"\n",
///     source_data.display(),
/// ))?;
///
/// let dest_dir = tempfile::tempdir()?;
/// let dest_data = dest_dir.path().join("data");
/// fs::create_dir_all(&dest_data)?;
/// let dest_config = dest_dir.path().join("vault.toml");
/// fs::write(&dest_config, format!(
///     "name = \"dest\"\n\n[origin_config]\ntype = \"fs\"\nroot = \"{}\"\n",
///     dest_data.display(),
/// ))?;
///
/// let source = Vault::new(source_config)?;
/// let dest = Arc::new(Vault::new(dest_config)?);
/// let dest_as_origin = OriginVault::new(dest); // wrap `dest` so it can act as a remote
///
/// source.push(&"".into(), &dest_as_origin).await?; // sync source's tree into dest
/// assert!(dest_data.join("notes.txt").is_file());
/// # Ok(())
/// # }
/// ```
pub struct OriginVault {
    vault: Arc<Vault>,
}

impl OriginVault {
    /// Wraps `vault` so it can be used as an `Origin`.
    pub fn new(vault: Arc<Vault>) -> Self {
        OriginVault { vault }
    }
}

#[async_trait::async_trait]
impl Origin for OriginVault {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        self.vault.fetch(id).await
    }

    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>> {
        self.vault.list(id).await
    }

    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        self.vault.get(id).await
    }

    async fn put(&self, object: &Object) -> VaultResult<()> {
        self.vault.put(object).await
    }

    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()> {
        self.vault.send(object, payload).await
    }

    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        self.vault.delete(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::Metadata;
    use futures::StreamExt;
    use futures::stream;

    fn write_vault_config(
        dir: &std::path::Path,
        name: &str,
        root_id: &str,
        fs_root: &std::path::Path,
    ) -> std::path::PathBuf {
        let path = dir.join("vault.toml");
        std::fs::write(
            &path,
            format!(
                r#"
                name = "{name}"
                root_id = "{root_id}"

                [origin_config]
                type = "fs"
                root = "{}"
                "#,
                fs_root.display()
            ),
        )
        .unwrap();
        path
    }

    #[tokio::test]
    async fn get_list_and_fetch_delegate_to_wrapped_vault() {
        let tmp = tempfile::tempdir().unwrap();
        let fs_root = tmp.path().join("data");
        tokio::fs::create_dir_all(&fs_root).await.unwrap();
        tokio::fs::write(fs_root.join("child.txt"), b"hello")
            .await
            .unwrap();

        let config_path = write_vault_config(tmp.path(), "inner", "", &fs_root);
        let vault = Vault::new(config_path).unwrap();
        let origin = OriginVault::new(Arc::new(vault));

        let root_obj = origin.get(&ObjectId::from("")).await.unwrap();
        assert!(matches!(root_obj, Object::Branch { .. }));

        let children = origin.list(&ObjectId::from("")).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].get_name(), "child.txt");

        let mut stream = origin.fetch(&ObjectId::from("child.txt")).await.unwrap();
        let mut collected = Vec::new();
        while let Some(chunk) = stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected, b"hello");
    }

    #[tokio::test]
    async fn put_send_and_delete_delegate_to_wrapped_vault() {
        let tmp = tempfile::tempdir().unwrap();
        let fs_root = tmp.path().to_path_buf();
        tokio::fs::create_dir(fs_root.join("root")).await.unwrap();

        let config_path = write_vault_config(tmp.path(), "inner", "root", &fs_root);
        let vault = Vault::new(config_path).unwrap();
        let origin = OriginVault::new(Arc::new(vault));

        let leaf = Object::Leaf {
            name: "new.txt".to_string(),
            id: ObjectId::from("root/new.txt"),
            meta: Metadata::new(),
        };
        origin.put(&leaf).await.unwrap();
        assert!(fs_root.join("root/new.txt").is_file());

        let payload: ByteStream = Box::pin(stream::once(async move {
            Ok(bytes::Bytes::from_static(b"data"))
        }));
        origin.send(&leaf, payload).await.unwrap();
        let contents = tokio::fs::read(fs_root.join("root/new.txt")).await.unwrap();
        assert_eq!(contents, b"data");

        origin
            .delete(&ObjectId::from("root/new.txt"))
            .await
            .unwrap();
        assert!(!fs_root.join("root/new.txt").exists());
    }

    #[tokio::test]
    async fn get_propagates_wrapped_vault_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let fs_root = tmp.path().to_path_buf();
        tokio::fs::create_dir(fs_root.join("root")).await.unwrap();

        let config_path = write_vault_config(tmp.path(), "inner", "root", &fs_root);
        let vault = Vault::new(config_path).unwrap();
        let origin = OriginVault::new(Arc::new(vault));

        let result = origin.get(&ObjectId::from("root/missing.txt")).await;
        assert!(matches!(result, Err(crate::error::VaultError::NotFound(_))));
    }
}
