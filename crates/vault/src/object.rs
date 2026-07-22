use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
};

use chrono::{DateTime, Utc};

use crate::{VaultResult, error::VaultError};

/// Identifies an `Object` within a `Vault`'s origin. Meaning is origin-specific
/// (a relative path for `OriginFileSystem`, an opaque id for `OriginHTTP`, etc).
///
/// # Examples
///
/// ```
/// use nimbus_vault::object::ObjectId;
///
/// let id = ObjectId::from("photos/2024/summer.jpg");
/// assert_eq!(id.as_str(), "photos/2024/summer.jpg");
/// assert!(!id.is_root());
///
/// let root = ObjectId::default();
/// assert_eq!(root.as_str(), "/");
/// assert!(root.is_root());
///
/// // Vault methods accept anything that converts into an ObjectId:
/// let from_string: ObjectId = String::from("notes.txt").into();
/// let from_str: ObjectId = "notes.txt".into();
/// assert_eq!(from_string, from_str);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ObjectId(String);
impl ObjectId {
    /// Borrows the id's underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Whether this id refers to the vault's root (`"/"`), the `ObjectId::default()` value.
    pub fn is_root(&self) -> bool {
        self.0.as_str() == crate::ROOT_ID
    }
}
impl From<&ObjectId> for ObjectId {
    fn from(id: &ObjectId) -> Self {
        id.clone()
    }
}

impl Default for ObjectId {
    /// Defaults to `"/"`, the conventional root id used when a `VaultConfig` omits `root_id`.
    fn default() -> Self {
        ObjectId(String::from(crate::ROOT_ID))
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
///
/// # Examples
///
/// ```
/// use nimbus_vault::object::Metadata;
///
/// let mut meta = Metadata::new();
/// meta.set_size(1024)
///     .set_content_type("text/plain".to_string())
///     .add_extra("checksum".to_string(), "abc123".to_string());
///
/// assert_eq!(meta.size, Some(1024));
/// assert_eq!(meta.content_type.as_deref(), Some("text/plain"));
/// assert_eq!(meta.extra.get("checksum").map(String::as_str), Some("abc123"));
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
    /// Payload size in bytes, if known.
    pub size: Option<u64>,
    /// MIME type of the payload, if known.
    pub content_type: Option<String>,
    /// Last-modified timestamp, if known.
    pub modified: Option<DateTime<Utc>>,
    /// Free-form, origin-specific key/value pairs not covered by the fields above.
    pub extra: HashMap<String, String>,
}
impl Metadata {
    /// Builds an empty `Metadata` with every field unset.
    pub fn new() -> Self {
        Metadata {
            size: None,
            content_type: None,
            modified: None,
            extra: HashMap::new(),
        }
    }
    /// Sets `size` and returns `self` for chaining.
    pub fn set_size(&mut self, size: u64) -> &mut Self {
        self.size = Some(size);
        self
    }
    /// Sets `content_type` and returns `self` for chaining.
    pub fn set_content_type(&mut self, ct: String) -> &mut Self {
        self.content_type = Some(ct);
        self
    }
    /// Sets `modified` and returns `self` for chaining.
    pub fn set_modified(&mut self, dt: DateTime<Utc>) -> &mut Self {
        self.modified = Some(dt);
        self
    }
    /// Inserts a key/value pair into `extra` and returns `self` for chaining.
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
///
/// # Examples
///
/// Objects returned by an origin are typically matched by variant:
///
/// ```
/// use nimbus_vault::object::{Metadata, Object, ObjectId};
///
/// let object = Object::Leaf {
///     name: "notes.txt".to_string(),
///     id: ObjectId::from("notes.txt"),
///     meta: Metadata::new(),
/// };
///
/// match &object {
///     Object::Leaf { name, .. } => println!("file: {name}"),
///     Object::Branch { name, .. } => println!("directory: {name}"),
///     Object::Root { .. } => println!("root"),
/// }
///
/// // or via the variant-agnostic accessors:
/// assert_eq!(object.get_name(), "notes.txt");
/// assert_eq!(object.get_id().as_str(), "notes.txt");
/// assert!(object.get_meta().is_some());
/// ```
#[derive(Serialize, Deserialize, Clone)]
pub enum Object {
    /// A file-like node with content but no children.
    Leaf {
        /// The leaf's display name (typically the last path component).
        name: String,
        /// The leaf's id within its origin.
        id: ObjectId,
        /// The leaf's metadata.
        meta: Metadata,
    },
    /// A directory-like node with children but no content of its own.
    Branch {
        /// The branch's display name (typically the last path component).
        name: String,
        /// The branch's id within its origin.
        id: ObjectId,
        /// The branch's metadata.
        meta: Metadata,
        /// The branch's children, once known (populated by `Vault::list`).
        children: Option<Vec<ObjectId>>,
    },
    /// The vault's root node; has children but no name/metadata of its own.
    Root {
        /// The root's id (conventionally `ObjectId::default()`, i.e. `"/"`).
        id: ObjectId,
        /// The root's children, once known (populated by `Vault::list`).
        children: Option<Vec<ObjectId>>,
    },
}

/// Selects which `Object` variant to build in `Object::push`.
pub enum ObjectType {
    /// Build a `Object::Leaf`.
    Leaf,
    /// Build a `Object::Branch`.
    Branch,
    /// Build a `Object::Root`; always rejected by `Object::push` since a vault has exactly
    /// one root.
    Root,
}
impl Object {
    /// Builds the canonical root `Object`, with id `"/"` and no known children.
    ///
    /// # Examples
    ///
    /// ```
    /// use nimbus_vault::object::Object;
    ///
    /// let root = Object::root();
    /// assert_eq!(root.get_id().as_str(), "/");
    /// assert!(root.get_meta().is_none());
    /// ```
    pub fn root() -> Self {
        Object::Root {
            id: ObjectId::from(crate::ROOT_ID),
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
    /// Returns this object's id, regardless of variant.
    pub fn get_id(&self) -> ObjectId {
        match self {
            Object::Root { id, .. } | Object::Leaf { id, .. } | Object::Branch { id, .. } => {
                id.clone()
            }
        }
    }
    /// Overwrites this object's id in place and returns `self` for chaining.
    pub fn with_id(&mut self, obj_id: ObjectId) -> &mut Self {
        match self {
            Object::Root { id, .. } | Object::Leaf { id, .. } | Object::Branch { id, .. } => {
                *id = obj_id;
            }
        }
        self
    }
    /// Returns this object's display name, or [`crate::ROOT_NAME`] for `Object::Root`.
    pub fn get_name(&self) -> String {
        match self {
            Object::Leaf { name, .. } | Object::Branch { name, .. } => name.clone(),
            _ => String::from(crate::ROOT_NAME),
        }
    }
    /// Returns this object's metadata, or `None` for `Object::Root` (which has none).
    pub fn get_meta(&self) -> Option<Metadata> {
        match self {
            Object::Leaf { meta, .. } | Object::Branch { meta, .. } => Some(meta.clone()),
            _ => None,
        }
    }
    /// Appends a new child of `object_type` onto `self`, which must be a `Branch`/`Root`.
    ///
    /// # Examples
    ///
    /// ```
    /// use nimbus_vault::object::{Metadata, Object, ObjectId, ObjectType};
    ///
    /// let mut root = Object::root();
    /// root.push(
    ///     ObjectType::Leaf,
    ///     "notes.txt".to_string(),
    ///     ObjectId::from("notes.txt"),
    ///     Metadata::new(),
    ///     None,
    /// )?;
    ///
    /// let children = match &root {
    ///     Object::Root { children, .. } => children.as_ref().unwrap(),
    ///     _ => unreachable!(),
    /// };
    /// assert_eq!(children[0].as_str(), "notes.txt");
    /// # Ok::<(), nimbus_vault::error::VaultError>(())
    /// ```
    ///
    /// Pushing onto a `Leaf`, or pushing an `ObjectType::Root`, both fail with
    /// `VaultError::InvalidMethodCall`:
    ///
    /// ```
    /// use nimbus_vault::{error::VaultError, object::{Metadata, Object, ObjectId, ObjectType}};
    ///
    /// let mut leaf = Object::Leaf {
    ///     name: "notes.txt".to_string(),
    ///     id: ObjectId::from("notes.txt"),
    ///     meta: Metadata::new(),
    /// };
    /// let result = leaf.push(ObjectType::Leaf, "x".to_string(), ObjectId::from("x"), Metadata::new(), None);
    /// assert!(matches!(result, Err(VaultError::InvalidMethodCall)));
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```
    /// use nimbus_vault::object::{Metadata, Object, ObjectId};
    ///
    /// let local = Object::Leaf {
    ///     name: "notes.txt".to_string(),
    ///     id: ObjectId::from("notes.txt"),
    ///     meta: Metadata::new(),
    /// };
    /// let identical = local.clone();
    /// assert!(!local.changed(&identical));
    ///
    /// let mut different_meta = Metadata::new();
    /// different_meta.set_size(42);
    /// let diverged = Object::Leaf {
    ///     name: "notes.txt".to_string(),
    ///     id: ObjectId::from("notes.txt"),
    ///     meta: different_meta,
    /// };
    /// assert!(local.changed(&diverged));
    /// ```
    pub fn changed(&self, remote: &Object) -> bool {
        let (Some(local_meta), Some(remote_meta)) = (self.get_meta(), remote.get_meta()) else {
            return false;
        };
        local_meta.hash_value() != remote_meta.hash_value()
    }
}

#[cfg(test)]
#[path = "tests/object.rs"]
mod tests;
