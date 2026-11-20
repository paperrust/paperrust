//! Markers between handlers and `pl-server` (response extensions or a
//! request-wide scratchpad). Domain crates do not know `pl-server`; through these types
//! they tell it what the Django middleware would otherwise know by itself.
//!
//! * [`DelegateToDjango`] / [`delegate_to_django`]: **the only** delegation mechanism.
//!   A handler that does not (yet) map a case in Rust responds with it; `pl-server`
//!   then sends the *original request* (buffered body, headers unchanged) to Django and
//!   returns its response. Without `pl-server` (e.g. in crate tests) the client sees 501.
//! * [`Authenticated`]: response extension "request was authenticated" → `pl-server` sets
//!   `X-Api-Version`/`X-Version` like `ApiVersionMiddleware`.
//! * [`RequestNotes`]: put into the request extensions by `pl-server`; after authentication `pl-authz`
//!   records whether a user is logged in and whether Django's `SessionMiddleware`
//!   would delete the session cookie. Handlers do not need to do anything for this.
//!
//! upstream: src/paperless/middleware.py@v3.4.2 (ApiVersionMiddleware),
//! django@5.2.18 contrib/sessions/middleware.py (SessionMiddleware.process_response)

use std::sync::{Arc, Mutex};

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// Response extension: "this request must be answered by Django". Only set before any
/// side effect has happened (the request is repeated unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegateToDjango;

/// Response extension: the response corresponds to Django's own error page (`response_for_exception`
/// → `bad_request`/`server_error`), i.e. an exception that DRF does not handle. There DRF sets
/// neither `Allow` nor `Vary: Accept` (`finalize_response` does not run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DjangoHandlerResponse;

/// Status code of the placeholder response if no `pl-server` carries out the delegation.
pub const DELEGATE_STATUS: StatusCode = StatusCode::NOT_IMPLEMENTED;

/// Response that `pl-server` replaces with Django's response to the original request.
pub fn delegate_to_django() -> Response {
    let mut r = (DELEGATE_STATUS, "Not implemented in PaperRust").into_response();
    r.extensions_mut().insert(DelegateToDjango);
    r
}

/// Is `resp` a delegation response?
pub fn is_delegation(resp: &Response) -> bool {
    resp.extensions().get::<DelegateToDjango>().is_some()
}

/// Response extension: the request was authenticated (`request.user.is_authenticated`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Authenticated;

/// What `pl-authz` records for `pl-server` during a request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    /// A user was authenticated.
    pub authenticated: bool,
    /// Additional `Set-Cookie` values (e.g. deletion of an empty session cookie).
    pub set_cookies: Vec<HeaderValue>,
}

/// Request extension (inserted by `pl-server`), shared between server and extractors.
#[derive(Debug, Clone, Default)]
pub struct RequestNotes(Arc<Mutex<Notes>>);

impl RequestNotes {
    pub fn new() -> Self {
        Self::default()
    }

    fn with<R>(&self, f: impl FnOnce(&mut Notes) -> R) -> R {
        let mut g = match self.0.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        f(&mut g)
    }

    /// Request is authenticated.
    pub fn mark_authenticated(&self) {
        self.with(|n| n.authenticated = true);
    }

    /// Append a `Set-Cookie` value (duplicate values are sent only once).
    pub fn add_set_cookie(&self, value: HeaderValue) {
        self.with(|n| {
            if !n.set_cookies.contains(&value) {
                n.set_cookies.push(value);
            }
        });
    }

    /// Current state (copy).
    pub fn snapshot(&self) -> Notes {
        self.with(|n| n.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegation_marker() {
        let r = delegate_to_django();
        assert_eq!(r.status(), StatusCode::NOT_IMPLEMENTED);
        assert!(is_delegation(&r));
        assert!(!is_delegation(&StatusCode::OK.into_response()));
    }

    #[test]
    fn notes_are_shared() {
        let n = RequestNotes::new();
        let c = n.clone();
        c.mark_authenticated();
        c.add_set_cookie(HeaderValue::from_static("a=b"));
        c.add_set_cookie(HeaderValue::from_static("a=b"));
        let s = n.snapshot();
        assert!(s.authenticated);
        assert_eq!(s.set_cookies.len(), 1);
    }
}
