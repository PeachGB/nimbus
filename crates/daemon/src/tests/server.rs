use super::*;
use axum::routing::get;
use std::sync::Arc;
use tokio::{
    sync::{Notify, oneshot},
    task::JoinHandle,
    time::Instant,
};

/// Short enough to keep the suite fast, long enough to tell "waited" from "didn't".
const GRACE: Duration = Duration::from_millis(200);

/// A plain-HTTP server under test, with one route, `/slow`, that answers `done` after a delay.
struct SlowServer {
    base: String,
    /// Tells the server to shut down.
    stop: oneshot::Sender<()>,
    /// Signalled once a request has reached the handler, so shutdown can start mid-request.
    arrived: Arc<Notify>,
    server: JoinHandle<std::io::Result<()>>,
}

async fn serve_slow(delay: Duration) -> SlowServer {
    let arrived = Arc::new(Notify::new());
    let handler_arrived = arrived.clone();
    let router = Router::new().route(
        "/slow",
        get(move || {
            let arrived = handler_arrived.clone();
            async move {
                arrived.notify_one();
                tokio::time::sleep(delay).await;
                "done"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let shutdown = async {
        let _ = stopped.await;
    };
    let server = tokio::spawn(serve_http(listener, router, shutdown, GRACE));
    SlowServer {
        base,
        stop,
        arrived,
        server,
    }
}

/// Starts a request to `/slow` and returns once the handler is running it.
async fn request_in_flight(server: &SlowServer) -> JoinHandle<reqwest::Result<String>> {
    let url = format!("{}/slow", server.base);
    let request = tokio::spawn(async move { reqwest::get(url).await?.text().await });
    server.arrived.notified().await;
    request
}

#[tokio::test]
async fn http_shutdown_waits_for_in_flight_requests_up_to_the_grace_period() {
    let server = serve_slow(Duration::from_secs(30)).await;
    let _request = request_in_flight(&server).await;

    let started = Instant::now();
    server.stop.send(()).unwrap();
    server.server.await.unwrap().unwrap();
    let waited = started.elapsed();

    assert!(
        waited >= GRACE,
        "returned after {waited:?}, before the grace period"
    );
    assert!(
        waited < GRACE + Duration::from_secs(2),
        "returned after {waited:?}, long past the grace period"
    );
}

#[tokio::test]
async fn http_shutdown_lets_a_quick_request_finish() {
    let server = serve_slow(Duration::from_millis(50)).await;
    let request = request_in_flight(&server).await;

    server.stop.send(()).unwrap();

    assert_eq!(request.await.unwrap().unwrap(), "done");
    server.server.await.unwrap().unwrap();
}
