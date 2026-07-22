use std::path::PathBuf;

use ratatui::{Terminal, backend::Backend};

pub mod app;
pub mod builder;
pub mod event;
pub mod ui;

pub use app::App;
pub use builder::OriginKind;

/// Runs the vault-creator wizard against an already-initialized `terminal`. The caller owns
/// terminal setup/teardown (`ratatui::init`/`restore` or equivalent), which is what makes this
/// embeddable from another app (`nimbus-cli`, `nimbus-tui`) that's already driving its own
/// terminal, as well as runnable standalone.
///
/// Returns `Some(path)` — the path a `vault.toml` was written to — if the wizard was completed,
/// or `None` if the user cancelled.
pub fn run<B: Backend>(terminal: &mut Terminal<B>) -> color_eyre::Result<Option<PathBuf>>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    App::new().run(terminal)
}

#[cfg(test)]
mod test;
