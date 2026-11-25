mod api;
mod auth;
mod config;
mod error;
mod server;
mod state;
mod tls;

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

    tls::install_crypto_provider();
    let args = Args::parse();
    let config = Config::resolve(&args)?;
    // Before anything is bound: an unsafe combination (a bearer token over plain HTTP on a
    // reachable address) stops here.
    for warning in config.validate()? {
        warn!("{warning}");
    }
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
    // Loaded before binding, so a missing or mismatched certificate fails without ever
    // listening.
    let https = match &config.tls {
        Some(tls) => Some(tls::Https::load(tls).await?),
        None => None,
    };

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    let router = api::router(state);

    match https {
        Some(https) => {
            https
                .serve(listener, router, server::ctrl_c(), server::SHUTDOWN_GRACE)
                .await
        }
        None => {
            info!("listening on http://{}", listener.local_addr()?);
            server::serve_http(listener, router, server::ctrl_c(), server::SHUTDOWN_GRACE).await
        }
    }
    .context("serving")
}
