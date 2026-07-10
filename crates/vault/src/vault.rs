use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

use crate::{
    VaultResult,
    config::VaultConfig,
    error::VaultError,
    object::{Object, ObjectId},
    origin::{ByteStream, Origin},
};

/// A tree-like view over an `Origin`, keyed from a single `root` `ObjectId`.
/// Caches fetched `Object`s in memory so repeat `get`/`list` calls can skip the origin.
pub struct Vault {
    name: String,
    origin: Arc<dyn Origin>,
    objects: Mutex<HashMap<ObjectId, Object>>,
    root: ObjectId,
}

impl Vault {
    /// Reads a `VaultConfig` from the TOML file at `from` and builds the `Vault` it describes.
    pub fn new(from: PathBuf) -> VaultResult<Self> {
        let (name, root_id, origin) = VaultConfig::build(from)?;
        Ok(Vault {
            name,
            origin: Arc::from(origin),
            objects: Mutex::new(HashMap::new()),
            root: root_id,
        })
    }

    #[cfg(test)]
    fn from_origin(name: String, root_id: ObjectId, origin: Arc<dyn Origin>) -> Self {
        Vault {
            name,
            origin,
            root: root_id,
            objects: Mutex::new(HashMap::new()),
        }
    }
    pub fn get_name(&self) -> &String {
        &self.name
    }
    pub fn get_origin(&self) -> Arc<dyn Origin> {
        self.origin.clone()
    }
    /// Resolves a filesystem-style `path` to an `ObjectId`, starting at `root` and walking
    /// each component via `list`, matching on child name. Errors with `VaultError::NotFound`
    /// as soon as a component has no matching child.
    pub async fn find(&self, path: PathBuf) -> VaultResult<ObjectId> {
        let mut current = self.root.clone();

        for component in path.components() {
            let Some(name) = component.as_os_str().to_str() else {
                return Err(VaultError::Generic("invalid path component".into()));
            };
            if name == "/" {
                continue;
            }

            let children = self.list(current.clone()).await?;

            let child = children
                .iter()
                .find(|c| c.get_name() == name)
                .ok_or_else(|| VaultError::NotFound(name.to_string()))?;

            current = child.get_id();
        }

        Ok(current)
    }
    /// Streams the payload for `id` straight from the origin. Not cached, since payloads
    /// aren't kept in the in-memory `Object` cache.
    pub async fn fetch(&self, id: impl Into<ObjectId>) -> VaultResult<ByteStream> {
        let id = id.into();
        self.origin.fetch(&id).await
    }
    /// Lists `id`'s children via the origin, caching each child and recording the resulting
    /// child id list on `id`'s own cache entry (if it's already cached as a `Branch`/`Root`).
    /// Always hits the origin — unlike `get`, this does not read from the cache.
    pub async fn list(&self, id: impl Into<ObjectId>) -> VaultResult<Vec<Object>> {
        let id = id.into();
        let children = self.origin.list(&id).await?;
        let child_ids: Vec<ObjectId> = children.iter().map(|child| child.get_id()).collect();
        let mut cache = self.objects.lock().await;
        for child in &children {
            cache.insert(child.get_id(), child.clone());
        }
        if let Some(Object::Branch { children: c, .. }) | Some(Object::Root { children: c, .. }) =
            cache.get_mut(&id)
        {
            *c = Some(child_ids)
        }
        Ok(children)
    }
    /// Returns `id`'s `Object`, serving it from the in-memory cache when present and otherwise
    /// fetching it from the origin and caching the result.
    pub async fn get(&self, id: impl Into<ObjectId>) -> VaultResult<Object> {
        let id = id.into();

        let cache = self.objects.lock().await;
        if let Some(object) = cache.get(&id) {
            return Ok(object.clone());
        }
        drop(cache);
        let object = self.origin.get(&id).await?;
        let mut cache = self.objects.lock().await;
        cache.insert(id, object.clone());
        Ok(object)
    }
    /// Streams `payload` to the origin as `object`'s contents. Not cached, mirroring `fetch`.
    pub async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()> {
        self.origin.send(object, payload).await
    }
    /// Writes `object` to the origin and caches it under its own id.
    pub async fn put(&self, object: &Object) -> VaultResult<()> {
        self.origin.put(&object).await?;
        self.objects
            .lock()
            .await
            .insert(object.get_id(), object.clone());
        Ok(())
    }
    /// Deletes `id` from the origin and evicts it from the cache.
    pub async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        self.origin.delete(id).await?;
        self.objects.lock().await.remove(id);
        Ok(())
    }
    /// Recursively syncs `id`'s subtree from `remote` into this vault's origin.
    /// For each of `remote`'s children under `id`: fetches this vault's copy (if any) and
    /// compares metadata via `Object::changed`; a missing local copy or diverging metadata
    /// triggers a `put` (and, for `Leaf`s, a `fetch`+`send` of the payload). `Branch`/`Root`
    /// children are then recursed into regardless of whether they themselves changed, so their
    /// descendants are still visited. Errors from `remote`/`self` other than `NotFound`
    /// short-circuit the whole walk.
    pub async fn pull(&self, id: &ObjectId, remote: &dyn Origin) -> VaultResult<()> {
        let remote_children = remote.list(id).await?;
        for remote_obj in remote_children {
            let remote_id = remote_obj.get_id();
            let needs_sync = match self.get(remote_id.clone()).await {
                Ok(local_obj) => local_obj.changed(&remote_obj),
                Err(VaultError::NotFound(_)) => true,
                Err(e) => return Err(e),
            };

            if needs_sync {
                self.put(&remote_obj).await?;
                if let Object::Leaf { .. } = remote_obj {
                    let bytes = remote.fetch(&remote_id).await?;
                    self.send(&remote_obj, bytes).await?;
                }
            }
            if matches!(remote_obj, Object::Branch { .. } | Object::Root { .. }) {
                Box::pin(self.pull(&remote_id, remote)).await?;
            }
        }
        Ok(())
    }
    /// The mirror image of `pull`: recursively syncs `id`'s subtree from this vault's origin
    /// out to `remote`, walking children via `self.list` instead of `remote.list` and writing
    /// through `remote.put`/`remote.send` instead of `self.put`/`self.send`. Same change
    /// detection and recursion rules as `pull` apply.
    pub async fn push(&self, id: &ObjectId, remote: &dyn Origin) -> VaultResult<()> {
        let local_children = self.list(id.clone()).await?;

        for local_obj in local_children {
            let local_id = local_obj.get_id();

            let needs_sync = match remote.get(&local_id).await {
                Ok(remote_obj) => local_obj.changed(&remote_obj),
                Err(VaultError::NotFound(_)) => true,
                Err(e) => return Err(e),
            };

            if needs_sync {
                remote.put(&local_obj).await?;
                if let Object::Leaf { .. } = local_obj {
                    let bytes = self.fetch(local_id.clone()).await?;
                    remote.send(&local_obj, bytes).await?;
                }
            }

            if matches!(local_obj, Object::Branch { .. } | Object::Root { .. }) {
                Box::pin(self.push(&local_id, remote)).await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
        async fn put(&self, _object: &Object) -> VaultResult<()> {
            Ok(())
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
        async fn put(&self, object: &Object) -> VaultResult<()> {
            self.put_calls.lock().unwrap().push(object.get_id());
            self.objects
                .lock()
                .unwrap()
                .insert(object.get_id(), object.clone());
            Ok(())
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
        async fn put(&self, _object: &Object) -> VaultResult<()> {
            Err(VaultError::Generic("put should not be called".into()))
        }
        async fn send(&self, _object: &Object, _payload: ByteStream) -> VaultResult<()> {
            Err(VaultError::Generic("send should not be called".into()))
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
        let object = Object::Leaf {
            name: "file".to_string(),
            id: ObjectId::from("f1"),
            meta: Metadata::new(),
        };
        vault.put(&object).await.unwrap();
        let cached = vault.get("f1").await.unwrap();
        assert_eq!(cached.get_name(), "file");
        assert!(origin.get_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_evicts_object_from_cache() {
        let (vault, origin) = make_vault_with_origin();
        let object = Object::Leaf {
            name: "file".to_string(),
            id: ObjectId::from("f1"),
            meta: Metadata::new(),
        };
        vault.put(&object).await.unwrap();
        vault.delete(&ObjectId::from("f1")).await.unwrap();
        vault.get("f1").await.unwrap();
        assert_eq!(origin.get_calls.lock().unwrap().len(), 1);
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
}
