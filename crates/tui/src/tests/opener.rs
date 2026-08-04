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
