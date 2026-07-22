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

    async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        self.vault.put(object, destination).await
    }

    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()> {
        self.vault.send(object, payload).await
    }

    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        self.vault.delete(id).await
    }
}

#[cfg(test)]
#[path = "tests/vault.rs"]
mod tests;
