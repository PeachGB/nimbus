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
