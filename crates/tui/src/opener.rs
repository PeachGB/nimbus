use std::{
    io::Write,
    path::Path,
    process::{Command, ExitStatus},
};

/// Whether the fetched bytes look like a program to run rather than a document to open.
///
/// Sniffs the content, because a vault stores bytes and names — not permission bits — so the temp
/// copy never arrives with an executable mode to test, and plenty of executables have no
/// extension at all. Only formats this platform can actually execute count: a PE on Linux or an
/// ELF on Windows is just a file you happen to be holding, and the OS opener is a better answer
/// for it than a guaranteed `Exec format error`. (Gating Mach-O on macOS also sidesteps the Java
/// class file, which shares the fat-binary `CAFEBABE` magic.)
pub fn is_executable(bytes: &[u8], path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);

    // A shebang line is the one marker that means "run me" regardless of what follows it.
    if bytes.starts_with(b"#!") {
        return true;
    }
    if cfg!(target_os = "windows") {
        // On Windows it's the extension that decides, not the content: `.bat`/`.cmd` are plain
        // text, and `MZ` alone also covers DLLs and other things that aren't entry points.
        return match extension.as_deref() {
            Some("bat" | "cmd") => true,
            Some("exe" | "com") => bytes.starts_with(b"MZ"),
            _ => false,
        };
    }
    if cfg!(target_os = "macos") {
        const MACH_O: [&[u8; 4]; 6] = [
            b"\xFE\xED\xFA\xCE", // 32-bit, big-endian
            b"\xCE\xFA\xED\xFE", // 32-bit, little-endian
            b"\xFE\xED\xFA\xCF", // 64-bit, big-endian
            b"\xCF\xFA\xED\xFE", // 64-bit, little-endian
            b"\xCA\xFE\xBA\xBE", // universal ("fat") binary
            b"\xBE\xBA\xFE\xCA",
        ];
        return MACH_O.iter().any(|magic| bytes.starts_with(*magic));
    }
    bytes.starts_with(b"\x7FELF")
}

/// Runs the temp copy at `path` as a program, attached to our stdio, and waits for it to exit.
///
/// The caller is expected to have released the terminal first (raw mode, alt screen, its own
/// input-reading thread), same as for [`try_os_open`] and [`editor_command`] — a program run this
/// way owns the tty until it's done. The child inherits our working directory, so whatever it
/// writes relative to the cwd lands where the user started the TUI rather than buried next to the
/// temp copy. An `Err` means it couldn't be started at all, which is the caller's cue to fall
/// back to opening the file instead.
pub fn run_executable(path: &Path) -> std::io::Result<ExitStatus> {
    #[cfg(unix)]
    {
        // The bytes came from a vault, which doesn't carry a mode, so the temp copy is written
        // 0o644 and would fail with EACCES however valid its contents are.
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        std::fs::set_permissions(path, permissions)?;
    }

    let batch = matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("bat" | "cmd")
    );
    let mut cmd = if cfg!(target_os = "windows") && batch {
        // `CreateProcess` can't run a batch file directly; the shell has to interpret it.
        let mut c = Command::new("cmd");
        c.arg("/C").arg(path);
        c
    } else {
        Command::new(path)
    };
    cmd.status()
}

/// Holds the terminal after a program has run, until a key is pressed.
///
/// Reclaiming the alt screen wipes everything the program printed, so without this pause anything
/// short-lived would flash past on its way back to the TUI. Enables raw mode only for the read,
/// so a single keypress is enough and the terminal is left as it was found (not in the alt
/// screen — the caller's `ratatui::init()` takes it back from here).
pub fn wait_for_key() {
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\n[press any key to return to nimbus]");
    let _ = stdout.flush();

    let raw = crossterm::terminal::enable_raw_mode().is_ok();
    while let Ok(event) = crossterm::event::read() {
        if let crossterm::event::Event::Key(key) = event
            && key.kind == crossterm::event::KeyEventKind::Press
        {
            break;
        }
    }
    if raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

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

#[cfg(test)]
#[path = "tests/opener.rs"]
mod tests;
