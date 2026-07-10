use futures::stream::BoxStream;

use crate::{
    VaultResult,
    object::{Object, ObjectId},
};
use bytes::Bytes;

/// [`command::OriginCommand`]: an origin backed by a shell command per operation.
pub mod command;
/// [`fs::OriginFileSystem`]: an origin backed by a directory on disk.
pub mod fs;
/// [`http::OriginHTTP`]: an origin backed by a REST-ish HTTP API.
pub mod http;
/// [`vault::OriginVault`]: an origin backed by another `Vault`.
pub mod vault;

/// A boxed, pinned stream of `VaultResult<Bytes>`, used for both reading (`fetch`) and
/// writing (`send`) object payloads without buffering the whole thing in memory.
pub type ByteStream = BoxStream<'static, VaultResult<Bytes>>;

/// The trait every storage backend implements. A `Vault` holds one `Origin` and delegates all
/// of its own I/O to it; `push`/`pull` operate between two arbitrary `Origin`s.
///
/// # Examples
///
/// Four implementations ship with this crate — [`fs::OriginFileSystem`], [`http::OriginHTTP`],
/// [`command::OriginCommand`], and [`vault::OriginVault`] — so you rarely need to implement
/// `Origin` yourself. A minimal custom origin (e.g. backed by an in-memory `HashMap`, a
/// database, or an object store SDK) looks like this:
///
/// ```no_run
/// use nimbus_vault::{
///     VaultResult,
///     error::VaultError,
///     object::{Object, ObjectId},
///     origin::{ByteStream, Origin},
/// };
///
/// struct MyOrigin;
///
/// #[async_trait::async_trait]
/// impl Origin for MyOrigin {
///     async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream> {
///         Err(VaultError::NotFound(id.to_string()))
///     }
///     async fn list(&self, _id: &ObjectId) -> VaultResult<Vec<Object>> {
///         Ok(vec![])
///     }
///     async fn get(&self, id: &ObjectId) -> VaultResult<Object> {
///         Err(VaultError::NotFound(id.to_string()))
///     }
///     async fn put(&self, _object: &Object) -> VaultResult<()> {
///         Ok(())
///     }
///     async fn send(&self, _object: &Object, _payload: ByteStream) -> VaultResult<()> {
///         Ok(())
///     }
///     async fn delete(&self, _id: &ObjectId) -> VaultResult<()> {
///         Ok(())
///     }
/// }
/// ```
///
/// Since `Vault`/`push`/`pull` all take `&dyn Origin` (or `Box<dyn Origin>`), a `MyOrigin` can
/// be dropped in anywhere an existing origin is used — for instance as the `remote` argument to
/// [`crate::vault::Vault::pull`].
#[async_trait::async_trait]
pub trait Origin: Send + Sync {
    /// Streams `id`'s payload.
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream>;
    /// Lists `id`'s children (for a `Branch`/`Root` id).
    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>>;
    /// Fetches `id`'s metadata as an `Object`.
    async fn get(&self, id: &ObjectId) -> VaultResult<Object>;
    /// Writes `object`'s metadata (without payload) to the origin.
    async fn put(&self, object: &Object) -> VaultResult<()>;
    /// Streams `payload` to the origin as `object`'s contents.
    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()>;
    /// Deletes `id` from the origin.
    async fn delete(&self, id: &ObjectId) -> VaultResult<()>;
}
