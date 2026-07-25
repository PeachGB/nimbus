//! Core session/vault-management logic shared by nimbus's frontends (`nimbus-cli` and
//! `nimbus-tui`). Owns the `App` (registered vaults, current vault/cwd, the local staging
//! vault) and the on-disk app config it's built from; frontends drive it through `App`'s
//! methods and are responsible for their own input/output loop.
//!
//! `App` exposes two overlapping surfaces: the command methods (`cd`, `cp`, `delete`, ...),
//! which mirror what a user types, and data-returning ones (`vault_names`, `list_cwd`,
//! `fetch_object_bytes`, `write_object_bytes`) for a frontend that renders rather than
//! prints. There is no terminal or UI code here.

pub mod app;
pub mod config;
