use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
};

use chrono::{DateTime, Utc};

use crate::{VaultResult, error::VaultError};

/// Checks that `name` is safe to use as one path component on any origin: not empty, not `.` or
/// `..`, and free of `/`, `\` and NUL. Every origin's `put` calls this, because a name is the
/// one piece of a transfer the *other* side controls — without it, a remote listing an object
/// named `../../.bashrc` could have `pull` write outside the destination vault.
///
/// # Examples
///
/// ```
/// use nimbus_vault::object::validate_name;
///
/// assert!(validate_name("notes.txt").is_ok());
/// assert!(validate_name("a#b ?.txt").is_ok());
/// assert!(validate_name("../escape").is_err());
/// assert!(validate_name("/etc/passwd").is_err());
/// assert!(validate_name("..").is_err());
/// assert!(validate_name("").is_err());
/// ```
pub fn validate_name(name: &str) -> VaultResult<()> {
    let bad = name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']);
    match bad {
        true => Err(VaultError::InvalidName(name.to_string())),
        false => Ok(()),
    }
}

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
#[derive(Serialize, Deserialize, Clone, Debug)]
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
    /// Overwrites this object's display name in place and returns `self` for chaining. A
    /// no-op for `Object::Root`, which has no name of its own. Since an origin's `put` writes
    /// an object under `get_name()` in the destination, setting a new name before a `put` is
    /// what turns a copy into a rename.
    ///
    /// # Examples
    ///
    /// ```
    /// use nimbus_vault::object::{Metadata, Object, ObjectId};
    ///
    /// let mut object = Object::Leaf {
    ///     name: "notes.txt".to_string(),
    ///     id: ObjectId::from("notes.txt"),
    ///     meta: Metadata::new(),
    /// };
    /// object.with_name("renamed.txt".to_string());
    /// assert_eq!(object.get_name(), "renamed.txt");
    ///
    /// // the root has no name to set
    /// let mut root = Object::root();
    /// root.with_name("ignored".to_string());
    /// assert_eq!(root.get_name(), nimbus_vault::ROOT_NAME);
    /// ```
    pub fn with_name(&mut self, obj_name: String) -> &mut Self {
        match self {
            Object::Leaf { name, .. } | Object::Branch { name, .. } => {
                *name = obj_name;
            }
            Object::Root { .. } => {}
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

    /// Replaces this object's metadata. A no-op for `Object::Root`, which has none.
    ///
    /// Used before a `send` to carry the *source's* metadata (above all its modification time)
    /// onto the object being written, so the copy ends up comparing equal to its source.
    pub fn set_meta(&mut self, new_meta: Metadata) -> &mut Self {
        if let Object::Leaf { meta, .. } | Object::Branch { meta, .. } = self {
            *meta = new_meta;
        }
        self
    }

    /// Decides what syncing `self` (the source) onto `dest` should do. Unlike
    /// [`Self::changed`], this knows which side is newer, so a sync never overwrites a
    /// destination that was edited after the source.
    ///
    /// - A file and a directory with the same id are a [`SyncDecision::KindMismatch`].
    /// - Two directories are always [`SyncDecision::Same`]: a directory has no content of its
    ///   own, and its children are compared one by one.
    /// - Two files with identical metadata are `Same`. Otherwise, when both carry a
    ///   modification time, the later one wins: [`SyncDecision::SourceNewer`] or
    ///   [`SyncDecision::DestNewer`]. Anything else (a missing time, equal times with
    ///   different sizes) is [`SyncDecision::Unknown`], which a sync treats as "copy".
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::{TimeZone, Utc};
    /// use nimbus_vault::object::{Metadata, Object, ObjectId, SyncDecision};
    ///
    /// let file = |hour| {
    ///     let mut meta = Metadata::new();
    ///     meta.set_size(5).set_modified(Utc.with_ymd_and_hms(2026, 1, 1, hour, 0, 0).unwrap());
    ///     Object::Leaf { name: "a".into(), id: ObjectId::from("a"), meta }
    /// };
    /// assert_eq!(file(9).compare(&file(9)), SyncDecision::Same);
    /// assert_eq!(file(10).compare(&file(9)), SyncDecision::SourceNewer);
    /// assert_eq!(file(9).compare(&file(10)), SyncDecision::DestNewer);
    /// ```
    pub fn compare(&self, dest: &Object) -> SyncDecision {
        let is_leaf = |o: &Object| matches!(o, Object::Leaf { .. });
        match (is_leaf(self), is_leaf(dest)) {
            (false, false) => return SyncDecision::Same,
            (true, false) | (false, true) => return SyncDecision::KindMismatch,
            (true, true) => {}
        }
        let (Some(src), Some(dst)) = (self.get_meta(), dest.get_meta()) else {
            return SyncDecision::Unknown;
        };
        if src.hash_value() == dst.hash_value() {
            return SyncDecision::Same;
        }
        match (src.modified, dst.modified) {
            (Some(s), Some(d)) if s > d => SyncDecision::SourceNewer,
            (Some(s), Some(d)) if s < d => SyncDecision::DestNewer,
            _ => SyncDecision::Unknown,
        }
    }
}

/// The outcome of [`Object::compare`]: what syncing a source object onto its destination
/// counterpart should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDecision {
    /// Nothing to do: identical files, or two directories.
    Same,
    /// The source was modified after the destination; copy it.
    SourceNewer,
    /// The destination was modified after the source. Copying would lose that newer edit, so
    /// a sync reports a conflict instead (unless forced).
    DestNewer,
    /// Not enough metadata to tell which side is newer; a sync copies.
    Unknown,
    /// One side is a file and the other a directory. Never overwritten, even when forced.
    KindMismatch,
}

#[cfg(test)]
#[path = "tests/object.rs"]
mod tests;
