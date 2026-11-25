use super::*;
use std::path::PathBuf;

fn path(name: &str) -> PathBuf {
    PathBuf::from("/tmp/nimbus-tui-test").join(name)
}

#[test]
fn shebang_scripts_are_executable_everywhere() {
    assert!(is_executable(b"#!/bin/sh\necho hi\n", &path("run")));
    assert!(is_executable(b"#!/usr/bin/env python3\n", &path("tool.py")));
}

#[test]
fn documents_are_not_executable() {
    assert!(!is_executable(b"# notes\n\nsome text\n", &path("notes.md")));
    assert!(!is_executable(b"{\"a\": 1}", &path("data.json")));
    assert!(!is_executable(b"", &path("empty")));
    // A `#` comment is not a `#!` line, however close it looks.
    assert!(!is_executable(b"# not a shebang\n", &path("config.toml")));
}

#[test]
#[cfg(target_os = "linux")]
fn elf_binaries_are_executable_on_linux() {
    assert!(is_executable(b"\x7FELF\x02\x01\x01\x00", &path("nimbus")));
}

#[test]
#[cfg(target_os = "linux")]
fn foreign_binaries_are_left_to_the_opener() {
    // Nothing here can run a PE or a Mach-O, so they're files to open, not programs to launch.
    assert!(!is_executable(
        b"MZ\x90\x00\x03\x00\x00\x00",
        &path("tool.exe")
    ));
    assert!(!is_executable(
        b"\xCF\xFA\xED\xFE\x0C\x00\x00\x01",
        &path("tool")
    ));
}

#[test]
#[cfg(unix)]
fn run_executable_makes_the_temp_copy_runnable_first() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("nimbus-opener-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("script.sh");
    // Written the way a fetched object is: contents only, no executable bit.
    std::fs::write(&script, b"#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();

    let status = run_executable(&script).expect("should have started");
    assert_eq!(status.code(), Some(3), "the script's own exit code is kept");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn run_executable_reports_a_program_it_could_not_start() {
    let missing = std::env::temp_dir().join("nimbus-opener-test-does-not-exist");
    assert!(run_executable(&missing).is_err());
}

// --- regression: temp copies live in a private directory ---

#[cfg(unix)]
#[test]
fn scratch_dir_is_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let dir = private_scratch_dir().unwrap();
    let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn scratch_dirs_are_unique_and_removed_on_drop() {
    let a = private_scratch_dir().unwrap();
    let b = private_scratch_dir().unwrap();
    assert_ne!(a.path(), b.path());
    assert!(a.path().starts_with(std::env::temp_dir()));
    let kept = a.path().to_path_buf();
    std::fs::write(kept.join("copy.txt"), b"x").unwrap();
    drop(a);
    assert!(!kept.exists());
}

// --- regression: each public function keeps its own doc comment ---

/// The `///` lines directly above `pub fn {name}` in this module's source.
fn doc_above(source: &str, name: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with(&format!("pub fn {name}(")))
        .unwrap_or_else(|| panic!("pub fn {name} not found"));
    let mut doc: Vec<String> = lines[..at]
        .iter()
        .rev()
        .take_while(|l| l.starts_with("///"))
        .map(|l| l.trim_start_matches('/').trim().to_string())
        .collect();
    doc.reverse();
    doc
}

#[test]
fn every_public_function_has_its_own_doc_comment() {
    // `private_scratch_dir` was once inserted between `run_executable` and its doc comment, so
    // `run_executable` had none and the scratch-dir docs opened with the wrong summary.
    let source = include_str!("../opener.rs");
    let summaries = [
        (
            "is_executable",
            "Whether the fetched bytes look like a program",
        ),
        (
            "private_scratch_dir",
            "Creates the directory that temp copies",
        ),
        (
            "run_executable",
            "Runs the temp copy at `path` as a program",
        ),
        ("wait_for_key", "Holds the terminal after a program has run"),
        (
            "try_os_open",
            "Tries the OS's default file-association opener",
        ),
    ];
    for (name, summary) in summaries {
        let doc = doc_above(source, name);
        assert!(
            doc.first().is_some_and(|first| first.starts_with(summary)),
            "{name}'s doc comment should start with {summary:?}, got {:?}",
            doc.first()
        );
    }
}
