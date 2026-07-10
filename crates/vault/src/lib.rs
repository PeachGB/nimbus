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
//!   chunks, it's never buffered whole into RAM.
//! - [`vault::Vault`] — owns one `Origin` plus an in-memory metadata cache. `get`/`list`
//!   populate the cache; `list` always re-hits the origin (it's the source of truth) while
//!   refreshing the cache. `find` resolves a `/`-separated path to an `ObjectId` by walking
//!   the tree one `list` call per component. `pull`/`push` recursively sync a subtree between
//!   the vault's own origin and any other `&dyn Origin`.
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
//! let notes = Object::Leaf {
//!     name: "notes.txt".to_string(),
//!     id: ObjectId::from("notes.txt"),
//!     meta: Metadata::new(),
//! };
//! vault.put(&notes).await?;
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
