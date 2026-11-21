//! HTTP server loop (instead of `axum::serve`) so hyper can preserve header-name
//! case end to end and connections support `Upgrade` (WebSocket tunnelling).

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tower::ServiceExt;

/// Serve `app` on `listener` until `shutdown` resolves; then stop accepting and give
/// open connections up to 10 s to finish.
pub async fn serve(listener: TcpListener, app: Router, shutdown: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    let mut builder = Builder::new(TokioExecutor::new());
    builder
        .http1()
        .preserve_header_case(true)
        .title_case_headers(false)
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(60));
    let graceful = GracefulShutdown::new();
    tokio::pin!(shutdown);
    // C2: FORCE_SCRIPT_NAME like granian `GRANIAN_URL_PATH_PREFIX` + Django `path_info`.
    let script_name = script_name_from_env();

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(v) => v,
                    Err(e) => {
                        // e.g. EMFILE; back off briefly instead of spinning.
                        tracing::warn!(error = %e, "accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let _ = stream.set_nodelay(true);
                let app = app.clone();
                let sn = script_name.clone();
                let svc = hyper::service::service_fn(move |mut req: hyper::Request<Incoming>| {
                    req.extensions_mut().insert(ConnectInfo::<SocketAddr>(peer));
                    if let Some(u) = sn.as_deref().and_then(|sn| strip_script_name(req.uri(), sn)) {
                        *req.uri_mut() = u;
                    }
                    app.clone().oneshot(req.map(Body::new))
                });
                let conn = builder.serve_connection_with_upgrades(TokioIo::new(stream), svc);
                let conn = graceful.watch(conn.into_owned());
                tokio::spawn(async move {
                    if let Err(e) = conn.await {
                        tracing::debug!(error = %e, "connection ended with error");
                    }
                });
            }
            () = &mut shutdown => break,
        }
    }
    drop(listener);
    tokio::select! {
        () = graceful.shutdown() => {}
        () = tokio::time::sleep(Duration::from_secs(10)) => {
            tracing::warn!("open connections did not finish within 10 s");
        }
    }
    Ok(())
}

/// `PAPERLESS_FORCE_SCRIPT_NAME` (environment, configuration file, UI) without trailing `/`;
/// `None` if empty.
fn script_name_from_env() -> Option<String> {
    let v = pl_core::config::Env::from_process().get("PAPERLESS_FORCE_SCRIPT_NAME").map(str::to_string)?;
    let v = v.trim_end_matches('/').to_string();
    (!v.is_empty()).then_some(v)
}

/// C2 – incoming paths under `FORCE_SCRIPT_NAME` (upstream: granian `url_path_prefix`, Django
/// `ASGIRequest.path_info = scope["path"].removeprefix(script_name)`): `/<sn>/api/…` → `/api/…`,
/// `/<sn>` → `/`. Paths without the prefix (the proxy has already removed it) stay unchanged, as with
/// Django. Deviation: only whole path segments (`/<sn>x` stays), upstream cuts there too.
pub fn strip_script_name(uri: &hyper::Uri, sn: &str) -> Option<hyper::Uri> {
    let path = uri.path();
    let rest = path.strip_prefix(sn)?;
    let rest = if rest.is_empty() {
        "/"
    } else if rest.starts_with('/') {
        rest
    } else {
        return None;
    };
    let pq = match uri.query() {
        Some(q) => format!("{rest}?{q}"),
        None => rest.to_string(),
    };
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(pq.parse().ok()?);
    hyper::Uri::from_parts(parts).ok()
}

#[cfg(test)]
mod script_name_tests {
    use super::strip_script_name;

    #[test]
    fn strips_whole_segment_prefix() {
        let u = |s: &str| s.parse::<hyper::Uri>().unwrap();
        assert_eq!(strip_script_name(&u("/paperless/api/documents/?page=2"), "/paperless").unwrap(), u("/api/documents/?page=2"));
        assert_eq!(strip_script_name(&u("/paperless"), "/paperless").unwrap(), u("/"));
        assert_eq!(strip_script_name(&u("/paperless/"), "/paperless").unwrap(), u("/"));
        assert!(strip_script_name(&u("/api/documents/"), "/paperless").is_none());
        assert!(strip_script_name(&u("/paperlessx/api/"), "/paperless").is_none());
        assert_eq!(strip_script_name(&u("http://h:8000/p/x"), "/p").unwrap(), u("http://h:8000/x"));
    }
}
