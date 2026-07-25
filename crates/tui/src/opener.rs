use std::{path::Path, process::Command};

/// Tries the OS's default file-association opener for `path` (`open` on macOS, `start` on
/// Windows, `xdg-open` elsewhere). Inherits our real stdio rather than redirecting it — the
/// resolved association isn't necessarily a detached GUI app (a desktop entry can be
/// `Terminal=true`, as most CLI tools' are, in which case the opener execs it attached straight
/// to our controlling terminal); the caller is expected to have already released the terminal
/// (raw mode, alt screen, its own input-reading thread) before calling this, same as it does for
/// the `editor_command` fallback. Returns whether it reported success — `xdg-open` in particular
/// exits non-zero when no application is associated with the file type, which is the caller's
/// cue to fall back to a text editor.
pub fn try_os_open(path: &Path) -> bool {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(path);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]).arg(path);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    matches!(cmd.status(), Ok(status) if status.success())
}

/// Builds the fallback text editor command for files with no (or no working) OS association:
/// `$EDITOR`/`$VISUAL` if set, else a platform-sensible default.
pub fn editor_command() -> Command {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| {
            if cfg!(target_os = "windows") {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        });
    Command::new(editor)
}
