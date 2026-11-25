//! Small filesystem helpers shared by [`crate::origin::fs::OriginFileSystem`] and by the crates
//! that persist their own files (session state, vault and daemon configs).

use std::{
    io::Write,
    path::{Path, PathBuf},
};

/// Prefix of the in-flight temp files written next to a destination before they are renamed
/// over it. [`OriginFileSystem`](crate::origin::fs::OriginFileSystem)'s
/// [`Origin::list`](crate::origin::Origin::list) hides entries with this prefix.
pub const TEMP_PREFIX: &str = ".nimbus-tmp.";

/// A unique, hidden path next to `path` — same directory, so a later `rename` onto `path` stays
/// on one filesystem and is atomic.
pub fn temp_sibling(path: &Path) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let invalid = |what: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} has no {what}", path.display()),
        )
    };
    let parent = path.parent().ok_or_else(|| invalid("parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| invalid("file name"))?
        .to_string_lossy();
    Ok(parent.join(format!(
        "{TEMP_PREFIX}{name}.{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )))
}

/// Replaces `path` with `bytes` atomically: a reader (or a crash) sees either the old file or
/// the complete new one, never a truncated mix. Writes a sibling temp file, fsyncs it, renames
/// it over `path`, then fsyncs the directory so the rename itself survives a crash.
///
/// `mode` (Unix only) sets the new file's permissions — e.g. `Some(0o600)` for anything holding
/// a secret. It is applied before the data is written, so the file is never briefly readable
/// with looser permissions. With `None`, an existing file keeps its permissions and a new one
/// gets the process default. Missing parent directories are created.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir()?;
/// let path = dir.path().join("state.toml");
/// nimbus_vault::fsutil::atomic_write(&path, b"cwd = \"/\"\n", Some(0o600))?;
/// assert_eq!(std::fs::read(&path)?, b"cwd = \"/\"\n");
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn atomic_write(path: &Path, bytes: &[u8], mode: Option<u32>) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_sibling(path)?;

    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        let mut file = options.open(&tmp)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // `mode` above is still filtered by the umask; set it exactly. Without an explicit
            // mode, carry over the permissions of the file being replaced.
            let permissions = match mode {
                Some(mode) => Some(std::fs::Permissions::from_mode(mode)),
                None => std::fs::metadata(path).ok().map(|m| m.permissions()),
            };
            if let Some(permissions) = permissions {
                file.set_permissions(permissions)?;
            }
        }
        #[cfg(not(unix))]
        let _ = mode;

        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();

    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    sync_parent_dir(path);
    Ok(())
}

/// Best-effort fsync of `path`'s parent directory, which is what makes a completed `rename`
/// durable. Not every platform allows opening a directory, hence no error.
pub fn sync_parent_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(dir) = std::fs::File::open(if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        })
    {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// The directory a config file's relative paths resolve against: the parent of `config_path`
/// made absolute against the cwd ([`std::path::absolute`], so no symlink resolution and the
/// file needn't exist). Being absolute is what makes the result independent of a later cwd.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use nimbus_vault::fsutil::config_dir;
///
/// assert_eq!(config_dir(Path::new("/etc/nimbus/vault.toml"))?, Path::new("/etc/nimbus"));
/// assert_eq!(config_dir(Path::new("vault.toml"))?, std::env::current_dir()?);
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn config_dir(config_path: &Path) -> std::io::Result<PathBuf> {
    let absolute = std::path::absolute(config_path)?;
    Ok(absolute.parent().map(Path::to_path_buf).unwrap_or(absolute))
}

/// `path` if it is absolute, else `dir.join(path)`. No normalization, no filesystem access.
///
/// # Examples
///
/// ```
/// use std::path::{Path, PathBuf};
/// use nimbus_vault::fsutil::resolve_against;
///
/// let dir = Path::new("/etc/nimbus");
/// assert_eq!(resolve_against(dir, "data".into()), PathBuf::from("/etc/nimbus/data"));
/// assert_eq!(resolve_against(dir, "/srv/data".into()), PathBuf::from("/srv/data"));
/// ```
pub fn resolve_against(dir: &Path, path: PathBuf) -> PathBuf {
    match path.is_absolute() {
        true => path,
        false => dir.join(path),
    }
}

/// Reads a secret kept alone in a file, such as a bearer token. The contents are used
/// verbatim, except that trailing `\r` and `\n` are dropped: an editor or `echo` adds a newline
/// that isn't part of the secret. Leading and inner whitespace stay. `path` is used as given,
/// with no `~` expansion. A file that is empty after trimming is an `InvalidData` error, so an
/// empty file can't quietly mean "no secret". So is a file that isn't UTF-8.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir()?;
/// let path = dir.path().join("token");
/// std::fs::write(&path, "s3cr3t\n")?;
/// assert_eq!(nimbus_vault::fsutil::read_secret(&path)?, "s3cr3t");
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn read_secret(path: &Path) -> std::io::Result<String> {
    let raw = std::fs::read_to_string(path)?;
    let secret = raw.trim_end_matches(['\r', '\n']);
    if secret.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the file is empty",
        ));
    }
    Ok(secret.to_string())
}

#[cfg(test)]
#[path = "tests/fsutil.rs"]
mod tests;
