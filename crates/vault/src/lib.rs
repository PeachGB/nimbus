#![warn(missing_docs)]
//! A generic sync abstraction: a tree of [`object::Object`]s (a [`vault::Vault`]) whose
//! actual storage lives behind a pluggable [`origin::Origin`] — a local directory, an HTTP
//! API, a shell command, or another vault. Syncing "a folder on disk" and "objects behind a
//! REST API" run through the exact same code path, because both are just implementations of
//! one `Origin` trait.
//!
//! # The model
//!
//! - [`object::Object`] — a node in the tree: `Leaf` (has content), `Branch` (has children),
//!   or `Root`. Objects only ever carry metadata (name, id, size, content type, modified time,
//!   plus a free-form `extra` map) — never raw bytes — so listing a tree never materializes
//!   its contents into memory.
//! - [`object::ObjectId`] — a newtype around `String`, opaque and origin-specific (a relative
//!   path for the filesystem origin, an arbitrary id for HTTP/command origins).
//! - [`origin::Origin`] — the trait every backend implements: `fetch`, `list`, `get`, `put`,
//!   `send`, `delete`. `fetch`/`send` are streaming
//!   ([`origin::ByteStream`] = `BoxStream<'static, VaultResult<Bytes>>`) — content moves in
//!   chunks, it's never buffered whole into RAM. `put(object, destination)` writes `object`
//!   under `destination` and returns the `Object` as it now exists at the origin —
//!   implementations may, but aren't required to, rename `object` in place, so callers should
//!   always act on the returned value rather than assuming `object` itself changed.
//! - [`vault::Vault`] — owns one `Origin` plus an in-memory metadata cache. `get`/`list`
//!   populate the cache; `list` always re-hits the origin (it's the source of truth) while
//!   refreshing the cache. `find` resolves a `/`-separated path to an `ObjectId` by walking
//!   the tree one `list` call per component. `put` only updates the cache on success, and
//!   caches the `Object` `put` returned. `pull`/`push` recursively sync a subtree between the
//!   vault's own origin and any other `&dyn Origin`, threading `put`'s return value through to
//!   the following `send`.
//!
//! Four built-in origins ship in this crate:
//!
//! - [`origin::fs::OriginFileSystem`] (`type = "fs"`) — a directory on disk, via `tokio::fs`.
//! - [`origin::http::OriginHTTP`] (`type = "http"`) — any REST-ish API, with a
//!   `{id}`-templated URL per operation.
//! - [`origin::command::OriginCommand`] (`type = "command"`) — a shell command per operation;
//!   the universal escape hatch for anything that isn't a plain filesystem or HTTP API.
//! - [`origin::vault::OriginVault`] (`type = "vault"`) — another [`vault::Vault`], wrapped so
//!   it can act as an origin in its own right, letting two vaults sync directly with `push`/
//!   `pull` without either side needing to know the other is a `Vault` rather than a plain
//!   origin.
//!
//! # Configuration
//!
//! A vault is fully described by a TOML file, deserialized into [`config::VaultConfig`] /
//! [`config::OriginConfig`]:
//!
//! ```toml
//! # vault.toml — backed by a local directory
//! name = "my-vault"
//!
//! [origin_config]
//! type = "fs"
//! root = "/srv/data"
//! ```
//!
//! ```no_run
//! use nimbus_vault::vault::Vault;
//!
//! # async fn example() -> nimbus_vault::VaultResult<()> {
//! let vault = Vault::new("vault.toml".into())?;
//! let root = vault.find("/".into()).await?;
//! let children = vault.list(root).await?;
//! # Ok(())
//! # }
//! ```
//!
//! An `origin_config` can also be built standalone, without a `name`/`root_id`/`Vault`
//! wrapper, via [`config::OriginConfig::from_file`] — useful for tooling that talks to an
//! origin directly, or for building the `remote` argument to
//! [`vault::Vault::pull`]/[`vault::Vault::push`].
//!
//! # Quick start
//!
//! A fully self-contained, end-to-end example — writing a vault config, creating an object,
//! reading it back, and listing its parent:
//!
//! ```
//! use std::fs;
//! use futures::StreamExt;
//! use nimbus_vault::{object::{Metadata, Object, ObjectId}, vault::Vault};
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let dir = tempfile::tempdir()?;
//! let data_dir = dir.path().join("data");
//! fs::create_dir_all(&data_dir)?;
//!
//! let config_path = dir.path().join("vault.toml");
//! fs::write(
//!     &config_path,
//!     format!(
//!         "name = \"my-vault\"\n\n[origin_config]\ntype = \"fs\"\nroot = \"{}\"\n",
//!         data_dir.display(),
//!     ),
//! )?;
//!
//! let vault = Vault::new(config_path)?;
//!
//! // create an object
//! let mut notes = Object::Leaf {
//!     name: "notes.txt".to_string(),
//!     id: ObjectId::from("notes.txt"),
//!     meta: Metadata::new(),
//! };
//! vault.put(&mut notes, &ObjectId::from("")).await?;
//! vault.send(&notes, Box::pin(futures::stream::once(async {
//!     Ok(bytes::Bytes::from_static(b"hello vault"))
//! }))).await?;
//!
//! // read it back
//! let mut stream = vault.fetch("notes.txt").await?;
//! let mut bytes = Vec::new();
//! while let Some(chunk) = stream.next().await {
//!     bytes.extend_from_slice(&chunk?);
//! }
//! assert_eq!(&bytes, b"hello vault");
//!
//! // list the vault's root
//! let names: Vec<String> = vault.list("").await?.iter().map(Object::get_name).collect();
//! assert_eq!(names, vec!["notes.txt".to_string()]);
//! # Ok(())
//! # }
//! ```
//!
//! # Syncing
//!
//! [`vault::Vault::pull`]/[`vault::Vault::push`] sync a subtree between the vault's own origin
//! and any other `&dyn Origin` — another config-defined origin, or another vault wrapped in
//! [`origin::vault::OriginVault`]:
//!
//! ```no_run
//! use nimbus_vault::{config::OriginConfig, vault::Vault};
//!
//! # async fn example() -> nimbus_vault::VaultResult<()> {
//! let vault = Vault::new("vault.toml".into())?;
//! let remote = OriginConfig::from_file("remote.toml".into())?;
//! let root = vault.find("".into()).await?;
//!
//! vault.pull(&root, remote.as_ref()).await?; // bring local up to date with remote
//! vault.push(&root, remote.as_ref()).await?; // push local changes back out
//! # Ok(())
//! # }
//! ```
//!
//! # Errors
//!
//! Every fallible operation in this crate returns [`VaultResult`], an alias for
//! `Result<T, `[`error::VaultError`]`>`.
//!
//! # Constants
//!
//! The `{placeholder}` keys, `OriginConfig::Command` field names, and root id/name
//! conventions shared across [`object`] and the [`origin::command`]/[`origin::http`]
//! implementations are defined once here (e.g. [`ROOT_ID`], [`PLACEHOLDER_ID`],
//! [`FETCH_CMD_FIELD`]) rather than being hardcoded independently in each module.

/// On-disk (TOML) configuration for a [`vault::Vault`] and the [`origin::Origin`] it wraps.
pub mod config;
/// The crate's error type, [`error::VaultError`], and the [`VaultResult`] alias built from it.
pub mod error;
/// The tree node type, [`object::Object`], along with [`object::ObjectId`] and
/// [`object::Metadata`].
pub mod object;
/// The [`origin::Origin`] trait and its built-in implementations (`fs`, `http`, `command`,
/// `vault`).
pub mod origin;
/// [`vault::Vault`], the tree-like view over an `Origin` that ties everything together.
pub mod vault;

/// The result type used throughout this crate, shorthand for `Result<T, `[`error::VaultError`]`>`.
pub type VaultResult<T> = Result<T, error::VaultError>;

/// Conventional root [`object::ObjectId`] value, used by [`object::ObjectId::default`] and
/// [`object::Object::root`].
pub const ROOT_ID: &str = "/";
/// Display name reported by [`object::Object::get_name`] for the `Root` variant, which has no
/// real name of its own.
pub const ROOT_NAME: &str = "##ROOT##";

/// Template placeholder key substituted with an object's id in
/// [`origin::command::OriginCommand`]/[`origin::http::OriginHTTP`] templates (rendered as
/// `{id}`).
pub const PLACEHOLDER_ID: &str = "id";
/// Template placeholder key substituted with an object's name in
/// [`origin::command::OriginCommand`] templates (rendered as `{name}`).
pub const PLACEHOLDER_NAME: &str = "name";
/// Template placeholder key substituted with an object's payload size in
/// [`origin::command::OriginCommand`] templates (rendered as `{size}`).
pub const PLACEHOLDER_SIZE: &str = "size";
/// Template placeholder key substituted with an object's content type in
/// [`origin::command::OriginCommand`] templates (rendered as `{content_type}`).
pub const PLACEHOLDER_CONTENT_TYPE: &str = "content_type";
/// Template placeholder key substituted with an object's modified timestamp in
/// [`origin::command::OriginCommand`] templates (rendered as `{modified}`).
pub const PLACEHOLDER_MODIFIED: &str = "modified";
/// `extra_vars` key [`origin::command::OriginCommand`]'s `put` uses to expose its
/// `destination` argument to the `put_cmd` template (rendered as `{destination}`), when not
/// already set.
pub const PLACEHOLDER_DESTINATION: &str = "destination";
/// Fallback content type substituted into command templates when an object's metadata doesn't
/// specify one.
pub const UNKNOWN_CONTENT_TYPE: &str = "unknown";

/// [`config::OriginConfig::Command`]/[`origin::command::CmdType`] field name for the fetch
/// command template.
pub const FETCH_CMD_FIELD: &str = "fetch_cmd";
/// [`config::OriginConfig::Command`]/[`origin::command::CmdType`] field name for the list
/// command template.
pub const LIST_CMD_FIELD: &str = "list_cmd";
/// [`config::OriginConfig::Command`]/[`origin::command::CmdType`] field name for the get
/// command template.
pub const GET_CMD_FIELD: &str = "get_cmd";
/// [`config::OriginConfig::Command`]/[`origin::command::CmdType`] field name for the put
/// command template.
pub const PUT_CMD_FIELD: &str = "put_cmd";
/// [`config::OriginConfig::Command`]/[`origin::command::CmdType`] field name for the send
/// command template.
pub const SEND_CMD_FIELD: &str = "send_cmd";
/// [`config::OriginConfig::Command`]/[`origin::command::CmdType`] field name for the delete
/// command template.
pub const DELETE_CMD_FIELD: &str = "delete_cmd";
