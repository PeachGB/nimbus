//! Small filesystem helpers shared by [`crate::origin::fs::OriginFileSystem`] and by the crates
//! that persist their own files (session state, vault and daemon configs).

use std::{
    io::Write,
    path::{Path, PathBuf},
};

/// Prefix of the in-flight temp files written next to a destination before they are renamed
/// over it. [`crate::origin::fs::OriginFileSystem::list`] hides entries with this prefix.
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

#[cfg(test)]
#[path = "tests/fsutil.rs"]
mod tests;
