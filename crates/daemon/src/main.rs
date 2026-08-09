mod api;
mod auth;
mod config;
mod error;
mod state;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::{
    config::{Args, Config},
    state::AppState,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let config = Config::resolve(&args)?;
    let state = AppState::load(&config)?;

    let names = state.vault_names();
    info!(
        auth = config.auth.describe(),
        read_only = config.read_only,
        "serving {:?} from {}",
        names,
        config.vaults_path.display()
    );
    if names.is_empty() {
        warn!(
            "no vault configs found in {} — every request will 404",
            config.vaults_path.display()
        );
    }
    if config.auth.is_open() && !config.bind.ip().is_loopback() {
        warn!(
            "listening on {} with no authentication configured — anyone who can reach this \
             port can read{} these vaults",
            config.bind,
            if config.read_only { "" } else { " and write" }
        );
    }

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    info!("listening on http://{}", listener.local_addr()?);

    api::serve(listener, api::router(state))
        .await
        .context("serving")
}
