//! WebSocket `/ws/status/` in Rust (events from the Rust worker over the event bus).
//!
//! upstream: src/paperless/consumers.py@v3.4.2 (StatusConsumer: connect/close, `_can_view`,
//! heartbeat), src/paperless/asgi.py@v3.4.2 (AuthMiddlewareStack: session cookie only),
//! src/paperless/urls.py@v3.4.2 (websocket_urlpatterns `ws/status/$`).
//!
//! * Authentication: `django.contrib.auth.get_user` from the session cookie (Channels'
//!   `AuthMiddlewareStack`; no token/basic auth, no remote-user, no CSRF – like upstream).
//!   Not authenticated → `close()` before `accept()` → HTTP 403 with empty body (granian).
//!   **PaperRust addition:** without a session user, a PaperRust device token
//!   (`Authorization: Bearer prt_…`, same check as the API) is accepted too – the desktop app in
//!   mode 2b relays the socket from its client core (pl-sync-client `wsrelay`), which has no
//!   session cookie. Browsers cannot set `Authorization` on a WebSocket, so this opens nothing for
//!   web pages; other schemes (`Token`, `Basic`) stay rejected like upstream.
//! * **PaperRust addition (security review 2026-11-19):** a session-cookie socket whose
//!   `Origin` header does not match the request host (Django `CsrfViewMiddleware` rules: own
//!   scheme+host or `CSRF_TRUSTED_ORIGINS`, i.e. also `PAPERLESS_URL`) is refused with 403
//!   (Cross-Site WebSocket Hijacking). No `Origin` (non-browser clients) is allowed. Upstream has
//!   no origin validator (`AuthMiddlewareStack` only).
//! * Messages: every `pl_queue::WsPayload` of the [`pl_queue::EventBus`] as `json.dumps(event)`
//!   (`WsPayload::to_ws_text`), `status_update`/`document_updated` only if
//!   [`pl_queue::events::visible_to`] the user, `documents_deleted` to everyone;
//!   heartbeat `{"type": "heartbeat"}` every 30 s. Client messages are ignored.

use std::time::Duration;

use axum::Extension;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Request};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use pl_queue::{EventBus, WsPayload};
use futures::SinkExt;
use tokio::sync::broadcast::error::RecvError;

/// Path of the status socket (`BASE_URL` = `/`).
pub const PATH: &str = "/ws/status/";

/// The user of a socket (fixed at connect time, like `scope["user"]`).
#[derive(Debug, Clone)]
pub struct SocketUser {
    pub id: i64,
    pub is_superuser: bool,
    pub groups: Vec<i64>,
}

impl SocketUser {
    pub fn can_see(&self, p: &WsPayload) -> bool {
        pl_queue::events::visible_to(p, self.id, self.is_superuser, &self.groups)
    }
}

/// `scope["user"]` from the session cookie (None = AnonymousUser).
pub async fn session_user(authz: &pl_authz::Authz, headers: &HeaderMap) -> Option<SocketUser> {
    let mut state = pl_authz::SessionState::default();
    match authz.session_user(headers, &mut state).await {
        Ok(Some(u)) if u.is_active => Some(SocketUser { id: u.id, is_superuser: u.is_superuser, groups: u.groups.clone() }),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(error = %e, "websocket session lookup failed");
            None
        }
    }
}

/// `Authorization: Bearer prt_…` (the only non-session credential accepted on the socket).
pub fn device_bearer(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().split_once(' '))
        .is_some_and(|(scheme, key)| scheme.eq_ignore_ascii_case("bearer") && key.trim().starts_with(pl_authz::device::PREFIX))
}

/// Socket user from a PaperRust device token (see module docs); revoked/unknown → None.
pub async fn device_user(authz: &pl_authz::Authz, headers: &HeaderMap) -> Option<SocketUser> {
    if !device_bearer(headers) {
        return None;
    }
    match authz.authenticate(&axum::http::Method::GET, PATH, headers).await {
        Ok(ctx) if ctx.method == Some(pl_authz::AuthMethod::DeviceToken) => {
            ctx.user.filter(|u| u.is_active).map(|u| SocketUser { id: u.id, is_superuser: u.is_superuser, groups: u.groups.clone() })
        }
        Ok(_) => None,
        Err(e) => {
            tracing::debug!(error = %e, "websocket device token rejected");
            None
        }
    }
}

/// Security review 2026-11-19: a browser's `Origin` must match the own address –
/// same rules as Django's `CsrfViewMiddleware._origin_verified` (pl-authz `csrf`):
/// `<scheme>://<get_host()>` or an entry from `CSRF_TRUSTED_ORIGINS` (exact or with `*`).
/// Without `Origin` (non-browser) allowed.
/// CORE-REQUEST: make `pl_authz::csrf::origin_verified` public, then call it here.
pub fn origin_allowed(cfg: &pl_authz::AuthConfig, headers: &HeaderMap) -> bool {
    let Some(origin) = pl_authz::csrf::meta_header(headers, "HTTP_ORIGIN") else {
        return true;
    };
    pl_authz::csrf::origin_verified(cfg, headers, &origin)
}

/// HTTP 403 with empty body (Channels closes before accept; granian answers like this).
fn forbidden() -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::FORBIDDEN;
    r
}

/// Handler for `GET /ws/status/` (mounted only with PgQueue).
pub async fn status_socket(Extension(authz): Extension<pl_authz::Authz>, Extension(bus): Extension<EventBus>, req: Request) -> Response {
    let (mut parts, _body) = req.into_parts();
    let user = match session_user(&authz, &parts.headers).await {
        // Security review 2026-11-19: session cookie only with a matching Origin (CSWSH).
        Some(_) if !origin_allowed(authz.config(), &parts.headers) => {
            tracing::warn!("websocket refused: cross-site Origin");
            return forbidden();
        }
        Some(u) => Some(u),
        // Device token: browsers cannot set `Authorization` on the WebSocket.
        None => device_user(&authz, &parts.headers).await,
    };
    let Some(user) = user else {
        return forbidden();
    };
    let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(u) => u,
        // Not a WebSocket handshake: Django would 404 (no HTTP route for /ws/status/).
        Err(rejection) => return rejection.into_response(),
    };
    // Subscribe before the upgrade completes so no event between accept and loop is lost.
    let rx = bus.subscribe();
    upgrade.on_upgrade(move |socket| run_socket(socket, user, rx))
}

async fn run_socket(mut socket: WebSocket, user: SocketUser, mut rx: tokio::sync::broadcast::Receiver<WsPayload>) {
    let period = Duration::from_secs(pl_queue::events::HEARTBEAT_INTERVAL_SECS);
    let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if socket.send(Message::Text(pl_queue::events::HEARTBEAT_MESSAGE.into())).await.is_err() {
                    break;
                }
            }
            ev = rx.recv() => match ev {
                Ok(p) => {
                    if user.can_see(&p) && socket.send(Message::Text(p.to_ws_text().into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(n)) => tracing::warn!(n, user = user.id, "websocket client lagging, status events dropped"),
                Err(RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                // StatusConsumer has no receive(): client messages are ignored.
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    let _ = socket.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use pl_queue::events::{ProgressStatus, ViewPermissions};

    #[test]
    fn visibility() {
        let u = SocketUser { id: 3, is_superuser: false, groups: vec![7] };
        let su = SocketUser { id: 2, is_superuser: true, groups: vec![] };
        let p = |perms: ViewPermissions| WsPayload::StatusUpdate {
            filename: Some("a.pdf".into()),
            task_id: Some("t".into()),
            current_progress: 0,
            max_progress: 100,
            status: ProgressStatus::Started,
            message: "new_file".into(),
            document_id: None,
            perms,
        };
        assert!(u.can_see(&p(ViewPermissions { owner_id: Some(3), ..Default::default() })));
        assert!(!u.can_see(&p(ViewPermissions { owner_id: Some(4), ..Default::default() })));
        assert!(u.can_see(&p(ViewPermissions { owner_id: Some(4), groups_can_view: vec![7], ..Default::default() })));
        assert!(u.can_see(&p(ViewPermissions { owner_id: None, users_can_view: vec![3], ..Default::default() })));
        assert!(su.can_see(&p(ViewPermissions { owner_id: Some(4), ..Default::default() })));
        assert!(u.can_see(&WsPayload::DocumentsDeleted { documents: vec![1] }));
    }

    /// Security review 2026-11-19: cross-site WebSocket hijacking – foreign origin rejected,
    /// own host, `CSRF_TRUSTED_ORIGINS` (also with `*`) and missing origin allowed.
    #[test]
    fn origin_check() {
        let mut cfg = pl_authz::AuthConfig::for_tests("s3cret");
        cfg.allowed_hosts = vec!["paperless.example".into(), "localhost".into()];
        cfg.csrf_trusted_origins = vec!["https://trusted.example".into(), "https://*.wild.example".into()];
        let h = |pairs: &[(&str, &str)]| {
            let mut m = HeaderMap::new();
            for (k, v) in pairs {
                m.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
            }
            m
        };
        let host = ("host", "paperless.example:8000");
        assert!(origin_allowed(&cfg, &h(&[host])), "no Origin: non-browser client");
        assert!(origin_allowed(&cfg, &h(&[host, ("origin", "http://paperless.example:8000")])));
        assert!(!origin_allowed(&cfg, &h(&[host, ("origin", "https://evil.example")])));
        assert!(!origin_allowed(&cfg, &h(&[host, ("origin", "http://paperless.example:9999")])));
        assert!(!origin_allowed(&cfg, &h(&[host, ("origin", "null")])));
        assert!(origin_allowed(&cfg, &h(&[host, ("origin", "https://trusted.example")])));
        assert!(origin_allowed(&cfg, &h(&[host, ("origin", "https://a.wild.example")])));
        assert!(!origin_allowed(&cfg, &h(&[host, ("origin", "http://a.wild.example")])));
        // host not allowed: only CSRF_TRUSTED_ORIGINS counts
        assert!(!origin_allowed(&cfg, &h(&[("host", "evil.example"), ("origin", "http://evil.example")])));
    }

    #[test]
    fn only_device_bearer_tokens_are_considered() {
        let h = |v: &str| {
            let mut m = HeaderMap::new();
            m.insert(axum::http::header::AUTHORIZATION, v.parse().unwrap());
            m
        };
        assert!(device_bearer(&h("Bearer prt_abc")));
        assert!(device_bearer(&h("bearer  prt_abc")));
        assert!(!device_bearer(&h("Token a0a0a0a0")));
        assert!(!device_bearer(&h("Bearer eyJhbGciOi")));
        assert!(!device_bearer(&h("Basic YWRtaW46YWRtaW4=")));
        assert!(!device_bearer(&HeaderMap::new()));
    }
}
