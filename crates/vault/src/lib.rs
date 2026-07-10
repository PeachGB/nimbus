pub mod config;
pub mod error;
pub mod object;
pub mod origin;
pub mod vault;

pub type VaultResult<T> = Result<T, error::VaultError>;
