use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
};

use chrono::{DateTime, Utc};

use crate::{VaultResult, error::VaultError};

/// Identifies an `Object` within a `Vault`'s origin. Meaning is origin-specific
/// (a relative path for `OriginFileSystem`, an opaque id for `OriginHTTP`, etc).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ObjectId(String);
impl ObjectId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Whether this id refers to the vault's root (`"/"`), the `ObjectId::default()` value.
    pub fn is_root(&self) -> bool {
        self.0.as_str() == "/"
    }
}

impl Default for ObjectId {
    /// Defaults to `"/"`, the conventional root id used when a `VaultConfig` omits `root_id`.
    fn default() -> Self {
        ObjectId(String::from("/"))
    }
}
impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ObjectId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
impl From<String> for ObjectId {
    fn from(s: String) -> Self {
        ObjectId(s)
    }
}
impl From<&str> for ObjectId {
    fn from(s: &str) -> Self {
        ObjectId(s.to_string())
    }
}
impl From<ObjectId> for String {
    fn from(id: ObjectId) -> Self {
        id.0
    }
}

/// Free-form metadata attached to an `Object`, used e.g. by `Object::changed` to detect drift
/// between a local and remote copy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub size: Option<u64>,
    pub content_type: Option<String>,
    pub modified: Option<DateTime<Utc>>,
    pub extra: HashMap<String, String>,
}
impl Metadata {
    pub fn new() -> Self {
        Metadata {
            size: None,
            content_type: None,
            modified: None,
            extra: HashMap::new(),
        }
    }
    pub fn set_size(&mut self, size: u64) -> &mut Self {
        self.size = Some(size);
        self
    }
    pub fn set_content_type(&mut self, ct: String) -> &mut Self {
        self.content_type = Some(ct);
        self
    }
    pub fn set_modified(&mut self, dt: DateTime<Utc>) -> &mut Self {
        self.modified = Some(dt);
        self
    }
    pub fn add_extra(&mut self, key: String, val: String) -> &mut Self {
        self.extra.insert(key, val);
        self
    }
    /// Hashes all fields (with `extra` sorted by key for stable ordering) for equality checks.
    pub fn hash_value(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}
impl Hash for Metadata {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.size.hash(state);
        self.content_type.hash(state);
        self.modified.hash(state);
        let mut entries: Vec<_> = self.extra.iter().collect();
        entries.sort_by_key(|(k, _)| k.to_string());
        entries.hash(state);
    }
}

/// A node in a `Vault`'s tree: a file-like `Leaf`, a directory-like `Branch`, or the `Root`.
#[derive(Serialize, Deserialize, Clone)]
pub enum Object {
    Leaf {
        name: String,
        id: ObjectId,
        meta: Metadata,
    },
    Branch {
        name: String,
        id: ObjectId,
        meta: Metadata,
        children: Option<Vec<ObjectId>>,
    },
    Root {
        id: ObjectId,
        children: Option<Vec<ObjectId>>,
    },
}

/// Selects which `Object` variant to build in `Object::push`.
pub enum ObjectType {
    Leaf,
    Branch,
    Root,
}
impl Object {
    pub fn root() -> Self {
        Object::Root {
            id: ObjectId::from("/"),
            children: None,
        }
    }
    fn leaf(name: String, id: ObjectId, meta: Metadata) -> Self {
        Object::Leaf { name, id, meta }
    }
    fn branch(name: String, id: ObjectId, meta: Metadata, children: Vec<ObjectId>) -> Self {
        Object::Branch {
            name,
            id,
            meta,
            children: Some(children),
        }
    }
    pub fn get_id(&self) -> ObjectId {
        match self {
            Object::Root { id, .. } | Object::Leaf { id, .. } | Object::Branch { id, .. } => {
                id.clone()
            }
        }
    }
    pub fn get_name(&self) -> String {
        match self {
            Object::Leaf { name, .. } | Object::Branch { name, .. } => name.clone(),
            _ => String::from("root"),
        }
    }
    pub fn get_meta(&self) -> Option<Metadata> {
        match self {
            Object::Leaf { meta, .. } | Object::Branch { meta, .. } => Some(meta.clone()),
            _ => None,
        }
    }
    ///add children to Branch
    pub fn push(
        &mut self,
        object_type: ObjectType,
        name: String,
        id: ObjectId,
        meta: Metadata,
        children: Option<Vec<ObjectId>>,
    ) -> VaultResult<()> {
        let object = match object_type {
            ObjectType::Leaf => Object::leaf(name, id, meta),

            ObjectType::Branch => Object::branch(name, id, meta, children.unwrap_or_default()),
            ObjectType::Root => {
                return Err(VaultError::InvalidMethodCall);
            }
        };

        match self {
            Object::Branch {
                children: child, ..
            }
            | Object::Root {
                children: child, ..
            } => {
                match child {
                    Some(c) => c.push(object.get_id()),
                    None => {
                        *child = Some(vec![object.get_id()]);
                    }
                }
                Ok(())
            }
            Object::Leaf { .. } => Err(VaultError::InvalidMethodCall),
        }
    }
    /// Compares metadata hashes to detect whether `remote` diverges from `self`.
    /// Returns `false` for objects with no metadata (e.g. `Root`).
    pub fn changed(&self, remote: &Object) -> bool {
        let (Some(local_meta), Some(remote_meta)) = (self.get_meta(), remote.get_meta()) else {
            return false;
        };
        local_meta.hash_value() != remote_meta.hash_value()
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(default_id.as_str(), "/");
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
        assert_eq!(root.get_id().as_str(), "/");
        assert_eq!(root.get_name(), "root");
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
}
