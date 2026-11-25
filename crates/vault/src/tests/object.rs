use super::*;

#[test]
fn object_id_from_string_and_str() {
    let a = ObjectId::from(String::from("abc"));
    let b = ObjectId::from("abc");
    assert_eq!(a.as_str(), "abc");
    assert_eq!(b.as_str(), "abc");
    assert_eq!(a.as_ref(), "abc");
    assert_eq!(format!("{a}"), "abc");
    let s: String = a.into();
    assert_eq!(s, "abc");
}

#[test]
fn object_id_default_and_is_root() {
    let default_id = ObjectId::default();
    assert_eq!(default_id.as_str(), crate::ROOT_ID);
    assert!(default_id.is_root());
    assert!(!ObjectId::from("child").is_root());
}

#[test]
fn metadata_builder_sets_fields() {
    let mut meta = Metadata::new();
    meta.set_size(42)
        .set_content_type("text/plain".to_string())
        .add_extra("k".to_string(), "v".to_string());

    assert_eq!(meta.size, Some(42));
    assert_eq!(meta.content_type.as_deref(), Some("text/plain"));
    assert_eq!(meta.extra.get("k").map(String::as_str), Some("v"));
    assert!(meta.modified.is_none());
}

#[test]
fn metadata_default_is_empty() {
    let meta = Metadata::default();
    assert!(meta.size.is_none());
    assert!(meta.content_type.is_none());
    assert!(meta.modified.is_none());
    assert!(meta.extra.is_empty());
}

#[test]
fn root_object_has_slash_id_and_no_children() {
    let root = Object::root();
    assert_eq!(root.get_id().as_str(), crate::ROOT_ID);
    assert_eq!(root.get_name(), crate::ROOT_NAME);
    assert!(root.get_meta().is_none());
}

#[test]
fn leaf_and_branch_accessors() {
    let leaf = Object::leaf("file".to_string(), ObjectId::from("f1"), Metadata::new());
    assert_eq!(leaf.get_id().as_str(), "f1");
    assert_eq!(leaf.get_name(), "file");
    assert!(leaf.get_meta().is_some());

    let branch = Object::branch(
        "dir".to_string(),
        ObjectId::from("d1"),
        Metadata::new(),
        vec![ObjectId::from("f1")],
    );
    assert_eq!(branch.get_id().as_str(), "d1");
    assert_eq!(branch.get_name(), "dir");
    match branch {
        Object::Branch { children, .. } => {
            assert_eq!(children.unwrap().len(), 1);
        }
        _ => panic!("expected branch"),
    }
}

#[test]
fn push_leaf_onto_root_appends_child() {
    let mut root = Object::root();
    root.push(
        ObjectType::Leaf,
        "file".to_string(),
        ObjectId::from("f1"),
        Metadata::new(),
        None,
    )
    .unwrap();

    match &root {
        Object::Root { children, .. } => {
            let children = children.as_ref().unwrap();
            assert_eq!(children.len(), 1);
            assert_eq!(children[0].as_str(), "f1");
        }
        _ => panic!("expected root"),
    }
}

#[test]
fn push_multiple_children_appends_in_order() {
    let mut branch = Object::branch(
        "dir".to_string(),
        ObjectId::from("d1"),
        Metadata::new(),
        vec![],
    );
    branch
        .push(
            ObjectType::Leaf,
            "a".to_string(),
            ObjectId::from("a"),
            Metadata::new(),
            None,
        )
        .unwrap();
    branch
        .push(
            ObjectType::Branch,
            "b".to_string(),
            ObjectId::from("b"),
            Metadata::new(),
            Some(vec![ObjectId::from("nested")]),
        )
        .unwrap();

    match branch {
        Object::Branch { children, .. } => {
            let children = children.unwrap();
            assert_eq!(children.len(), 2);
            assert_eq!(children[0].as_str(), "a");
            assert_eq!(children[1].as_str(), "b");
        }
        _ => panic!("expected branch"),
    }
}

#[test]
fn push_onto_leaf_is_invalid() {
    let mut leaf = Object::leaf("file".to_string(), ObjectId::from("f1"), Metadata::new());
    let result = leaf.push(
        ObjectType::Leaf,
        "child".to_string(),
        ObjectId::from("c1"),
        Metadata::new(),
        None,
    );
    assert!(matches!(result, Err(VaultError::InvalidMethodCall)));
}

#[test]
fn changed_is_false_for_identical_metadata() {
    let a = Object::leaf("file".to_string(), ObjectId::from("f1"), Metadata::new());
    let b = Object::leaf("file".to_string(), ObjectId::from("f1"), Metadata::new());
    assert!(!a.changed(&b));
}

#[test]
fn changed_is_true_for_diverging_metadata() {
    let mut remote_meta = Metadata::new();
    remote_meta.set_size(42);
    let a = Object::leaf("file".to_string(), ObjectId::from("f1"), Metadata::new());
    let b = Object::leaf("file".to_string(), ObjectId::from("f1"), remote_meta);
    assert!(a.changed(&b));
}

#[test]
fn changed_is_false_when_either_side_has_no_metadata() {
    let root = Object::root();
    let leaf = Object::leaf("file".to_string(), ObjectId::from("f1"), Metadata::new());
    assert!(!root.changed(&leaf));
    assert!(!leaf.changed(&root));
}

#[test]
fn pushing_root_object_type_is_invalid() {
    let mut root = Object::root();
    let result = root.push(
        ObjectType::Root,
        "x".to_string(),
        ObjectId::from("x"),
        Metadata::new(),
        None,
    );
    assert!(matches!(result, Err(VaultError::InvalidMethodCall)));
}

#[test]
fn validate_name_accepts_ordinary_names() {
    for name in [
        "notes.txt",
        "a#b",
        "sp ace",
        "50%",
        "ñandú",
        ".hidden",
        "...",
        "a..b",
    ] {
        assert!(validate_name(name).is_ok(), "{name:?} should be accepted");
    }
}

#[test]
fn validate_name_rejects_names_that_are_not_one_path_component() {
    for name in ["", ".", "..", "../x", "a/b", "/etc/passwd", "a\\b", "a\0b"] {
        assert!(
            matches!(validate_name(name), Err(VaultError::InvalidName(_))),
            "{name:?} should be rejected"
        );
    }
}

fn leaf_with(size: u64, modified: Option<i64>) -> Object {
    let mut meta = Metadata::new();
    meta.set_size(size);
    if let Some(secs) = modified {
        meta.set_modified(chrono::DateTime::from_timestamp(secs, 0).unwrap());
    }
    Object::Leaf {
        name: "a".to_string(),
        id: ObjectId::from("a"),
        meta,
    }
}

#[test]
fn compare_knows_which_side_is_newer() {
    assert_eq!(
        leaf_with(1, Some(100)).compare(&leaf_with(1, Some(100))),
        SyncDecision::Same
    );
    assert_eq!(
        leaf_with(1, Some(200)).compare(&leaf_with(9, Some(100))),
        SyncDecision::SourceNewer
    );
    assert_eq!(
        leaf_with(1, Some(100)).compare(&leaf_with(9, Some(200))),
        SyncDecision::DestNewer
    );
    // Not enough information: copy rather than silently skipping (old `changed()` skipped).
    assert_eq!(
        leaf_with(1, None).compare(&leaf_with(2, None)),
        SyncDecision::Unknown
    );
    assert_eq!(
        leaf_with(1, Some(100)).compare(&leaf_with(2, Some(100))),
        SyncDecision::Unknown
    );
}

#[test]
fn compare_treats_directories_as_same_and_mixed_kinds_as_mismatch() {
    let dir = Object::Branch {
        name: "d".to_string(),
        id: ObjectId::from("d"),
        meta: Metadata::new(),
        children: None,
    };
    assert_eq!(dir.compare(&dir.clone()), SyncDecision::Same);
    assert_eq!(dir.compare(&Object::root()), SyncDecision::Same);
    assert_eq!(dir.compare(&leaf_with(1, None)), SyncDecision::KindMismatch);
    assert_eq!(leaf_with(1, None).compare(&dir), SyncDecision::KindMismatch);
}

#[test]
fn set_meta_replaces_metadata_and_ignores_root() {
    let mut object = leaf_with(1, None);
    let mut meta = Metadata::new();
    meta.set_size(42);
    object.set_meta(meta);
    assert_eq!(object.get_meta().unwrap().size, Some(42));

    let mut root = Object::root();
    root.set_meta(Metadata::new());
    assert!(root.get_meta().is_none());
}
