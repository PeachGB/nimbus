use super::*;
use tempfile::tempdir;

fn origin_at(root: PathBuf) -> OriginFileSystem {
    OriginFileSystem { root }
}

#[tokio::test]
async fn get_returns_not_found_for_missing_path() {
    let dir = tempdir().unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let result = origin.get(&ObjectId::from("missing")).await;
    assert!(matches!(result, Err(VaultError::NotFound(_))));
}

#[tokio::test]
async fn get_returns_leaf_for_file() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("file.txt"), b"content")
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    let object = origin.get(&ObjectId::from("file.txt")).await.unwrap();
    match object {
        Object::Leaf { name, id, meta } => {
            assert_eq!(name, "file.txt");
            assert_eq!(id.as_str(), "file.txt");
            assert_eq!(meta.size, Some(7));
        }
        _ => panic!("expected leaf"),
    }
}

#[tokio::test]
#[cfg(unix)]
async fn get_returns_leaf_for_dangling_symlink_instead_of_erroring() {
    let dir = tempdir().unwrap();
    tokio::fs::symlink(dir.path().join("does-not-exist"), dir.path().join("broken"))
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    let object = origin.get(&ObjectId::from("broken")).await.unwrap();
    assert!(matches!(object, Object::Leaf { .. }));
}

#[tokio::test]
#[cfg(unix)]
async fn list_includes_dangling_symlinks_instead_of_erroring() {
    let dir = tempdir().unwrap();
    tokio::fs::symlink(dir.path().join("does-not-exist"), dir.path().join("broken"))
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    let objects = origin.list(&ObjectId::from("/")).await.unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].get_name(), "broken");
}

#[tokio::test]
async fn get_returns_branch_for_directory() {
    let dir = tempdir().unwrap();
    tokio::fs::create_dir(dir.path().join("sub")).await.unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    let object = origin.get(&ObjectId::from("sub")).await.unwrap();
    assert!(matches!(object, Object::Branch { .. }));
}

#[tokio::test]
async fn list_returns_all_directory_entries() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.txt"), b"a")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("b.txt"), b"bb")
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    let mut objects = origin.list(&ObjectId::from("")).await.unwrap();
    objects.sort_by_key(|o| o.get_name());

    assert_eq!(objects.len(), 2);
    assert_eq!(objects[0].get_name(), "a.txt");
    assert_eq!(objects[1].get_name(), "b.txt");
}

#[tokio::test]
async fn list_with_root_id_lists_vault_root_not_filesystem_root() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.txt"), b"a")
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    let objects = origin.list(&ObjectId::from("/")).await.unwrap();

    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].get_name(), "a.txt");
}

#[tokio::test]
async fn list_returns_not_found_for_missing_directory() {
    let dir = tempdir().unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let result = origin.list(&ObjectId::from("missing")).await;
    assert!(matches!(result, Err(VaultError::NotFound(_))));
}

#[tokio::test]
async fn fetch_streams_file_contents() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("file.txt"), b"hello world")
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    let mut stream = origin.fetch(&ObjectId::from("file.txt")).await.unwrap();
    let mut collected = Vec::new();
    while let Some(chunk) = stream.next().await {
        collected.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(collected, b"hello world");
}

#[tokio::test]
async fn fetch_returns_not_found_for_missing_file() {
    let dir = tempdir().unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let result = origin.fetch(&ObjectId::from("missing")).await;
    assert!(matches!(result, Err(VaultError::NotFound(_))));
}

#[tokio::test]
async fn put_creates_file_for_leaf() {
    let dir = tempdir().unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("file.txt"),
        meta: Metadata::new(),
    };
    origin.put(&mut object, &ObjectId::from("")).await.unwrap();
    assert!(dir.path().join("file.txt").is_file());
}

#[tokio::test]
async fn put_creates_directory_for_branch() {
    let dir = tempdir().unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let mut object = Object::Branch {
        name: "sub".to_string(),
        id: ObjectId::from("sub"),
        meta: Metadata::new(),
        children: None,
    };
    origin.put(&mut object, &ObjectId::from("")).await.unwrap();
    assert!(dir.path().join("sub").is_dir());
}

#[tokio::test]
async fn put_rejects_root_object() {
    let dir = tempdir().unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let mut object = Object::root();
    let result = origin.put(&mut object, &ObjectId::from("")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn send_writes_stream_to_file() {
    use futures::stream;

    let dir = tempdir().unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("file.txt"),
        meta: Metadata::new(),
    };
    let payload: ByteStream = Box::pin(stream::iter(vec![
        Ok(bytes::Bytes::from_static(b"hello ")),
        Ok(bytes::Bytes::from_static(b"world")),
    ]));

    origin.send(&object, payload).await.unwrap();

    let contents = tokio::fs::read(dir.path().join("file.txt")).await.unwrap();
    assert_eq!(contents, b"hello world");
}

#[tokio::test]
async fn delete_removes_file() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("file.txt"), b"x")
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    origin.delete(&ObjectId::from("file.txt")).await.unwrap();
    assert!(!dir.path().join("file.txt").exists());
}

#[tokio::test]
async fn delete_removes_directory_recursively() {
    let dir = tempdir().unwrap();
    tokio::fs::create_dir(dir.path().join("sub")).await.unwrap();
    tokio::fs::write(dir.path().join("sub/file.txt"), b"x")
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    origin.delete(&ObjectId::from("sub")).await.unwrap();
    assert!(!dir.path().join("sub").exists());
}

#[tokio::test]
async fn delete_returns_not_found_for_missing_path() {
    let dir = tempdir().unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let result = origin.delete(&ObjectId::from("missing")).await;
    assert!(matches!(result, Err(VaultError::NotFound(_))));
}

#[tokio::test]
async fn send_failure_leaves_existing_file_untouched() {
    use futures::stream;

    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("file.txt"), b"original")
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("file.txt"),
        meta: Metadata::new(),
    };
    let payload: ByteStream = Box::pin(stream::iter(vec![
        Ok(bytes::Bytes::from_static(b"partial ")),
        Err(VaultError::OriginError("boom".to_string())),
    ]));

    assert!(origin.send(&object, payload).await.is_err());

    let contents = tokio::fs::read(dir.path().join("file.txt")).await.unwrap();
    assert_eq!(contents, b"original");
    // No temp file left behind.
    let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
    let mut names = Vec::new();
    while let Some(e) = entries.next_entry().await.unwrap() {
        names.push(e.file_name().to_string_lossy().into_owned());
    }
    assert_eq!(names, vec!["file.txt".to_string()]);
}

#[tokio::test]
async fn send_overwrites_existing_file() {
    use futures::stream;

    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("file.txt"), b"a much longer original")
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("file.txt"),
        meta: Metadata::new(),
    };
    let payload: ByteStream = Box::pin(stream::iter(vec![Ok(bytes::Bytes::from_static(b"new"))]));

    origin.send(&object, payload).await.unwrap();

    let contents = tokio::fs::read(dir.path().join("file.txt")).await.unwrap();
    assert_eq!(contents, b"new");
}

#[tokio::test]
async fn list_hides_in_flight_temp_files() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.txt"), b"x")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join(format!("{TEMP_PREFIX}a.txt.1-0")), b"y")
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    let objects = origin.list(&ObjectId::from("")).await.unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].get_name(), "a.txt");
}

#[tokio::test]
async fn put_does_not_truncate_existing_file() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("file.txt"), b"keep me")
        .await
        .unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("file.txt"),
        meta: Metadata::new(),
    };

    origin.put(&mut object, &ObjectId::from("")).await.unwrap();

    let contents = tokio::fs::read(dir.path().join("file.txt")).await.unwrap();
    assert_eq!(contents, b"keep me");
}

// --- regressions: hostile names and ids can't leave the root ---

#[tokio::test]
async fn put_rejects_names_that_escape_the_destination() {
    let outer = tempdir().unwrap();
    let root = outer.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    let origin = origin_at(root.clone());

    let absolute = outer.path().join("abs").to_string_lossy().into_owned();
    for name in ["../escaped.txt", "a/b", "..", "", absolute.as_str()] {
        for mut object in [
            Object::Leaf {
                name: name.to_string(),
                id: ObjectId::from("x"),
                meta: Metadata::new(),
            },
            Object::Branch {
                name: name.to_string(),
                id: ObjectId::from("x"),
                meta: Metadata::new(),
                children: None,
            },
        ] {
            let result = origin.put(&mut object, &ObjectId::from("")).await;
            assert!(
                matches!(result, Err(VaultError::InvalidName(_))),
                "{name:?}: {result:?}"
            );
        }
    }
    assert!(!outer.path().join("escaped.txt").exists());
    assert!(!outer.path().join("abs").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}

#[tokio::test]
async fn every_operation_rejects_ids_that_escape_the_root() {
    use futures::stream;

    let outer = tempdir().unwrap();
    let root = outer.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(outer.path().join("secret.txt"), b"secret").unwrap();
    let origin = origin_at(root);

    for raw in [
        "../secret.txt",
        "/../secret.txt",
        "a/../../secret.txt",
        "//etc/passwd",
        "a\0b",
    ] {
        let id = ObjectId::from(raw);
        let invalid = |r: &VaultResult<_>| matches!(r, Err(VaultError::InvalidId(_)));

        assert!(invalid(&origin.get(&id).await.map(|_| ())), "get {raw:?}");
        assert!(invalid(&origin.list(&id).await.map(|_| ())), "list {raw:?}");
        assert!(
            invalid(&origin.fetch(&id).await.map(|_| ())),
            "fetch {raw:?}"
        );
        assert!(invalid(&origin.delete(&id).await), "delete {raw:?}");
        let mut object = Object::Leaf {
            name: "x".to_string(),
            id: id.clone(),
            meta: Metadata::new(),
        };
        assert!(
            invalid(&origin.put(&mut object, &id).await.map(|_| ())),
            "put into {raw:?}"
        );
        let payload: ByteStream =
            Box::pin(stream::iter(vec![Ok(bytes::Bytes::from_static(b"pwned"))]));
        assert!(
            invalid(&origin.send(&object, payload).await),
            "send {raw:?}"
        );
    }
    assert_eq!(
        std::fs::read(outer.path().join("secret.txt")).unwrap(),
        b"secret"
    );
}

#[tokio::test]
async fn ids_with_dot_segments_inside_the_root_still_work() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("docs/a.txt"), b"x").unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    for raw in ["docs/a.txt", "/docs/a.txt", "./docs/a.txt", "docs/./a.txt"] {
        assert!(origin.get(&ObjectId::from(raw)).await.is_ok(), "{raw:?}");
    }
}

// --- rename ---

#[tokio::test]
async fn rename_moves_a_file_and_replaces_an_existing_file_atomically() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join("dst")).unwrap();
    std::fs::write(dir.path().join("a.txt"), b"new").unwrap();
    std::fs::write(dir.path().join("dst/b.txt"), b"old").unwrap();
    let origin = origin_at(dir.path().to_path_buf());

    let moved = origin
        .rename(&ObjectId::from("a.txt"), &ObjectId::from("dst"), "b.txt")
        .await
        .unwrap();
    assert_eq!(moved.get_id().as_str(), "dst/b.txt");
    assert!(!dir.path().join("a.txt").exists());
    assert_eq!(std::fs::read(dir.path().join("dst/b.txt")).unwrap(), b"new");
}

#[tokio::test]
async fn rename_refuses_to_replace_or_create_a_directory_over_something() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("d1")).unwrap();
    std::fs::create_dir_all(dir.path().join("d2")).unwrap();
    std::fs::write(dir.path().join("f"), b"x").unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let root = ObjectId::from("");

    // dir over (empty) dir, file over dir, dir over file
    assert!(
        origin
            .rename(&ObjectId::from("d1"), &root, "d2")
            .await
            .is_err()
    );
    assert!(
        origin
            .rename(&ObjectId::from("f"), &root, "d2")
            .await
            .is_err()
    );
    assert!(
        origin
            .rename(&ObjectId::from("d1"), &root, "f")
            .await
            .is_err()
    );
    assert!(dir.path().join("d1").is_dir() && dir.path().join("d2").is_dir());
    assert_eq!(std::fs::read(dir.path().join("f")).unwrap(), b"x");
}

#[tokio::test]
async fn rename_rejects_bad_names_missing_sources_moves_into_itself_and_the_root() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("d/sub")).unwrap();
    let origin = origin_at(dir.path().to_path_buf());
    let root = ObjectId::from("");

    assert!(matches!(
        origin.rename(&ObjectId::from("d"), &root, "../out").await,
        Err(VaultError::InvalidName(_))
    ));
    assert!(matches!(
        origin.rename(&ObjectId::from("missing"), &root, "x").await,
        Err(VaultError::NotFound(_))
    ));
    assert!(
        origin
            .rename(&ObjectId::from("d"), &ObjectId::from("d/sub"), "d")
            .await
            .is_err()
    );
    assert!(origin.rename(&root, &root, "x").await.is_err());
    assert!(dir.path().join("d/sub").is_dir());
}

// --- regressions: symlinks can't lead out of the root ---

#[cfg(unix)]
fn vault_with_escaping_links() -> (tempfile::TempDir, std::path::PathBuf, OriginFileSystem) {
    let outer = tempdir().unwrap();
    let root = outer.path().join("vault");
    let outside = outer.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
    std::fs::create_dir(root.join("inner")).unwrap();
    std::fs::write(root.join("inner/ok.txt"), b"fine").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("secret-link")).unwrap();
    std::os::unix::fs::symlink(outside.join("new.txt"), root.join("dangling")).unwrap();
    std::os::unix::fs::symlink(root.join("inner"), root.join("in-link")).unwrap();
    let origin = origin_at(root);
    (outer, outside, origin)
}

#[cfg(unix)]
#[tokio::test]
async fn operations_through_a_symlink_out_of_the_root_are_refused() {
    use futures::stream;
    let (_outer, outside, origin) = vault_with_escaping_links();
    let invalid = |r: VaultResult<()>| matches!(r, Err(VaultError::InvalidId(_)));
    let id = |raw: &str| ObjectId::from(raw);

    assert!(invalid(origin.fetch(&id("secret-link")).await.map(|_| ())));
    assert!(invalid(
        origin.fetch(&id("out/secret.txt")).await.map(|_| ())
    ));
    assert!(invalid(origin.list(&id("out")).await.map(|_| ())));
    assert!(invalid(origin.get(&id("out/secret.txt")).await.map(|_| ())));
    assert!(invalid(origin.delete(&id("out/secret.txt")).await));
    let payload: ByteStream = Box::pin(stream::iter(vec![Ok(bytes::Bytes::from_static(b"x"))]));
    let target = Object::Leaf {
        name: "secret.txt".to_string(),
        id: id("out/secret.txt"),
        meta: Metadata::new(),
    };
    assert!(invalid(origin.send(&target, payload).await));
    for (dest, name) in [("out", "new.txt"), ("", "dangling")] {
        let mut object = Object::Leaf {
            name: name.to_string(),
            id: id("x"),
            meta: Metadata::new(),
        };
        assert!(
            invalid(origin.put(&mut object, &id(dest)).await.map(|_| ())),
            "put {dest}/{name}"
        );
    }
    assert!(invalid(
        origin
            .rename(&id("out/secret.txt"), &id(""), "got.txt")
            .await
            .map(|_| ())
    ));

    assert_eq!(
        std::fs::read(outside.join("secret.txt")).unwrap(),
        b"secret"
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_that_stay_inside_and_the_links_themselves_still_work() {
    let (_outer, outside, origin) = vault_with_escaping_links();
    // Listing the root still shows every link, escaping and dangling ones included.
    let names: std::collections::HashSet<String> = origin
        .list(&ObjectId::from(""))
        .await
        .unwrap()
        .iter()
        .map(|o| o.get_name())
        .collect();
    for name in ["out", "secret-link", "dangling", "in-link", "inner"] {
        assert!(names.contains(name), "{name} missing from {names:?}");
    }
    // A link pointing inside the vault is followed as before.
    let mut fetched = Vec::new();
    let mut stream = origin
        .fetch(&ObjectId::from("in-link/ok.txt"))
        .await
        .unwrap();
    while let Some(chunk) = stream.next().await {
        fetched.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(fetched, b"fine");
    // Deleting an escaping link removes the link, never what it points at.
    origin.delete(&ObjectId::from("out")).await.unwrap();
    assert_eq!(
        std::fs::read(outside.join("secret.txt")).unwrap(),
        b"secret"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn get_describes_an_escaping_symlink_as_itself_not_its_target() {
    let (_outer, _outside, origin) = vault_with_escaping_links();
    // `out` points at a directory outside the root: it must not look like a directory to walk.
    let out = origin.get(&ObjectId::from("out")).await.unwrap();
    assert!(matches!(out, Object::Leaf { .. }), "{out:?}");
    // `secret-link` points at a 6-byte file outside; its own size is the link's, not 6.
    let link = origin.get(&ObjectId::from("secret-link")).await.unwrap();
    assert_ne!(link.get_meta().unwrap().size, Some(6));
    // A link that stays inside is still described by its target.
    let inside = origin.get(&ObjectId::from("in-link")).await.unwrap();
    assert!(matches!(inside, Object::Branch { .. }), "{inside:?}");
}
