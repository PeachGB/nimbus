use super::*;
use crate::object::Metadata;
use futures::StreamExt;
use futures::stream;

fn write_vault_config(
    dir: &std::path::Path,
    name: &str,
    root_id: &str,
    fs_root: &std::path::Path,
) -> std::path::PathBuf {
    let path = dir.join("vault.toml");
    std::fs::write(
        &path,
        format!(
            r#"
            name = "{name}"
            root_id = "{root_id}"

            [origin_config]
            type = "fs"
            root = "{}"
            "#,
            fs_root.display()
        ),
    )
    .unwrap();
    path
}

#[tokio::test]
async fn get_list_and_fetch_delegate_to_wrapped_vault() {
    let tmp = tempfile::tempdir().unwrap();
    let fs_root = tmp.path().join("data");
    tokio::fs::create_dir_all(&fs_root).await.unwrap();
    tokio::fs::write(fs_root.join("child.txt"), b"hello")
        .await
        .unwrap();

    let config_path = write_vault_config(tmp.path(), "inner", "", &fs_root);
    let vault = Vault::new(config_path).unwrap();
    let origin = OriginVault::new(Arc::new(vault));

    let root_obj = origin.get(&ObjectId::from("")).await.unwrap();
    assert!(matches!(root_obj, Object::Branch { .. }));

    let children = origin.list(&ObjectId::from("")).await.unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].get_name(), "child.txt");

    let mut stream = origin.fetch(&ObjectId::from("child.txt")).await.unwrap();
    let mut collected = Vec::new();
    while let Some(chunk) = stream.next().await {
        collected.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(collected, b"hello");
}

#[tokio::test]
async fn put_send_and_delete_delegate_to_wrapped_vault() {
    let tmp = tempfile::tempdir().unwrap();
    let fs_root = tmp.path().to_path_buf();
    tokio::fs::create_dir(fs_root.join("root")).await.unwrap();

    let config_path = write_vault_config(tmp.path(), "inner", "root", &fs_root);
    let vault = Vault::new(config_path).unwrap();
    let origin = OriginVault::new(Arc::new(vault));

    let mut leaf = Object::Leaf {
        name: "new.txt".to_string(),
        id: ObjectId::from("root/new.txt"),
        meta: Metadata::new(),
    };
    origin
        .put(&mut leaf, &ObjectId::from("root"))
        .await
        .unwrap();
    assert!(fs_root.join("root/new.txt").is_file());

    let payload: ByteStream = Box::pin(stream::once(async move {
        Ok(bytes::Bytes::from_static(b"data"))
    }));
    origin.send(&leaf, payload).await.unwrap();
    let contents = tokio::fs::read(fs_root.join("root/new.txt")).await.unwrap();
    assert_eq!(contents, b"data");

    origin
        .delete(&ObjectId::from("root/new.txt"))
        .await
        .unwrap();
    assert!(!fs_root.join("root/new.txt").exists());
}

#[tokio::test]
async fn get_propagates_wrapped_vault_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let fs_root = tmp.path().to_path_buf();
    tokio::fs::create_dir(fs_root.join("root")).await.unwrap();

    let config_path = write_vault_config(tmp.path(), "inner", "root", &fs_root);
    let vault = Vault::new(config_path).unwrap();
    let origin = OriginVault::new(Arc::new(vault));

    let result = origin.get(&ObjectId::from("root/missing.txt")).await;
    assert!(matches!(result, Err(crate::error::VaultError::NotFound(_))));
}
