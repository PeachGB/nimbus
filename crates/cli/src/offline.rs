//! The commands that need no vault, session or config: generating a key, and decrypting a
//! stored object straight from its file. They run before (and without) `App::init`, so a
//! broken config or a missing session can't stand in the way of recovering data.

use anyhow::{Result, anyhow};
use nimbus_vault::{
    crypt::{CryptKey, create_key_file, decrypt_file},
    error::VaultError,
};
use std::path::Path;

/// What `keygen` prints after writing a key: the one thing a user must not get wrong.
pub const KEY_LOSS_WARNING: &str = "\
Anyone with this key can read what it encrypts, and without it no one can, you included:
losing the key loses the data. Back it up now, somewhere other than the vault's storage
(a password manager, say). Keep the file private; nimbus warns if other users can read it.";

/// `keygen <path>`: writes a new key to a new file, owner-only, never overwriting one.
pub fn keygen(path: &Path) -> Result<()> {
    create_key_file(path).map_err(|e| match e {
        VaultError::Io(e) if e.kind() == std::io::ErrorKind::AlreadyExists => anyhow!(
            "{} already exists; keygen never replaces a key file, since that would lose \
             whatever was encrypted with it",
            path.display()
        ),
        e => anyhow!("{e}"),
    })?;
    println!("wrote a new key to {}", path.display());
    eprintln!("{KEY_LOSS_WARNING}");
    Ok(())
}

/// `decrypt <input> <output>`: decrypts one stored object from its file into a new file, with
/// the key from `--key-file` or `--key-env`. Nothing is written unless the whole object checks
/// out; an existing `output` needs `force`.
pub async fn decrypt(
    input: &Path,
    output: &Path,
    key_file: Option<&Path>,
    key_env: Option<&str>,
    force: bool,
) -> Result<()> {
    let key = CryptKey::load(key_file, key_env).map_err(|e| anyhow!("{e}"))?;
    match decrypt_file(&key, input, output, force).await {
        Ok(size) => {
            println!("decrypted {size} bytes into {}", output.display());
            Ok(())
        }
        Err(VaultError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(anyhow!(
            "{} already exists; pass --force to replace it",
            output.display()
        )),
        Err(e) => Err(anyhow!("decrypting {}: {e}", input.display())),
    }
}

#[cfg(test)]
#[path = "tests/offline.rs"]
mod tests;
