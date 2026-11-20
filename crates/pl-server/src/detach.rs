//! Writing requests run to completion even if the client aborts (FINDINGS B10).
//!
//! hyper/axum drop a handler's future as soon as the client closes the connection
//! (timeout, leaving the page, proxy timeout). A writing handler consists of several,
//! individually committed steps (e.g. PATCH: save → workflows "document updated" →
//! WebSocket → search index; bulk change: commit → enqueue follow-up; upload; notes). If the
//! future was dropped after saving, workflow, search index and notification were missing permanently.
//! Django always finishes a request.
//!
//! Therefore the handler body of every request with an unsafe method (everything except GET, HEAD,
//! OPTIONS, TRACE) runs as its own Tokio task; the request only waits for its result. If the
//! client closes, only the waiting is aborted – the task keeps working until the response (which
//! nobody reads anymore). This applies systematically to all writing routes (Rust handlers and own
//! middleware below), without touching each handler individually. Reading requests may still be
//! aborted (saves work, no side effects).
//!
//! Side effect: a panic in the handler becomes `500 Server Error` instead of a connection abort.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Middleware: run unsafe methods detached (see module docs).
pub async fn middleware(req: Request, next: Next) -> Response {
    if !detached(req.method()) {
        return next.run(req).await;
    }
    let method = req.method().clone();
    let path = crate::frontline::log_path(req.uri().path());
    match tokio::spawn(next.run(req)).await {
        Ok(resp) => resp,
        Err(e) => {
            let what = if e.is_panic() { "panicked" } else { "was cancelled" };
            pl_core::ApiError::internal(format!("request handler {what}: {method} {path}")).into_response()
        }
    }
}

/// Methods whose handlers must not be aborted in the middle of their work.
pub fn detached(method: &axum::http::Method) -> bool {
    !method.is_safe()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Method;
    use axum::routing::{get, post};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tower::ServiceExt;

    #[test]
    fn methods() {
        for m in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(detached(&m), "{m}");
        }
        for m in [Method::GET, Method::HEAD, Method::OPTIONS, Method::TRACE] {
            assert!(!detached(&m), "{m}");
        }
    }

    async fn boom() -> &'static str {
        panic!("boom")
    }

    fn app(done: Arc<AtomicBool>) -> Router {
        let d1 = done.clone();
        let d2 = done;
        Router::new()
            .route(
                "/w",
                post(move || {
                    let d = d1.clone();
                    async move {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        d.store(true, Ordering::SeqCst);
                        "ok"
                    }
                }),
            )
            .route(
                "/r",
                get(move || {
                    let d = d2.clone();
                    async move {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        d.store(true, Ordering::SeqCst);
                        "ok"
                    }
                }),
            )
            .route("/p", post(boom))
            .layer(axum::middleware::from_fn(middleware))
    }

    /// Client aborts after 20 ms (future dropped): the writing handler still runs to
    /// completion, the reading one does not.
    #[tokio::test]
    async fn write_survives_client_abort_read_does_not() {
        let done = Arc::new(AtomicBool::new(false));
        let req = Request::builder().method(Method::POST).uri("/w").body(Body::empty()).unwrap();
        let r = tokio::time::timeout(Duration::from_millis(20), app(done.clone()).oneshot(req)).await;
        assert!(r.is_err(), "client gave up");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(done.load(Ordering::SeqCst), "POST handler finished after the client left");

        let done = Arc::new(AtomicBool::new(false));
        let req = Request::builder().method(Method::GET).uri("/r").body(Body::empty()).unwrap();
        let r = tokio::time::timeout(Duration::from_millis(20), app(done.clone()).oneshot(req)).await;
        assert!(r.is_err());
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!done.load(Ordering::SeqCst), "GET handler was cancelled");
    }

    #[tokio::test]
    async fn panic_becomes_500() {
        let req = Request::builder().method(Method::POST).uri("/p").body(Body::empty()).unwrap();
        let resp = app(Arc::new(AtomicBool::new(false))).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }
}
