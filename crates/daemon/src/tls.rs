//! Serving HTTPS: loading the certificate, reloading it on SIGHUP, and the TLS serve loop. The
//! shutdown rule it follows is [`crate::server`]'s.
//!
//! Everything runs on rustls with the **ring** provider, the one `reqwest` (and so the http
//! origin) already uses. axum-server is built with `tls-rustls-no-provider`, so it has no
//! provider of its own, and its config loading calls `ServerConfig::builder()`, which uses the
//! process-wide default. [`install_crypto_provider`] sets that default once at startup.

use anyhow::{Context, Result};
use axum::Router;
use axum_server::{Handle, tls_rustls::RustlsConfig};
use std::{future::Future, time::Duration};
use tracing::{info, warn};

use crate::config::TlsConfig;

/// Installs ring as rustls's process-wide crypto provider. Safe to call more than once: a
/// provider that is already installed is left in place.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// An HTTPS server's certificate, loaded and ready to serve.
pub struct Https {
    /// The live rustls config, which a reload swaps the certificate inside.
    config: RustlsConfig,
    /// Where the certificate and key are read from, at load and on every reload.
    tls: TlsConfig,
}

impl Https {
    /// Loads `tls`'s certificate and key. A missing or unreadable file, a file with no
    /// certificate or key in it, or a key that doesn't match the certificate is an error naming
    /// the paths, so a bad setup fails at startup rather than on the first connection.
    pub async fn load(tls: &TlsConfig) -> Result<Self> {
        install_crypto_provider();
        let (cert, key) = read_pem(tls)?;
        let config = RustlsConfig::from_pem(cert, key)
            .await
            .with_context(|| loading(tls))?;
        Ok(Https {
            config,
            tls: tls.clone(),
        })
    }

    /// Serves `router` over HTTPS on `listener` until `shutdown` resolves, then stops accepting
    /// and gives in-flight requests `grace` to finish. On Unix, SIGHUP reloads the certificate
    /// meanwhile (see [`reload`]).
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        router: Router,
        shutdown: impl Future<Output = ()> + Send + 'static,
        grace: Duration,
    ) -> std::io::Result<()> {
        #[cfg(unix)]
        reload_on_sighup(self.config.clone(), self.tls.clone())?;
        info!(
            cert = %self.tls.cert_file.display(),
            "listening on https://{}",
            listener.local_addr()?
        );
        let handle = Handle::new();
        let stopper = handle.clone();
        tokio::spawn(async move {
            shutdown.await;
            stopper.graceful_shutdown(Some(grace));
        });
        axum_server::from_tcp_rustls(listener.into_std()?, self.config)
            .handle(handle)
            .serve(router.into_make_service())
            .await
    }

    /// The live rustls config, for tests that reload it while the server runs.
    #[cfg(test)]
    pub(crate) fn rustls_config(&self) -> RustlsConfig {
        self.config.clone()
    }
}

/// Reads `tls`'s certificate chain and key, naming the file in any error.
fn read_pem(tls: &TlsConfig) -> Result<(Vec<u8>, Vec<u8>)> {
    let cert = std::fs::read(&tls.cert_file)
        .with_context(|| format!("reading TLS certificate {}", tls.cert_file.display()))?;
    let key = std::fs::read(&tls.key_file)
        .with_context(|| format!("reading TLS private key {}", tls.key_file.display()))?;
    Ok((cert, key))
}

/// The context for a certificate and key that were read but didn't load, naming both.
fn loading(tls: &TlsConfig) -> String {
    format!(
        "loading TLS certificate {} with key {}",
        tls.cert_file.display(),
        tls.key_file.display()
    )
}

/// Re-reads `tls`'s files into `config`. New connections get the new certificate; open ones
/// keep the one they started with. On any error the old certificate stays in place, since the
/// swap only happens after the new one has loaded.
pub async fn reload(config: &RustlsConfig, tls: &TlsConfig) -> Result<()> {
    let (cert, key) = read_pem(tls)?;
    config
        .reload_from_pem(cert, key)
        .await
        .with_context(|| loading(tls))
}

/// Reloads the certificate every time the process gets SIGHUP, the conventional "re-read your
/// files" signal, so a renewed certificate takes effect without a restart. A failed reload is
/// logged, and the daemon keeps serving the certificate it had.
#[cfg(unix)]
fn reload_on_sighup(config: RustlsConfig, tls: TlsConfig) -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut hangups = signal(SignalKind::hangup())
        .map_err(|e| std::io::Error::new(e.kind(), format!("listening for SIGHUP: {e}")))?;
    tokio::spawn(async move {
        while hangups.recv().await.is_some() {
            match reload(&config, &tls).await {
                Ok(()) => info!("reloaded TLS certificate {}", tls.cert_file.display()),
                Err(e) => warn!("TLS reload failed, still serving the old certificate: {e:#}"),
            }
        }
    });
    Ok(())
}

#[cfg(test)]
#[path = "tests/tls.rs"]
mod tests;
