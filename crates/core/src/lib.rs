//! Core session/vault-management logic shared by nimbus's frontends (`nimbus-cli`, and
//! eventually `nimbus-tui`). Owns the `App` (registered vaults, current vault/cwd, the
//! local staging vault) and the on-disk app config it's built from; frontends drive it
//! through `App`'s methods and are responsible for their own input/output loop.

pub mod app;
pub mod config;
