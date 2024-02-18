use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        Request, StatusCode,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use tokio::{runtime::Handle, task::spawn_blocking};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Serve a CPU-bound load.
///
/// This is intentionally not an async sleep!
/// It's simulating a large CPU-bound computation.
fn serve_sleepy() -> impl IntoResponse {
    std::thread::sleep(Duration::from_secs(5));
    const IMAGE: &[u8] = include_bytes!("f32.png");
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-cache")],
        IMAGE,
    )
}

fn serve_main() -> impl IntoResponse {
    const CONTENT: &str = include_str!("index.html");
    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, "text/html; charset=utf-8"),
            (CACHE_CONTROL, "no-cache"),
        ],
        CONTENT,
    )
}

fn serve_nonblocking(handle: Handle) -> Router {
    // Handlers must be Clone + Send + 'static
    Router::new()
        .route(
            "/",
            get({
                let handle = handle.clone();
                || async move {
                    handle
                        .spawn_blocking(serve_main)
                        .await
                        .expect("failed to join worker thread")
                }
            }),
        )
        .route(
            "/stateless/:image",
            get({
                let handle = handle.clone();
                |_: Path<String>| async move {
                    handle
                        .spawn_blocking(serve_sleepy)
                        .await
                        .expect("failed to join worker thread")
                }
            }),
        )
        .route(
            "/stateful/:image",
            get({
                let handle = handle.clone();
                |_: State<String>, _: Path<String>| async move {
                    handle
                        .spawn_blocking(serve_sleepy)
                        .await
                        .expect("failed to join worker thread")
                }
            }),
        )
        .with_state("nothing".to_owned())
}

fn serve_spawning(handle: Handle) -> Router {
    // Handlers must be Clone + Send + 'static
    Router::new()
        .route(
            "/",
            get({
                let handle = handle.clone();
                || async move {
                    handle
                        .spawn(async { serve_main() })
                        .await
                        .expect("failed to join worker thread")
                }
            }),
        )
        .route(
            "/stateless/:image",
            get({
                let handle = handle.clone();
                |_: Path<String>| async move {
                    handle
                        .spawn(async { serve_sleepy() })
                        .await
                        .expect("failed to join worker thread")
                }
            }),
        )
        .route(
            "/stateful/:image",
            get({
                let handle = handle.clone();
                |_: State<String>, _: Path<String>| async move {
                    handle
                        .spawn(async { serve_sleepy() })
                        .await
                        .expect("failed to join worker thread")
                }
            }),
        )
        .with_state("nothing".to_owned())
}

fn serve_blocking() -> Router {
    Router::new()
        .route("/", get(|| async { serve_main() }))
        .route("/stateless/:image", get(|| async { serve_sleepy() }))
        .route(
            "/stateful/:image",
            get(|_: State<String>| async { serve_sleepy() }),
        )
        .with_state("nothing".to_owned())
}

fn main() {
    let web_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("could not construct Tokio runtime");
    let h = web_rt.handle().clone();

    // Tracing config, from the Axum example:
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // axum logs rejections from built-in extractors with the `axum::rejection`
                // target, at `TRACE` level. `axum::rejection=trace` enables showing those events
                "slow_axum=debug,tower_http=debug,axum::rejection=trace".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let trace = TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
        // Log the path and query string
        let query = request
            .uri()
            .path_and_query()
            .map(|v| v.as_str())
            .unwrap_or("");
        tracing::info_span!(
            "http_request",
            method = ?request.method(),
            query,
        )
    });

    let server = async move {
        // let app = serve_blocking().layer(trace);
        // let app = serve_spawning(h).layer(trace);
        let app = serve_nonblocking(h).layer(trace);
        const ADDR: &str = "0.0.0.0:3000";
        let listener = tokio::net::TcpListener::bind(ADDR).await?;
        tracing::info!("listening at {:?}", ADDR);
        axum::serve(listener, app).await
    };
    web_rt.block_on(server).expect("server terminated");
}
