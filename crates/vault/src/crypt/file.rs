//! Key files and whole encrypted files on the local disk: loading a key from where a config
//! says it is, creating a new key file, and decrypting one stored object straight from its file
//! (the recovery path, which needs no vault at all).

use std::path::Path;

use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use zeroize::Zeroizing;

use super::{CryptKey, decrypt};
use crate::{VaultResult, error::VaultError, fsutil};

impl CryptKey {
    /// Reads a key from exactly one of `key_file` (its text form, read with
    /// [`fsutil::read_secret`], so a trailing newline is fine) or the environment variable
    /// `key_env`. The text is held in a wiped buffer and parsed straight into the key; no error
    /// quotes it, and every error names where the key was looked for.
    pub fn load(key_file: Option<&Path>, key_env: Option<&str>) -> VaultResult<CryptKey> {
        let (text, source) = match (key_file, key_env) {
            (Some(path), None) => (
                fsutil::read_secret_zeroizing(path).map_err(|e| {
                    VaultError::Generic(format!("crypt: reading key file {}: {e}", path.display()))
                })?,
                path.display().to_string(),
            ),
            (None, Some(name)) => (
                Zeroizing::new(std::env::var(name).map_err(|_| {
                    VaultError::Generic(format!("crypt: environment variable '{name}' is not set"))
                })?),
                format!("environment variable '{name}'"),
            ),
            (None, None) => {
                return Err(VaultError::Generic(
                    "crypt: set one of key_file or key_env".to_string(),
                ));
            }
            (Some(_), Some(_)) => {
                return Err(VaultError::Generic(
                    "crypt: set only one of key_file or key_env".to_string(),
                ));
            }
        };
        CryptKey::parse(&text).map_err(|e| VaultError::Generic(format!("crypt: {source}: {e}")))
    }
}

/// Generates a new key and writes its text form to a new file at `path`, readable by its owner
/// only (`0600` on Unix; on Windows there are no mode bits and the file inherits its
/// directory's ACL), and returns the key. Refuses to replace an existing file: a key file that
/// gets overwritten takes every object encrypted with it along. On a filesystem with hard
/// links the file appears whole or not at all; elsewhere it is copied into a new file, which
/// never replaces anything but is not atomic (see `PrivateTemp::place`).
///
/// # Examples
///
/// ```
/// use nimbus_vault::crypt::{CryptKey, create_key_file};
///
/// let dir = tempfile::tempdir()?;
/// let path = dir.path().join("vault.key");
/// create_key_file(&path)?;
/// assert!(CryptKey::load(Some(&path), None).is_ok());
/// assert!(create_key_file(&path).is_err()); // never overwritten
/// # Ok::<(), nimbus_vault::error::VaultError>(())
/// ```
pub fn create_key_file(path: &Path) -> VaultResult<CryptKey> {
    let key = CryptKey::generate()?;
    write_key_file(path, &key)?;
    Ok(key)
}

/// Writes `key`'s text form to a new file at `path`, with the same guarantees as
/// [`create_key_file`]: owner-only, never replacing an existing file (an existing `path` is an
/// I/O `AlreadyExists` error), whole or not at all where the filesystem has hard links. For a
/// key generated earlier and shown to the user before it is written, as the vault creator
/// wizard does.
pub fn write_key_file(path: &Path, key: &CryptKey) -> VaultResult<()> {
    write_key_file_with(path, key, |_| {})
}

/// [`write_key_file`], calling `before_place` with the temp file's path once the key is written
/// and before it is moved into place: a seam for tests.
fn write_key_file_with(
    path: &Path,
    key: &CryptKey,
    before_place: impl FnOnce(&Path),
) -> VaultResult<()> {
    let text = key.to_text();
    create_parent(path)?;
    let temp = PrivateTemp::create(path)?;
    write_key(&temp, text.as_bytes())
        .and_then(|()| {
            before_place(&temp.path);
            temp.place(path, false, hard_link)
        })
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => already_exists(path),
            _ => VaultError::Generic(format!("writing key file {}: {e}", path.display())),
        })
}

/// Writes the key text, then the newline on its own: appending it to the text would reallocate
/// the wiped buffer and leave the old copy of the key in freed memory.
fn write_key(temp: &PrivateTemp, text: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = temp.file()?;
    file.write_all(text)?;
    file.write_all(b"\n")?;
    file.sync_all()
}

/// Decrypts the stored object in the file `input` into a new file `output`, with no vault
/// involved: the recovery path for when nimbus, the config or the storage's origin is gone.
/// Returns the plaintext size.
///
/// The plaintext goes to a hidden temp file next to `output`, readable by its owner only
/// (`0600` on Unix; on Windows it inherits the directory's ACL), and is moved into place only
/// once the whole object has decrypted and checked out: a wrong key, a damaged or a truncated
/// object leaves no `output` and no temp file behind. An existing `output` is refused unless
/// `overwrite` is set, and then replaced atomically. With `overwrite`, a detected tampering
/// with the temp file (see `PrivateTemp::place`) leaves no output, not the old one.
pub async fn decrypt_file(
    key: &CryptKey,
    input: &Path,
    output: &Path,
    overwrite: bool,
) -> VaultResult<u64> {
    if !overwrite && std::fs::symlink_metadata(output).is_ok() {
        return Err(already_exists(output));
    }
    let source = tokio::fs::File::open(input)
        .await
        .map_err(|e| VaultError::Generic(format!("opening {}: {e}", input.display())))?;
    let ciphertext = tokio_util::io::ReaderStream::new(source)
        .map(|chunk| chunk.map_err(VaultError::from))
        .boxed();
    create_parent(output)?;
    let temp = PrivateTemp::create(output)?;
    let size = write_stream(&temp, decrypt(key, ciphertext)).await?;
    temp.place(output, overwrite, hard_link)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => already_exists(output),
            _ => VaultError::from(e),
        })?;
    Ok(size)
}

/// An `AlreadyExists` I/O error naming `path`, which a caller can tell apart from a failure.
fn already_exists(path: &Path) -> VaultError {
    VaultError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("{} already exists", path.display()),
    ))
}

/// Creates `path`'s parent directory if it is missing.
fn create_parent(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// Writes `stream` into `temp` and syncs, returning the number of bytes written. The stream's
/// first error stops it.
async fn write_stream(
    temp: &PrivateTemp,
    mut stream: crate::origin::ByteStream,
) -> VaultResult<u64> {
    let mut file = tokio::fs::File::from_std(temp.file()?);
    let mut written = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        written += chunk.len() as u64;
    }
    file.sync_all().await?;
    Ok(written)
}

/// How many fresh temp names [`PrivateTemp::create`] tries before giving up.
const TEMP_ATTEMPTS: usize = 8;

/// A temp file next to a destination, owner-only, that this process created (never one that
/// was already there) and so is the only one allowed to delete. Dropped without being placed,
/// it removes itself.
#[derive(Debug)]
struct PrivateTemp {
    path: std::path::PathBuf,
    /// The handle `create_new` returned. Everything is written and read through it, never by
    /// reopening `path`: someone who can write to the directory could swap the name for a
    /// symlink to a file of theirs in between, and the key or plaintext would land there.
    handle: std::fs::File,
    /// Cleared once the file has been moved into place.
    owned: bool,
}

impl PrivateTemp {
    /// A new temp file next to `destination`. A name that is already taken (a stale temp from a
    /// crashed run that had the same pid, or a planted file) is left alone and the next name is
    /// tried.
    fn create(destination: &Path) -> std::io::Result<Self> {
        Self::create_with(|| fsutil::temp_sibling(destination))
    }

    /// [`PrivateTemp::create`] with the names coming from `next_name`.
    fn create_with(
        mut next_name: impl FnMut() -> std::io::Result<std::path::PathBuf>,
    ) -> std::io::Result<Self> {
        for _ in 0..TEMP_ATTEMPTS {
            let path = next_name()?;
            match private_options().open(&path) {
                Ok(handle) => {
                    return Ok(PrivateTemp {
                        path,
                        handle,
                        owned: true,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other(format!(
            "couldn't create a temp file: {TEMP_ATTEMPTS} names in a row were taken"
        )))
    }

    /// The temp file's own handle (a duplicate of the one `create_new` returned), for writing.
    fn file(&self) -> std::io::Result<std::fs::File> {
        self.handle.try_clone()
    }

    /// Moves the finished temp file to `destination`. With `replace`, a `rename` (atomic,
    /// replacing whatever is there). Without, `link` (a hard link) gives it the new name, which
    /// fails if `destination` exists, so nothing is ever replaced even by a file that appeared
    /// in the meantime; then the temp name is removed.
    ///
    /// Some filesystems have no hard links (FAT and exFAT, as on most USB sticks, and many
    /// SMB, FUSE and Android mounts). There the bytes are copied into a `destination` created
    /// with `create_new` (owner-only, never replacing anything), then the temp is removed. That
    /// still never clobbers, but it isn't atomic: a crash midway leaves a partial
    /// `destination`, which for a key file is an unparseable key that nothing was encrypted
    /// with yet. The directory is synced either way.
    ///
    /// After a link or rename, the destination must be the very file this temp's handle wrote
    /// (same device and inode, a regular file, not a symlink): someone who can write to the
    /// directory could have put their own file at the temp name in between, a key they know or
    /// content of theirs. Then the new destination name is removed and this fails. On non-Unix
    /// platforms there is no inode to compare and the check is skipped. The copy fallback needs
    /// no check: it reads through the handle into a destination it creates itself.
    fn place(
        mut self,
        destination: &Path,
        replace: bool,
        link: impl Fn(&Path, &Path) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        let ours = file_identity(&self.handle.metadata()?);
        if replace {
            std::fs::rename(&self.path, destination)?;
            self.owned = false;
            check_placed(destination, ours)?;
        } else {
            match link(&self.path, destination) {
                Ok(()) => {
                    if let Err(e) = check_placed(destination, ours) {
                        self.owned = false; // the temp name isn't ours any more either
                        return Err(e);
                    }
                }
                Err(e) if links_unsupported(&e) => {
                    use std::io::{Seek, SeekFrom};
                    let mut source = self.file()?;
                    source.seek(SeekFrom::Start(0))?;
                    copy_new(&mut source, destination)?
                }
                Err(e) => return Err(e),
            }
            std::fs::remove_file(&self.path)?;
        }
        self.owned = false;
        fsutil::sync_parent_dir(destination);
        Ok(())
    }
}

impl Drop for PrivateTemp {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The (device, inode) of a file, to tell whether a name still points at the file we wrote.
/// `None` where the platform has no such identity (not Unix), and the check is skipped there.
fn file_identity(metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// Checks that `destination`, just linked or renamed into place, is the very file this call
/// wrote (identity `ours`), not something put at the temp name in between by someone who can
/// write to the directory: a file of theirs (a key they chose) or a symlink. On a mismatch the
/// destination name just created is removed and the placement fails.
fn check_placed(destination: &Path, ours: Option<(u64, u64)>) -> std::io::Result<()> {
    let Some(ours) = ours else {
        return Ok(());
    };
    let placed = std::fs::symlink_metadata(destination)?;
    if placed.file_type().is_file() && file_identity(&placed) == Some(ours) {
        return Ok(());
    }
    let _ = std::fs::remove_file(destination);
    Err(std::io::Error::other(
        "the temp file was replaced while writing; is the directory writable by other users?",
    ))
}

/// `std::fs::hard_link`, as the default `link` for [`PrivateTemp::place`].
fn hard_link(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::hard_link(from, to)
}

/// Whether `error` from a hard link means the filesystem has none (as opposed to, say, the
/// destination existing).
fn links_unsupported(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::Unsupported | std::io::ErrorKind::PermissionDenied
    )
}

/// Copies `source` into a new owner-only file `to`, failing if `to` exists, and syncs it. A
/// copy that fails midway (disk full, an I/O error) removes `to` again, since this call created
/// it: no partial key or plaintext is left behind. Only a crash can leave one.
fn copy_new(source: &mut impl std::io::Read, to: &Path) -> std::io::Result<()> {
    let mut target = private_options().open(to)?;
    let copied = std::io::copy(source, &mut target).and_then(|_| target.sync_all());
    if copied.is_err() {
        drop(target);
        let _ = std::fs::remove_file(to);
    }
    copied
}

/// Options that create a new file, owner-only on Unix.
fn private_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

#[cfg(test)]
#[path = "tests/file.rs"]
mod tests;
