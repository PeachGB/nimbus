use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use tokio::sync::Mutex;

use crate::{
    ROOT_ID, VaultResult,
    crypt::{CryptError, CryptKey, ciphertext_len, decrypt, encrypt, plaintext_len},
    error::VaultError,
    object::{Metadata, Object, ObjectId},
    origin::{ByteStream, Origin},
};

/// Name of the marker object a crypt origin keeps at its inner origin's root.
pub const MARKER_NAME: &str = ".nimbus-crypt";
/// What the marker holds, encrypted: opening it proves the key is the vault's key.
const MARKER_PLAINTEXT: &[u8] = b"nimbus-crypt vault marker v1";

/// An origin that stores every object of another origin encrypted at rest: `send` encrypts,
/// `fetch` decrypts, and listings report plaintext sizes, computed from the stored sizes
/// without reading anything (see [`crate::crypt`] for the format). The key never reaches the
/// inner origin, so the storage (a server, a bucket, a backup disk) only ever holds ciphertext.
///
/// Names, the tree, sizes and modification times pass through in the clear, so sync works as
/// it does over a plain origin. Moves and deletes pass through untouched.
///
/// A marker object, [`MARKER_NAME`] at the inner origin's root, records which key the vault
/// uses. It is checked before the first operation and written by the first write; a different
/// key is then refused with [`CryptError::WrongKey`] before anything is read or written, so two
/// keys can't end up mixed in one vault. Listings hide it (by its id, so a file of the same
/// name elsewhere in the tree stays visible).
///
/// The marker is reserved: creating, overwriting, moving or deleting it through this origin is
/// refused. The first write reads it back after creating it, so of two first writers with
/// different keys, the one whose marker didn't stand fails instead of writing. Two writers
/// creating it at exactly the same moment can still both read back their own before the other
/// lands; the loser's next operation then fails with `WrongKey`, and the object it wrote in
/// that moment is readable only with its own key.
///
/// # Examples
///
/// ```
/// use futures::{StreamExt, TryStreamExt, stream};
/// use nimbus_vault::{
///     crypt::CryptKey,
///     object::{Metadata, Object, ObjectId},
///     origin::{Origin, crypt::OriginCrypt, fs::OriginFileSystem},
/// };
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// let origin = OriginCrypt::new(
///     Box::new(OriginFileSystem::new(dir.path().to_path_buf())),
///     CryptKey::generate()?,
/// );
///
/// let mut note = Object::Leaf { name: "note.txt".into(), id: "note.txt".into(), meta: Metadata::new() };
/// let note = origin.put(&mut note, &ObjectId::from("/")).await?;
/// origin.send(&note, stream::iter([Ok(bytes::Bytes::from_static(b"secret"))]).boxed()).await?;
///
/// // On disk: ciphertext only.
/// assert!(!std::fs::read(dir.path().join("note.txt"))?.windows(6).any(|w| w == b"secret"));
/// // Through the origin: the plaintext, and its size.
/// let read: Vec<_> = origin.fetch(&note.get_id()).await?.try_collect().await?;
/// assert_eq!(read.concat(), b"secret");
/// assert_eq!(origin.get(&note.get_id()).await?.get_meta().unwrap().size, Some(6));
/// # Ok(())
/// # }
/// ```
pub struct OriginCrypt {
    inner: Box<dyn Origin>,
    key: CryptKey,
    marker: Mutex<Marker>,
}

/// What is known about the vault's marker.
#[derive(Clone, PartialEq, Eq)]
enum Marker {
    /// Not looked at yet.
    Unchecked,
    /// Looked for and absent: reads may go ahead; the next write creates it.
    Absent,
    /// Present, opened with this key, and stored under this id (which listings hide).
    Verified(ObjectId),
}

impl OriginCrypt {
    /// Wraps `inner`, encrypting everything stored in it with `key`.
    pub fn new(inner: Box<dyn Origin>, key: CryptKey) -> Self {
        OriginCrypt {
            inner,
            key,
            marker: Mutex::new(Marker::Unchecked),
        }
    }

    /// Makes sure the vault's marker opens with this key before an operation goes ahead. A
    /// write (`writing`) creates the marker when there is none; a read doesn't. Only success
    /// is remembered, so a failure is checked again next time.
    /// Returns the marker's id once there is one, for listings to hide.
    async fn check_key(&self, writing: bool) -> VaultResult<Option<ObjectId>> {
        let mut marker = self.marker.lock().await;
        match &*marker {
            Marker::Verified(id) => return Ok(Some(id.clone())),
            Marker::Absent if !writing => return Ok(None),
            _ => {}
        }
        *marker = match self.find_marker().await? {
            Some(found) => {
                self.verify_marker(&found).await?;
                Marker::Verified(found.get_id())
            }
            None if writing => {
                self.write_marker().await?;
                // Read back and opened, not trusted: another writer with another key may have
                // written its own marker at the same moment, and the last one stands. The id
                // remembered is the one a listing gives it, which an origin may spell
                // differently from what its `put` returned (over http, say).
                let found = self.find_marker().await?.ok_or_else(|| {
                    VaultError::Generic(format!(
                        "{MARKER_NAME} vanished right after it was written"
                    ))
                })?;
                self.verify_marker(&found).await?;
                Marker::Verified(found.get_id())
            }
            None => Marker::Absent,
        };
        Ok(match &*marker {
            Marker::Verified(id) => Some(id.clone()),
            _ => None,
        })
    }

    /// The marker object, if the inner origin's root has one.
    async fn find_marker(&self) -> VaultResult<Option<Object>> {
        match self.inner.list(&ObjectId::from(ROOT_ID)).await {
            Ok(children) => Ok(children.into_iter().find(is_marker)),
            Err(VaultError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Opens the marker: [`CryptError::WrongKey`] if it was written with another key.
    async fn verify_marker(&self, marker: &Object) -> VaultResult<()> {
        let stored = self.inner.fetch(&marker.get_id()).await?;
        let opened: Vec<Bytes> = decrypt(&self.key, stored).try_collect().await?;
        if opened.concat() != MARKER_PLAINTEXT {
            return Err(CryptError::Corrupt { chunk: 0 }.into());
        }
        Ok(())
    }

    /// Writes a new marker at the inner origin's root.
    async fn write_marker(&self) -> VaultResult<()> {
        let sealed = self.seal_small(MARKER_PLAINTEXT).await?;
        let mut marker = Object::Leaf {
            name: MARKER_NAME.to_string(),
            id: ObjectId::from(MARKER_NAME),
            meta: Metadata::new(),
        };
        let mut stored = self
            .inner
            .put(&mut marker, &ObjectId::from(ROOT_ID))
            .await?;
        set_size(&mut stored, sealed.len() as u64);
        self.inner.send(&stored, one_chunk(sealed)).await
    }

    /// `plaintext` encrypted in memory, for the marker and the empty payload of a new leaf.
    async fn seal_small(&self, plaintext: &'static [u8]) -> VaultResult<Vec<u8>> {
        let input = futures::stream::iter([Ok(Bytes::from_static(plaintext))]).boxed();
        let sealed: Vec<Bytes> = encrypt(&self.key, input)?.try_collect().await?;
        Ok(sealed.concat())
    }

    /// `object` as the caller sees it: a leaf's stored size turned into its plaintext size.
    ///
    /// A stored size that can't be a ciphertext's length is handled two ways on purpose.
    /// `get` is strict: a single object was asked for, so it is an error
    /// ([`CryptError::Truncated`]). `list` is lenient: the size becomes unknown, so one
    /// damaged object doesn't break the listing of its whole directory, and a sync sees it as
    /// different from anything (a push then repairs it; a pull's `fetch` reports the error).
    fn plaintext_view(mut object: Object, strict: bool) -> VaultResult<Object> {
        if let Object::Leaf { meta, .. } = &mut object
            && let Some(stored) = meta.size
        {
            meta.size = match plaintext_len(stored) {
                Ok(size) => Some(size),
                Err(error) if strict => return Err(error.into()),
                Err(_) => None,
            };
        }
        Ok(object)
    }

    /// `object` as the inner origin must see it for a `send`: its size turned into the size of
    /// its ciphertext (a command template can use `{size}`).
    fn ciphertext_view(object: &Object) -> VaultResult<Object> {
        let mut object = object.clone();
        if let Object::Leaf { meta, .. } = &mut object
            && let Some(plain) = meta.size
        {
            meta.size = Some(ciphertext_len(plain).ok_or(CryptError::TooLarge)?);
        }
        Ok(object)
    }
}

#[async_trait::async_trait]
impl Origin for OriginCrypt {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
        self.check_key(false).await?;
        Ok(decrypt(&self.key, self.inner.fetch(id).await?))
    }

    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>> {
        let marker = self.check_key(false).await?;
        self.inner
            .list(id)
            .await?
            .into_iter()
            .filter(|child| marker.as_ref() != Some(&child.get_id()))
            .map(|child| Self::plaintext_view(child, false))
            .collect()
    }

    async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
        self.check_key(false).await?;
        Self::plaintext_view(self.inner.get(id).await?, true)
    }

    /// A new leaf (one the inner origin holds as 0 bytes after its `put`) is given an
    /// encrypted empty payload straight away, so every object this origin creates is a valid
    /// ciphertext even if the `send` that should follow never comes. An existing object is
    /// left exactly as it is: an overwrite is a `put` then a `send`, and its old contents must
    /// survive until the `send` replaces them, even if that `send` fails.
    ///
    /// An inner origin that reports no size for a fresh object (a command origin whose
    /// `get_cmd` prints `null`, say) gets no empty payload: the object stays 0 bytes until its
    /// `send`, and reads as [`CryptError::Truncated`] until then, never as silently empty.
    async fn put(&self, object: &mut Object, destination: &ObjectId) -> VaultResult<Object> {
        self.check_key(true).await?;
        refuse_marker_name(destination, &object.get_name())?;
        let stored = self.inner.put(object, destination).await?;
        if !matches!(stored, Object::Leaf { .. }) {
            return Ok(stored);
        }
        let current = self.inner.get(&stored.get_id()).await?;
        if current.get_meta().and_then(|meta| meta.size) != Some(0) {
            return Self::plaintext_view(current, true);
        }
        let sealed = self.seal_small(b"").await?;
        let mut empty = stored.clone();
        set_size(&mut empty, sealed.len() as u64);
        self.inner.send(&empty, one_chunk(sealed)).await?;
        Self::plaintext_view(self.inner.get(&stored.get_id()).await?, true)
    }

    /// Encrypts `payload` under a fresh salt, so every send (an overwrite included) uses a new
    /// object key.
    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()> {
        let marker = self.check_key(true).await?;
        refuse_marker_target(marker.as_ref(), &object.get_id())?;
        let sealed = encrypt(&self.key, payload)?;
        self.inner
            .send(&Self::ciphertext_view(object)?, sealed)
            .await
    }

    async fn delete(&self, id: &ObjectId) -> VaultResult<()> {
        let marker = self.check_key(true).await?;
        refuse_marker_target(marker.as_ref(), id)?;
        self.inner.delete(id).await
    }

    async fn rename(
        &self,
        id: &ObjectId,
        new_parent: &ObjectId,
        new_name: &str,
    ) -> VaultResult<Object> {
        let marker = self.check_key(true).await?;
        refuse_marker_target(marker.as_ref(), id)?;
        refuse_marker_name(new_parent, new_name)?;
        let moved = self.inner.rename(id, new_parent, new_name).await?;
        Self::plaintext_view(moved, true)
    }
}

/// Whether `object` is the vault's marker.
fn is_marker(object: &Object) -> bool {
    matches!(object, Object::Leaf { name, .. } if name == MARKER_NAME)
}

/// `id`'s path components, split on `/` and `\` (a separator for the fs origin on Windows),
/// with empty and `.` ones dropped, so every spelling of one path (`.nimbus-crypt`,
/// `/.nimbus-crypt`, `./.nimbus-crypt`, `.\.nimbus-crypt`) yields the same components. The
/// same rules apply on every platform. A component made only of dots and spaces other than `.`
/// itself (`..`, `...`, `.. `) is refused outright: resolving `..` would let `a/../.nimbus-crypt`
/// name the marker, and Windows strips trailing dots and spaces, so the others can mean `..`
/// too. Refusing them here keeps the marker safe whatever the inner origin does with them.
fn components(id: &str) -> VaultResult<Vec<&str>> {
    let mut parts = Vec::new();
    for part in id.split(['/', '\\']) {
        match part {
            "" | "." => {}
            part if part.chars().all(|c| c == '.' || c == ' ') => {
                return Err(VaultError::InvalidId(id.to_string()));
            }
            part => parts.push(part),
        }
    }
    Ok(parts)
}

/// Whether a path component names the marker on some filesystem: case-insensitively (macOS
/// and Windows by default), and with trailing dots and spaces dropped (Windows strips them).
fn names_marker(component: &str) -> bool {
    component
        .trim_end_matches(['.', ' '])
        .eq_ignore_ascii_case(MARKER_NAME)
}

/// Whether the path made of `parts` is the marker: exactly one component, naming it.
fn is_marker_path(parts: &[&str]) -> bool {
    matches!(parts, [only] if names_marker(only))
}

/// The error for anything that would replace, move or delete the marker.
fn reserved() -> VaultError {
    VaultError::Reserved(format!(
        "{MARKER_NAME} is reserved by nimbus-crypt: it records the vault's key"
    ))
}

/// Refuses to create or move anything to the marker's place: `name` under `parent`, when that
/// comes to the marker's path at the root.
fn refuse_marker_name(parent: &ObjectId, name: &str) -> VaultResult<()> {
    let mut parts = components(parent.as_str())?;
    parts.extend(components(name)?);
    match is_marker_path(&parts) {
        true => Err(reserved()),
        false => Ok(()),
    }
}

/// Refuses to write to, move or delete the marker itself: the id the marker was verified
/// under, or any spelling of its path (see [`components`] and [`names_marker`]).
fn refuse_marker_target(marker: Option<&ObjectId>, id: &ObjectId) -> VaultResult<()> {
    match marker == Some(id) || is_marker_path(&components(id.as_str())?) {
        true => Err(reserved()),
        false => Ok(()),
    }
}

/// Sets a leaf's size, keeping the rest of its metadata.
fn set_size(object: &mut Object, size: u64) {
    if let Object::Leaf { meta, .. } = object {
        meta.size = Some(size);
    }
}

/// `bytes` as a one-item stream.
fn one_chunk(bytes: Vec<u8>) -> ByteStream {
    futures::stream::iter([Ok(Bytes::from(bytes))]).boxed()
}

#[cfg(test)]
#[path = "tests/crypt.rs"]
mod tests;
