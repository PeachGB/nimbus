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
