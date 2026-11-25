use super::*;
use crate::error::VaultError;
use crate::object::Metadata;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream;
use std::sync::Mutex;

struct MockOrigin {
    get_calls: Mutex<Vec<ObjectId>>,
}

impl MockOrigin {
    fn new() -> Self {
        MockOrigin {
            get_calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl Origin for MockOrigin {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        if id.as_str() == "missing" {
            return Err(VaultError::NotFound(id.to_string()));
        }
        let bytes = Bytes::from_static(b"hello");
        Ok(Box::pin(stream::once(async move { Ok(bytes) })))
    }
    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>> {
        Ok(vec![Object::Leaf {
            name: "child".to_string(),
            id: ObjectId::from(format!("{}/child", id.as_str())),
            meta: Metadata::new(),
        }])
    }
    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        self.get_calls.lock().unwrap().push(id.clone());
        Ok(Object::Leaf {
            name: id.to_string(),
            id: id.clone(),
            meta: Metadata::new(),
        })
    }
    async fn put(&self, object: &mut Object, _destination: &ObjectId) -> VaultResult<Object> {
        Ok(object.clone())
    }
    async fn send(&self, _object: &Object, _payload: ByteStream) -> VaultResult<()> {
        Ok(())
    }
    async fn delete(&self, _id: &ObjectId) -> VaultResult<()> {
        Ok(())
    }
}

/// An `Origin` backed by an in-memory tree, used to exercise `Vault::push`/`Vault::pull`
/// against realistic parent/child relationships (unlike `MockOrigin`, whose `list` always
/// fabricates a single flat child).
struct MapOrigin {
    objects: Mutex<HashMap<ObjectId, Object>>,
    children: Mutex<HashMap<ObjectId, Vec<ObjectId>>>,
    payloads: Mutex<HashMap<ObjectId, Bytes>>,
    put_calls: Mutex<Vec<ObjectId>>,
    send_calls: Mutex<Vec<ObjectId>>,
}

impl MapOrigin {
    fn empty() -> Self {
        MapOrigin {
            objects: Mutex::new(HashMap::new()),
            children: Mutex::new(HashMap::new()),
            payloads: Mutex::new(HashMap::new()),
            put_calls: Mutex::new(Vec::new()),
            send_calls: Mutex::new(Vec::new()),
        }
    }
    fn insert(&self, parent: &ObjectId, object: Object) {
        let id = object.get_id();
        self.objects.lock().unwrap().insert(id.clone(), object);
        self.children
            .lock()
            .unwrap()
            .entry(parent.clone())
            .or_default()
            .push(id);
    }
    fn set_payload(&self, id: &ObjectId, bytes: &'static [u8]) {
        self.payloads
            .lock()
            .unwrap()
            .insert(id.clone(), Bytes::from_static(bytes));
    }
}

#[async_trait::async_trait]
impl Origin for MapOrigin {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        let bytes = self
            .payloads
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_else(|| Bytes::from_static(b"default"));
        Ok(Box::pin(stream::once(async move { Ok(bytes) })))
    }
    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>> {
        let ids = self
            .children
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_default();
        let objects = self.objects.lock().unwrap();
        Ok(ids
            .iter()
            .map(|child_id| objects.get(child_id).unwrap().clone())
            .collect())
    }
    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        self.objects
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| VaultError::NotFound(id.to_string()))
    }
    async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        let id = object.get_id();
        self.put_calls.lock().unwrap().push(id.clone());
        self.objects
            .lock()
            .unwrap()
            .insert(id.clone(), object.clone());
        self.children
            .lock()
            .unwrap()
            .entry(destination.clone())
            .or_default()
            .push(id);
        Ok(object.clone())
    }
    async fn send(&self, object: &Object, mut payload: ByteStream) -> VaultResult<()> {
        self.send_calls.lock().unwrap().push(object.get_id());
        while payload.next().await.is_some() {}
        Ok(())
    }
    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        self.objects.lock().unwrap().remove(id);
        Ok(())
    }
}

/// An `Origin` whose `get` always fails with a non-`NotFound` error, used to verify that
/// `push`/`pull` propagate unexpected errors instead of treating them as "needs sync".
struct FailingGetOrigin;

#[async_trait::async_trait]
impl Origin for FailingGetOrigin {
    async fn fetch(&self, _id: &ObjectId) -> VaultResult<ByteStream> {
        Err(VaultError::Generic("fetch should not be called".into()))
    }
    async fn list(&self, _id: &ObjectId) -> VaultResult<Vec<Object>> {
        Ok(vec![])
    }
    async fn get(&self, _id: &ObjectId) -> VaultResult<Object> {
        Err(VaultError::Generic("boom".into()))
    }
    async fn put(&self, _object: &mut Object, _destination: &ObjectId) -> VaultResult<Object> {
        Err(VaultError::Generic("put should not be called".into()))
    }
    async fn send(&self, _object: &Object, _payload: ByteStream) -> VaultResult<()> {
        Err(VaultError::Generic("send should not be called".into()))
    }
    async fn delete(&self, _id: &ObjectId) -> VaultResult<()> {
        Ok(())
    }
}

/// An `Origin` whose `put` mirrors `OriginHTTP`/`OriginCommand`: it does *not* mutate the
/// `&mut Object` it's given in place, instead returning a freshly-built `Object` renamed to
/// `"{destination}/{name}"`. Used to verify that callers rely on `put`'s return value
/// rather than assuming the input object was renamed in place (which only `OriginFileSystem`
/// currently does). Optionally fails instead, to verify failure doesn't leave stale state.
struct RenamingOrigin {
    fail: bool,
    put_calls: Mutex<Vec<ObjectId>>,
    send_calls: Mutex<Vec<ObjectId>>,
}

impl RenamingOrigin {
    fn new() -> Self {
        RenamingOrigin {
            fail: false,
            put_calls: Mutex::new(Vec::new()),
            send_calls: Mutex::new(Vec::new()),
        }
    }
    fn failing() -> Self {
        RenamingOrigin {
            fail: true,
            put_calls: Mutex::new(Vec::new()),
            send_calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl Origin for RenamingOrigin {
    async fn fetch(&self, _id: &ObjectId) -> VaultResult<ByteStream> {
        Ok(Box::pin(stream::once(async {
            Ok(Bytes::from_static(b"payload"))
        })))
    }
    async fn list(&self, _id: &ObjectId) -> VaultResult<Vec<Object>> {
        Ok(vec![])
    }
    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        Err(VaultError::NotFound(id.to_string()))
    }
    async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        self.put_calls.lock().unwrap().push(object.get_id());
        if self.fail {
            return Err(VaultError::Generic("put failed".into()));
        }
        let new_id = ObjectId::from(format!("{}/{}", destination.as_str(), object.get_name()));
        let mut renamed = object.clone();
        renamed.with_id(new_id);
        Ok(renamed)
    }
    async fn send(&self, object: &Object, mut payload: ByteStream) -> VaultResult<()> {
        self.send_calls.lock().unwrap().push(object.get_id());
        while payload.next().await.is_some() {}
        Ok(())
    }
    async fn delete(&self, _id: &ObjectId) -> VaultResult<()> {
        Ok(())
    }
}

fn make_vault() -> Vault {
    Vault::from_origin(
        "test-vault".to_string(),
        ObjectId::from("root"),
        Arc::new(MockOrigin::new()),
    )
}

fn make_vault_with_origin() -> (Vault, Arc<MockOrigin>) {
    let origin = Arc::new(MockOrigin::new());
    let vault = Vault::from_origin(
        "test-vault".to_string(),
        ObjectId::from("root"),
        origin.clone(),
    );
    (vault, origin)
}

#[test]
fn new_sets_name_and_root() {
    let vault = make_vault();
    assert_eq!(vault.get_name(), "test-vault");
    assert_eq!(vault.root.as_str(), "root");
}

#[tokio::test]
async fn find_returns_root_id_for_empty_path() {
    let vault = make_vault();
    let id = vault.find(PathBuf::from("")).await.unwrap();
    assert_eq!(id.as_str(), "root");
}

#[tokio::test]
async fn find_resolves_single_path_component() {
    let vault = make_vault();
    let id = vault.find(PathBuf::from("child")).await.unwrap();
    assert_eq!(id.as_str(), "root/child");
}

#[tokio::test]
async fn find_resolves_absolute_path_skipping_root_component() {
    let vault = make_vault();
    let id = vault.find(PathBuf::from("/child")).await.unwrap();
    assert_eq!(id.as_str(), "root/child");
}

#[tokio::test]
async fn find_returns_not_found_for_unknown_component() {
    let vault = make_vault();
    let result = vault.find(PathBuf::from("missing")).await;
    assert!(matches!(result, Err(VaultError::NotFound(_))));
}

#[tokio::test]
async fn get_delegates_to_origin() {
    let vault = make_vault();
    let obj = vault.get("some-id").await.unwrap();
    assert_eq!(obj.get_id().as_str(), "some-id");
}

#[tokio::test]
async fn list_delegates_to_origin() {
    let vault = make_vault();
    let objects = vault.list("dir").await.unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].get_id().as_str(), "dir/child");
}

#[tokio::test]
async fn get_caches_result_and_skips_origin_on_second_call() {
    let (vault, origin) = make_vault_with_origin();
    vault.get("some-id").await.unwrap();
    vault.get("some-id").await.unwrap();
    assert_eq!(origin.get_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn list_caches_children_so_get_skips_origin() {
    let (vault, origin) = make_vault_with_origin();
    vault.list("root").await.unwrap();
    let obj = vault.get("root/child").await.unwrap();
    assert_eq!(obj.get_name(), "child");
    assert!(origin.get_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn put_caches_object_so_get_skips_origin() {
    let (vault, origin) = make_vault_with_origin();
    let mut object = Object::Leaf {
        name: "file".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    vault
        .put(&mut object, &ObjectId::from("root"))
        .await
        .unwrap();
    let cached = vault.get("f1").await.unwrap();
    assert_eq!(cached.get_name(), "file");
    assert!(origin.get_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn delete_evicts_object_from_cache() {
    let (vault, origin) = make_vault_with_origin();
    let mut object = Object::Leaf {
        name: "file".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    vault
        .put(&mut object, &ObjectId::from("root"))
        .await
        .unwrap();
    vault.delete(&ObjectId::from("f1")).await.unwrap();
    vault.get("f1").await.unwrap();
    assert_eq!(origin.get_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn put_caches_under_the_returned_id_not_the_input_id() {
    // Some origins (OriginHTTP, OriginCommand) don't rename the input object in place —
    // they report the real id via `put`'s return value instead. `Vault::put` must cache
    // (and return) that value, not the caller's stale input id.
    let vault = Vault::from_origin(
        "v".to_string(),
        ObjectId::from("root"),
        Arc::new(RenamingOrigin::new()),
    );
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };

    let returned = vault
        .put(&mut object, &ObjectId::from("dir"))
        .await
        .unwrap();
    assert_eq!(returned.get_id().as_str(), "dir/file.txt");

    // Cached under the returned id, servable without hitting the origin again.
    let cached = vault.get("dir/file.txt").await.unwrap();
    assert_eq!(cached.get_id().as_str(), "dir/file.txt");
}

#[tokio::test]
async fn put_does_not_cache_when_origin_put_fails() {
    let vault = Vault::from_origin(
        "v".to_string(),
        ObjectId::from("root"),
        Arc::new(RenamingOrigin::failing()),
    );
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };

    let result = vault.put(&mut object, &ObjectId::from("dir")).await;
    assert!(result.is_err());

    // Neither the input id nor the would-be renamed id should have been cached.
    let by_input_id = vault.get("f1").await;
    assert!(matches!(by_input_id, Err(VaultError::NotFound(_))));
    let by_renamed_id = vault.get("dir/file.txt").await;
    assert!(matches!(by_renamed_id, Err(VaultError::NotFound(_))));
}

#[tokio::test]
async fn pull_sends_leaf_payload_to_the_id_put_actually_returned() {
    // RenamingOrigin as `self`'s origin doesn't mutate `remote_obj` in place, so `pull`
    // must use `put`'s return value (not the stale `remote_obj`) as the `send` target.
    let renaming = Arc::new(RenamingOrigin::new());
    let vault = Vault::from_origin(
        "local".to_string(),
        ObjectId::from("root"),
        renaming.clone(),
    );

    let remote = MapOrigin::empty();
    remote.insert(&ObjectId::from("root"), leaf("f1", "f1"));
    remote.set_payload(&ObjectId::from("f1"), b"remote-data");

    vault.pull(&ObjectId::from("root"), &remote).await.unwrap();

    assert_eq!(
        renaming.send_calls.lock().unwrap().as_slice(),
        &[ObjectId::from("root/f1")]
    );
}

#[tokio::test]
async fn push_sends_leaf_payload_to_the_id_put_actually_returned() {
    let local_origin = Arc::new(MapOrigin::empty());
    local_origin.insert(&ObjectId::from("root"), leaf("f1", "f1"));
    local_origin.set_payload(&ObjectId::from("f1"), b"local-data");
    let vault = Vault::from_origin("local".to_string(), ObjectId::from("root"), local_origin);

    let remote = RenamingOrigin::new();
    vault.push(&ObjectId::from("root"), &remote).await.unwrap();

    assert_eq!(
        remote.put_calls.lock().unwrap().as_slice(),
        &[ObjectId::from("f1")]
    );
    assert_eq!(
        remote.send_calls.lock().unwrap().as_slice(),
        &[ObjectId::from("root/f1")]
    );
}

#[tokio::test]
async fn fetch_delegates_to_origin() {
    let vault = make_vault();
    let mut stream = vault.fetch("some-id").await.unwrap();
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(&chunk[..], b"hello");
}

#[tokio::test]
async fn fetch_propagates_origin_error() {
    let vault = make_vault();
    let result = vault.fetch("missing").await;
    assert!(matches!(result, Err(VaultError::NotFound(_))));
}

#[test]
fn get_origin_returns_same_origin() {
    let vault = make_vault();
    let origin = vault.get_origin();
    assert_eq!(Arc::strong_count(&origin), 2);
}

#[test]
fn new_builds_vault_from_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("vault.toml");
    let root = dir.path().join("data");
    std::fs::write(
        &config_path,
        format!(
            r#"
            name = "config-vault"

            [origin_config]
            type = "fs"
            root = "{}"
            "#,
            root.display()
        ),
    )
    .unwrap();

    let vault = Vault::new(config_path).unwrap();
    assert_eq!(vault.get_name(), "config-vault");
    assert_eq!(vault.root.as_str(), ObjectId::default().as_str());
}

#[test]
fn new_honors_explicit_root_id_from_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("vault.toml");
    let root = dir.path().join("data");
    std::fs::write(
        &config_path,
        format!(
            r#"
            name = "config-vault"
            root_id = "custom-root"

            [origin_config]
            type = "fs"
            root = "{}"
            "#,
            root.display()
        ),
    )
    .unwrap();

    let vault = Vault::new(config_path).unwrap();
    assert_eq!(vault.root.as_str(), "custom-root");
}

#[test]
fn new_propagates_error_for_missing_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let result = Vault::new(dir.path().join("missing.toml"));
    assert!(matches!(result, Err(VaultError::Io(_))));
}

fn leaf(name: &str, id: &str) -> Object {
    Object::Leaf {
        name: name.to_string(),
        id: ObjectId::from(id),
        meta: Metadata::new(),
    }
}

fn branch(name: &str, id: &str) -> Object {
    Object::Branch {
        name: name.to_string(),
        id: ObjectId::from(id),
        meta: Metadata::new(),
        children: None,
    }
}

#[tokio::test]
async fn pull_copies_missing_objects_and_recurses_into_branches() {
    let root = ObjectId::from("root");
    let local_origin = Arc::new(MapOrigin::empty());
    let vault = Vault::from_origin("local".to_string(), root.clone(), local_origin.clone());

    let remote = MapOrigin::empty();
    remote.insert(&root, branch("d1", "d1"));
    remote.insert(&ObjectId::from("d1"), leaf("f1", "f1"));
    remote.set_payload(&ObjectId::from("f1"), b"remote-data");

    vault.pull(&root, &remote).await.unwrap();

    assert_eq!(
        local_origin.put_calls.lock().unwrap().as_slice(),
        &[ObjectId::from("d1"), ObjectId::from("f1")]
    );
    assert_eq!(
        local_origin.send_calls.lock().unwrap().as_slice(),
        &[ObjectId::from("f1")]
    );
    assert!(
        local_origin
            .objects
            .lock()
            .unwrap()
            .contains_key(&ObjectId::from("f1"))
    );
}

#[tokio::test]
async fn pull_skips_objects_with_unchanged_metadata() {
    let root = ObjectId::from("root");
    let local_origin = Arc::new(MapOrigin::empty());
    local_origin.insert(&root, leaf("f1", "f1"));
    let vault = Vault::from_origin("local".to_string(), root.clone(), local_origin.clone());

    let remote = MapOrigin::empty();
    remote.insert(&root, leaf("f1", "f1"));

    vault.pull(&root, &remote).await.unwrap();

    assert!(local_origin.put_calls.lock().unwrap().is_empty());
    assert!(local_origin.send_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pull_propagates_unexpected_error_from_local_get() {
    let root = ObjectId::from("root");
    let vault = Vault::from_origin(
        "local".to_string(),
        root.clone(),
        Arc::new(FailingGetOrigin),
    );

    let remote = MapOrigin::empty();
    remote.insert(&root, leaf("f1", "f1"));

    let result = vault.pull(&root, &remote).await;
    assert!(matches!(result, Err(VaultError::Generic(_))));
}

#[tokio::test]
async fn push_copies_missing_objects_and_recurses_into_branches() {
    let root = ObjectId::from("root");
    let local_origin = Arc::new(MapOrigin::empty());
    local_origin.insert(&root, branch("d1", "d1"));
    local_origin.insert(&ObjectId::from("d1"), leaf("f1", "f1"));
    local_origin.set_payload(&ObjectId::from("f1"), b"local-data");
    let vault = Vault::from_origin("local".to_string(), root.clone(), local_origin);

    let remote = MapOrigin::empty();
    vault.push(&root, &remote).await.unwrap();

    assert_eq!(
        remote.put_calls.lock().unwrap().as_slice(),
        &[ObjectId::from("d1"), ObjectId::from("f1")]
    );
    assert_eq!(
        remote.send_calls.lock().unwrap().as_slice(),
        &[ObjectId::from("f1")]
    );
    assert!(
        remote
            .objects
            .lock()
            .unwrap()
            .contains_key(&ObjectId::from("f1"))
    );
}

#[tokio::test]
async fn push_skips_objects_with_unchanged_metadata() {
    let root = ObjectId::from("root");
    let local_origin = Arc::new(MapOrigin::empty());
    local_origin.insert(&root, leaf("f1", "f1"));
    let vault = Vault::from_origin("local".to_string(), root.clone(), local_origin);

    let remote = MapOrigin::empty();
    remote.insert(&root, leaf("f1", "f1"));

    vault.push(&root, &remote).await.unwrap();

    assert!(remote.put_calls.lock().unwrap().is_empty());
    assert!(remote.send_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn push_propagates_unexpected_error_from_remote_get() {
    let root = ObjectId::from("root");
    let local_origin = Arc::new(MapOrigin::empty());
    local_origin.insert(&root, leaf("f1", "f1"));
    let vault = Vault::from_origin("local".to_string(), root.clone(), local_origin);

    let result = vault.push(&root, &FailingGetOrigin).await;
    assert!(matches!(result, Err(VaultError::Generic(_))));
}

#[tokio::test]
async fn push_syncs_into_another_vault_wrapped_as_an_origin_via_origin_vault() {
    use crate::origin::vault::OriginVault;

    let tmp = tempfile::tempdir().unwrap();
    let source_root = tmp.path().join("source");
    let dest_root = tmp.path().join("dest");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&dest_root).unwrap();
    std::fs::write(source_root.join("file.txt"), b"payload").unwrap();

    let source_origin = crate::origin::fs::OriginFileSystem::new(source_root);
    let source_vault = Vault::from_origin(
        "source".to_string(),
        ObjectId::from(""),
        Arc::new(source_origin),
    );

    let dest_origin = crate::origin::fs::OriginFileSystem::new(dest_root.clone());
    let dest_vault = Arc::new(Vault::from_origin(
        "dest".to_string(),
        ObjectId::from(""),
        Arc::new(dest_origin),
    ));
    let dest_as_origin = OriginVault::new(dest_vault);

    source_vault
        .push(&ObjectId::from(""), &dest_as_origin)
        .await
        .unwrap();

    let contents = std::fs::read(dest_root.join("file.txt")).unwrap();
    assert_eq!(contents, b"payload");
}

// --- regressions: hostile names from a remote, stale cache ---

fn fs_backed_vault(root: std::path::PathBuf) -> Vault {
    Vault::from_parts(
        "local".to_string(),
        Arc::new(crate::origin::fs::OriginFileSystem::new(root)),
        ObjectId::from(""),
    )
    .unwrap()
}

async fn send_bytes(vault: &Vault, object: &Object, bytes: &'static [u8]) {
    let payload: ByteStream = Box::pin(stream::once(async move { Ok(Bytes::from_static(bytes)) }));
    vault.send(object, payload).await.unwrap();
}

#[tokio::test]
async fn pull_refuses_a_remote_name_that_escapes_the_destination() {
    // A remote listing `../victim.txt` used to make `pull` overwrite a file outside the vault.
    let outer = tempfile::tempdir().unwrap();
    let vault_root = outer.path().join("vault");
    std::fs::create_dir(&vault_root).unwrap();
    std::fs::write(outer.path().join("victim.txt"), b"untouched").unwrap();
    let local = fs_backed_vault(vault_root.clone());

    let remote = MapOrigin::empty();
    let root = ObjectId::from("");
    let evil = Object::Leaf {
        name: "../victim.txt".to_string(),
        id: ObjectId::from("evil"),
        meta: Metadata::new(),
    };
    remote.insert(&root, evil);
    remote.set_payload(&ObjectId::from("evil"), b"pwned");

    let result = local.pull(&root, &remote).await;
    assert!(
        matches!(result, Err(VaultError::InvalidName(_))),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read(outer.path().join("victim.txt")).unwrap(),
        b"untouched"
    );
    assert_eq!(std::fs::read_dir(&vault_root).unwrap().count(), 0);
}

#[tokio::test]
async fn send_evicts_the_cached_object_so_get_sees_new_metadata() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"").unwrap();
    let vault = fs_backed_vault(dir.path().to_path_buf());

    let before = vault.get("a.txt").await.unwrap();
    assert_eq!(before.get_meta().unwrap().size, Some(0));
    send_bytes(&vault, &before, b"hello").await;

    let after = vault.get("a.txt").await.unwrap();
    assert_eq!(after.get_meta().unwrap().size, Some(5));
}

#[tokio::test]
async fn delete_evicts_descendants_from_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs/deep")).unwrap();
    std::fs::write(dir.path().join("docs/deep/a.txt"), b"x").unwrap();
    std::fs::write(dir.path().join("docs2"), b"sibling, not a child").unwrap();
    let vault = fs_backed_vault(dir.path().to_path_buf());

    vault.list("").await.unwrap();
    vault.list("docs").await.unwrap();
    vault.list("docs/deep").await.unwrap();
    vault.delete(&ObjectId::from("docs")).await.unwrap();

    assert!(matches!(
        vault.get("docs/deep/a.txt").await,
        Err(VaultError::NotFound(_))
    ));
    assert!(matches!(
        vault.get("docs/deep").await,
        Err(VaultError::NotFound(_))
    ));
    // A sibling that merely shares the prefix stays cached and reachable.
    assert!(vault.get("docs2").await.is_ok());
}

#[tokio::test]
async fn get_fresh_bypasses_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"1").unwrap();
    let vault = fs_backed_vault(dir.path().to_path_buf());

    vault.get("a.txt").await.unwrap();
    std::fs::write(dir.path().join("a.txt"), b"changed outside").unwrap();

    assert_eq!(
        vault.get("a.txt").await.unwrap().get_meta().unwrap().size,
        Some(1)
    );
    let fresh = vault.get_fresh("a.txt").await.unwrap();
    assert_eq!(fresh.get_meta().unwrap().size, Some(15));
    // ...and refreshes the cache for later plain `get`s.
    assert_eq!(
        vault.get("a.txt").await.unwrap().get_meta().unwrap().size,
        Some(15)
    );
}

#[test]
fn is_same_or_descendant_matches_whole_segments_only() {
    let id = ObjectId::from("docs");
    for yes in ["docs", "/docs", "docs/a", "/docs/a/b"] {
        assert!(
            Vault::is_same_or_descendant(&ObjectId::from(yes), &id),
            "{yes}"
        );
    }
    for no in ["docs2", "doc", "other/docs"] {
        assert!(
            !Vault::is_same_or_descendant(&ObjectId::from(no), &id),
            "{no}"
        );
    }
}

// --- regressions: push/pull only copy forward, and are idempotent ---

fn set_mtime(path: &std::path::Path, secs_ago: u64) {
    let time = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn mtime(path: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

fn fs_origin(root: &std::path::Path) -> crate::origin::fs::OriginFileSystem {
    crate::origin::fs::OriginFileSystem::new(root.to_path_buf())
}

#[tokio::test]
async fn push_twice_transfers_nothing_the_second_time() {
    let local_dir = tempfile::tempdir().unwrap();
    let remote_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(local_dir.path().join("docs")).unwrap();
    std::fs::write(local_dir.path().join("docs/a.txt"), b"a").unwrap();
    std::fs::write(local_dir.path().join("b.txt"), b"bb").unwrap();
    let local = fs_backed_vault(local_dir.path().to_path_buf());
    let remote = fs_origin(remote_dir.path());
    let root = ObjectId::from("");

    let first = local.push(&root, &remote).await.unwrap();
    assert_eq!(first.copied.len(), 3, "{first:?}"); // docs/, docs/a.txt, b.txt
    assert!(first.conflicts.is_empty());
    // The copies carry the source's modification time...
    for file in ["docs/a.txt", "b.txt"] {
        assert_eq!(
            mtime(&local_dir.path().join(file)),
            mtime(&remote_dir.path().join(file)),
            "{file}"
        );
    }
    // ...so a second push finds everything identical.
    let second = local.push(&root, &remote).await.unwrap();
    assert!(second.copied.is_empty(), "{second:?}");
    assert_eq!(second.unchanged, 2);
}

#[tokio::test]
async fn push_does_not_overwrite_a_newer_remote_file_unless_forced() {
    let local_dir = tempfile::tempdir().unwrap();
    let remote_dir = tempfile::tempdir().unwrap();
    std::fs::write(local_dir.path().join("notes.txt"), b"old local").unwrap();
    std::fs::write(remote_dir.path().join("notes.txt"), b"newer remote edit").unwrap();
    set_mtime(&local_dir.path().join("notes.txt"), 3600);
    let local = fs_backed_vault(local_dir.path().to_path_buf());
    let remote = fs_origin(remote_dir.path());
    let root = ObjectId::from("");

    let report = local.push(&root, &remote).await.unwrap();
    assert!(report.copied.is_empty());
    assert_eq!(
        report.conflicts,
        vec![SyncConflict {
            id: ObjectId::from("notes.txt"),
            reason: "newer at the destination".to_string(),
            can_force: true,
        }]
    );
    assert_eq!(
        std::fs::read(remote_dir.path().join("notes.txt")).unwrap(),
        b"newer remote edit"
    );

    let forced = local
        .push_with(&root, &remote, SyncOptions { force: true })
        .await
        .unwrap();
    assert_eq!(forced.copied, vec![ObjectId::from("notes.txt")]);
    assert_eq!(
        std::fs::read(remote_dir.path().join("notes.txt")).unwrap(),
        b"old local"
    );
}

#[tokio::test]
async fn pull_copies_a_newer_remote_file_and_keeps_a_newer_local_one() {
    let local_dir = tempfile::tempdir().unwrap();
    let remote_dir = tempfile::tempdir().unwrap();
    // `fresh.txt` changed on the remote; `edited.txt` was changed locally since.
    std::fs::write(local_dir.path().join("fresh.txt"), b"stale").unwrap();
    std::fs::write(remote_dir.path().join("fresh.txt"), b"fresh from remote").unwrap();
    set_mtime(&local_dir.path().join("fresh.txt"), 3600);
    std::fs::write(remote_dir.path().join("edited.txt"), b"remote copy").unwrap();
    std::fs::write(local_dir.path().join("edited.txt"), b"local edit").unwrap();
    set_mtime(&remote_dir.path().join("edited.txt"), 3600);
    let local = fs_backed_vault(local_dir.path().to_path_buf());
    let remote = fs_origin(remote_dir.path());

    let report = local.pull(&ObjectId::from(""), &remote).await.unwrap();
    assert_eq!(report.copied, vec![ObjectId::from("fresh.txt")]);
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].id, ObjectId::from("edited.txt"));
    assert_eq!(
        std::fs::read(local_dir.path().join("fresh.txt")).unwrap(),
        b"fresh from remote"
    );
    assert_eq!(
        std::fs::read(local_dir.path().join("edited.txt")).unwrap(),
        b"local edit"
    );
}

#[tokio::test]
async fn a_file_never_replaces_a_directory_even_when_forced() {
    let local_dir = tempfile::tempdir().unwrap();
    let remote_dir = tempfile::tempdir().unwrap();
    std::fs::write(local_dir.path().join("thing"), b"a file").unwrap();
    std::fs::create_dir(remote_dir.path().join("thing")).unwrap();
    std::fs::write(remote_dir.path().join("thing/inside.txt"), b"keep").unwrap();
    let local = fs_backed_vault(local_dir.path().to_path_buf());
    let remote = fs_origin(remote_dir.path());

    let report = local
        .push_with(&ObjectId::from(""), &remote, SyncOptions { force: true })
        .await
        .unwrap();
    assert_eq!(report.conflicts.len(), 1, "{report:?}");
    assert_eq!(
        std::fs::read(remote_dir.path().join("thing/inside.txt")).unwrap(),
        b"keep"
    );
}

#[test]
fn sync_report_summarises_counts_and_lists_conflicts() {
    let report = SyncReport {
        copied: vec![ObjectId::from("a"), ObjectId::from("b")],
        unchanged: 5,
        conflicts: vec![SyncConflict {
            id: ObjectId::from("notes.txt"),
            reason: "newer at the destination".to_string(),
            can_force: true,
        }],
    };
    assert_eq!(
        report.to_string(),
        "copied 2, unchanged 5, 1 conflict\n  notes.txt: newer at the destination"
    );
    assert_eq!(
        SyncReport::default().to_string(),
        "copied 0, unchanged 0, 0 conflicts"
    );
}

// --- regressions: a symlink leaving the vault is skipped, never aborts the sync ---

/// A vault root next to an `outside` directory, holding `a.txt`, `z.txt`, a link to the
/// outside directory and a link to a file in it. Returns (tempdir, vault root, outside dir).
#[cfg(unix)]
fn vault_with_links_out() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("vault");
    let outside = outer.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    std::fs::write(root.join("z.txt"), b"z").unwrap();
    std::os::unix::fs::symlink("../outside", root.join("link")).unwrap();
    std::os::unix::fs::symlink("../outside/secret.txt", root.join("file-link")).unwrap();
    (outer, root, outside)
}

#[cfg(unix)]
fn assert_link_conflicts(report: &SyncReport) {
    let mut ids: Vec<&str> = report.conflicts.iter().map(|c| c.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["file-link", "link"], "{report:?}");
    assert!(report.conflicts.iter().all(|c| !c.can_force));
    assert!(!report.has_forceable_conflicts());
}

#[cfg(unix)]
#[tokio::test]
async fn push_skips_symlinks_leaving_the_vault_and_copies_everything_else() {
    // The reviewer's reproduction: this used to return Err(InvalidId("link")), leave an empty
    // `link/` directory on the remote, and never copy whatever came after the link.
    let (_outer, root, _outside) = vault_with_links_out();
    let remote_dir = tempfile::tempdir().unwrap();
    let local = fs_backed_vault(root);
    let remote = fs_origin(remote_dir.path());

    let report = local.push(&ObjectId::from(""), &remote).await.unwrap();

    assert_link_conflicts(&report);
    let mut copied: Vec<&str> = report.copied.iter().map(|id| id.as_str()).collect();
    copied.sort();
    assert_eq!(copied, vec!["a.txt", "z.txt"]);
    let mut on_remote: Vec<String> = std::fs::read_dir(remote_dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    on_remote.sort();
    assert_eq!(on_remote, vec!["a.txt", "z.txt"]);
}

#[cfg(unix)]
#[tokio::test]
async fn pull_skips_symlinks_leaving_the_remote_vault() {
    let (_outer, remote_root, _outside) = vault_with_links_out();
    let local_dir = tempfile::tempdir().unwrap();
    let local = fs_backed_vault(local_dir.path().to_path_buf());
    let remote = fs_origin(&remote_root);

    let report = local.pull(&ObjectId::from(""), &remote).await.unwrap();

    assert_link_conflicts(&report);
    assert_eq!(std::fs::read(local_dir.path().join("a.txt")).unwrap(), b"a");
    assert_eq!(std::fs::read(local_dir.path().join("z.txt")).unwrap(), b"z");
    assert!(!local_dir.path().join("link").exists());
    assert!(!local_dir.path().join("file-link").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_destination_symlink_leaving_its_vault_is_skipped_not_written_through() {
    let outer = tempfile::tempdir().unwrap();
    let remote_root = outer.path().join("remote");
    std::fs::create_dir_all(&remote_root).unwrap();
    std::fs::write(outer.path().join("target.txt"), b"outside, keep").unwrap();
    std::os::unix::fs::symlink("../target.txt", remote_root.join("notes.txt")).unwrap();
    let local_dir = tempfile::tempdir().unwrap();
    std::fs::write(local_dir.path().join("notes.txt"), b"local").unwrap();
    std::fs::write(local_dir.path().join("other.txt"), b"other").unwrap();
    let local = fs_backed_vault(local_dir.path().to_path_buf());

    let report = local
        .push_with(
            &ObjectId::from(""),
            &fs_origin(&remote_root),
            SyncOptions { force: true },
        )
        .await
        .unwrap();

    assert_eq!(report.conflicts.len(), 1, "{report:?}");
    assert_eq!(report.conflicts[0].id.as_str(), "notes.txt");
    assert_eq!(
        std::fs::read(outer.path().join("target.txt")).unwrap(),
        b"outside, keep"
    );
    assert_eq!(
        std::fs::read(remote_root.join("other.txt")).unwrap(),
        b"other"
    );
}

/// An fs origin that refuses, with `InvalidId`, to list or fetch the ids in `refuse`: what any
/// origin does with an entry it lists but won't let you into.
struct RefusingOrigin {
    inner: crate::origin::fs::OriginFileSystem,
    refuse: &'static [&'static str],
}

#[async_trait::async_trait]
impl Origin for RefusingOrigin {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        match self.refuse.contains(&id.as_str()) {
            true => Err(VaultError::InvalidId(id.to_string())),
            false => self.inner.fetch(id).await,
        }
    }
    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>> {
        match self.refuse.contains(&id.as_str()) {
            true => Err(VaultError::InvalidId(id.to_string())),
            false => self.inner.list(id).await,
        }
    }
    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        self.inner.get(id).await
    }
    async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        self.inner.put(object, destination).await
    }
    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()> {
        self.inner.send(object, payload).await
    }
    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        self.inner.delete(id).await
    }
}

#[tokio::test]
async fn an_entry_the_source_refuses_leaves_nothing_behind_at_the_destination() {
    // Anything that can be refused (listing a directory, opening a file) must happen before
    // the entry is created on the destination; otherwise a skipped entry left an empty
    // directory or an empty file there.
    let src_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(src_dir.path().join("locked-dir")).unwrap();
    std::fs::write(src_dir.path().join("locked-dir/x.txt"), b"x").unwrap();
    std::fs::write(src_dir.path().join("locked.txt"), b"l").unwrap();
    std::fs::write(src_dir.path().join("ok.txt"), b"ok").unwrap();
    let src = RefusingOrigin {
        inner: crate::origin::fs::OriginFileSystem::new(src_dir.path().to_path_buf()),
        refuse: &["locked-dir", "locked.txt"],
    };
    let dst_dir = tempfile::tempdir().unwrap();
    let local = fs_backed_vault(dst_dir.path().to_path_buf());

    let report = local.pull(&ObjectId::from(""), &src).await.unwrap();

    let mut skipped: Vec<&str> = report.conflicts.iter().map(|c| c.id.as_str()).collect();
    skipped.sort();
    assert_eq!(skipped, vec!["locked-dir", "locked.txt"]);
    let mut on_dst: Vec<String> = std::fs::read_dir(dst_dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    on_dst.sort();
    assert_eq!(on_dst, vec!["ok.txt"]);
}
