//! A ranger-style terminal file manager over nimbus vaults: a list of registered vaults, and
//! inside each one a browsable object tree backed by any `nimbus-vault` origin.
//!
//! Every operation goes through `nimbus_core::App`, so this frontend and `nimbus-cli` do the
//! same things by the same code path. The `:` command line's grammar *is* [`event::AppEvent`],
//! which derives `clap::Subcommand` — so commands, keybindings and the help overlay all come
//! from one definition. See `README.md` for the key reference.

use crate::app::App;

pub mod app;
pub mod command;
pub mod event;
pub mod opener;
pub mod ui;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let result = App::new().run(terminal).await;
    ratatui::restore();
    result
}
