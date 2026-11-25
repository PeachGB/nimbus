//! Serving plain HTTP, and the shutdown rule HTTP and HTTPS share: once shutdown starts, stop
//! accepting and give in-flight requests a bounded grace period. The HTTPS side lives in
//! [`crate::tls`]; handlers and routing live in [`crate::api`].

use axum::Router;
use std::{future::Future, time::Duration};
use tokio::sync::watch;

/// How long in-flight requests get to finish once shutdown starts.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Resolves when the process gets Ctrl-C. Failing to listen for it is ignored, the way
/// `axum::serve` examples do: the daemon then just runs until killed.
pub async fn ctrl_c() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Serves `router` over plain HTTP on `listener` until `shutdown` resolves, then stops
/// accepting and gives in-flight requests `grace` to finish. A request still running after
/// that is dropped, so a stuck client can't keep the daemon from exiting.
pub async fn serve_http(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
    grace: Duration,
) -> std::io::Result<()> {
    // One signal, two listeners: axum's graceful shutdown and the deadline after it.
    let (stop, stopping) = watch::channel(false);
    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        shutdown.await;
        let _ = stop.send(true);
    });
    tokio::select! {
        result = server => result,
        () = deadline(stopping, grace) => Ok(()),
    }
}

/// Resolves `grace` after shutdown is signalled on `stopping`.
async fn deadline(mut stopping: watch::Receiver<bool>, grace: Duration) {
    // An error means the sender was dropped unsent, which only happens when axum is done with
    // the shutdown future, so the server is finishing either way.
    let _ = stopping.wait_for(|stop| *stop).await;
    tokio::time::sleep(grace).await;
}

#[cfg(test)]
#[path = "tests/server.rs"]
mod tests;
