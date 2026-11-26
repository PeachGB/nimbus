use super::*;
use nimbus_vault::vault::{SyncConflict, SyncReport};

fn conflict(id: &str, reason: &str, can_force: bool) -> SyncConflict {
    SyncConflict {
        id: ObjectId::from(id),
        reason: reason.to_string(),
        can_force,
    }
}

#[test]
fn sync_status_without_conflicts_is_just_the_counts() {
    let report = SyncReport {
        copied: vec![ObjectId::from("a")],
        unchanged: 4,
        conflicts: vec![],
    };
    assert_eq!(
        sync_status("pushed", &report),
        "pushed: 1 copied, 4 unchanged"
    );
}

#[test]
fn sync_status_names_each_conflict_with_its_own_reason() {
    // Every conflict used to be called "newer at the destination".
    let report = SyncReport {
        copied: vec![],
        unchanged: 0,
        conflicts: vec![
            conflict(
                "thing",
                "a file here, a directory at the destination",
                false,
            ),
            conflict(
                "link",
                "leaves the vault (a symlink pointing outside it); skipped",
                false,
            ),
        ],
    };
    let status = sync_status("pulled", &report);
    assert!(
        status.contains("thing (a file here, a directory at the destination)"),
        "{status}"
    );
    assert!(status.contains("link (leaves the vault"), "{status}");
    assert!(!status.contains("newer at the destination"), "{status}");
    // None of these can be forced, so no --force hint.
    assert!(!status.contains("--force"), "{status}");
}

#[test]
fn sync_status_hints_at_force_only_when_something_can_be_forced() {
    let report = SyncReport {
        copied: vec![],
        unchanged: 0,
        conflicts: vec![
            conflict("notes.txt", "newer at the destination", true),
            conflict(
                "thing",
                "a file here, a directory at the destination",
                false,
            ),
        ],
    };
    assert!(sync_status("pushed", &report).contains("--force"));
}

#[test]
fn startup_warnings_end_up_in_the_status_bar_text() {
    // What `App::default` puts in `status`: the TUI owns the terminal, so nothing it would
    // print could be seen.
    assert_eq!(with_warnings(None, vec![]), None);
    assert_eq!(
        with_warnings(None, vec!["WARNING: a".into(), "NOTE: b".into()]),
        Some("WARNING: a  ·  NOTE: b".to_string())
    );
    assert_eq!(
        with_warnings(Some("vault registered".into()), vec!["WARNING: key".into()]),
        Some("vault registered  ·  WARNING: key".to_string())
    );
}

#[test]
fn a_creator_wizard_that_kept_a_key_says_so_in_the_status_bar() {
    let kept = CreatorOutcome {
        config: None,
        unsaved_key_file: Some(PathBuf::from("/keys/v.key")),
    };
    assert_eq!(
        unsaved_key_status(&kept),
        Some(nimbus_creator::kept_key_notice(std::path::Path::new(
            "/keys/v.key"
        )))
    );

    let cancelled = CreatorOutcome {
        config: None,
        unsaved_key_file: None,
    };
    assert_eq!(unsaved_key_status(&cancelled), None);
}
