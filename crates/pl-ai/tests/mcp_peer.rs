//! Security review final M2/M3: failed attempts and pre-login limit per client IP.
//! Without network: the handlers are called directly with the peer set.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue};
use pl_ai::classifier::IdentityTruncator;
use pl_ai::mcp::http::{client_key, handle_post_from};
use pl_ai::mcp::memory::{MemUser, MemoryBackend};
use pl_ai::mcp::{McpService, McpSettings};

const ALICE: &str = "alicetoken0000000000000000000000000000000";

fn svc(settings: McpSettings) -> McpService<MemoryBackend> {
    let b = MemoryBackend::default();
    let perms: BTreeSet<String> = ["documents.view_document".to_string()].into_iter().collect();
    b.add_user(MemUser {
        id: 1,
        username: "alice".into(),
        token: ALICE.into(),
        is_superuser: false,
        is_active: true,
        groups: vec![],
        perms,
    });
    McpService::new(Arc::new(b), settings, None, Arc::new(IdentityTruncator))
}

fn headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("authorization", HeaderValue::from_str(&format!("Token {token}")).unwrap());
    h.insert("content-type", HeaderValue::from_static("application/json"));
    h
}

const INIT: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;

fn ip(s: &str) -> Option<IpAddr> {
    Some(s.parse().unwrap())
}

/// M2: an attacker with many invalid tokens locks out only their own IP, not other users
/// (who are not in the login cache).
#[tokio::test]
async fn failures_counted_per_ip() {
    let s = svc(McpSettings { max_auth_failures_per_minute: 3, auth_cache_secs: 0, ..Default::default() });
    for i in 0..3 {
        let r = handle_post_from(&s, ip("203.0.113.9"), &headers(&format!("guess{i:036}")), INIT).await;
        assert_eq!(r.status(), 401);
    }
    let r = handle_post_from(&s, ip("203.0.113.9"), &headers("guess-after-limit"), INIT).await;
    assert_eq!(r.status(), 429);
    assert!(r.headers().get("retry-after").is_some());
    // even a valid token from the attacker IP is rejected without a backend call
    assert_eq!(handle_post_from(&s, ip("203.0.113.9"), &headers(ALICE), INIT).await.status(), 429);
    // another client (without cache, auth_cache_secs = 0) works normally
    assert_eq!(handle_post_from(&s, ip("198.51.100.4"), &headers(ALICE), INIT).await.status(), 200);
    // IPv6: the same /64 counts together, a different /64 does not
    for i in 0..3 {
        let a = format!("2001:db8:1:2::{:x}", i + 1);
        assert_eq!(handle_post_from(&s, ip(&a), &headers("bad"), INIT).await.status(), 401);
    }
    assert_eq!(handle_post_from(&s, ip("2001:db8:1:2::ffff"), &headers("bad"), INIT).await.status(), 429);
    assert_eq!(handle_post_from(&s, ip("2001:db8:1:3::1"), &headers(ALICE), INIT).await.status(), 200);
}

/// The global emergency brake only takes effect at the (high) global threshold.
#[tokio::test]
async fn global_brake() {
    let s = svc(McpSettings {
        max_auth_failures_per_minute: 100,
        max_auth_failures_global_per_minute: 5,
        auth_cache_secs: 0,
        ..Default::default()
    });
    for i in 0..5 {
        let a = format!("192.0.2.{}", i + 1);
        assert_eq!(handle_post_from(&s, ip(&a), &headers("bad"), INIT).await.status(), 401);
    }
    assert_eq!(handle_post_from(&s, ip("192.0.2.200"), &headers(ALICE), INIT).await.status(), 429);
}

/// Pre-login limit per IP (not per token): changing tokens do not circumvent it.
#[tokio::test]
async fn pre_auth_limit_per_ip_not_per_token() {
    let s = svc(McpSettings { rate_limit_per_minute: 2, max_auth_failures_per_minute: 1000, ..Default::default() });
    assert_eq!(handle_post_from(&s, ip("203.0.113.1"), &headers("t1"), INIT).await.status(), 401);
    assert_eq!(handle_post_from(&s, ip("203.0.113.1"), &headers("t2"), INIT).await.status(), 401);
    assert_eq!(handle_post_from(&s, ip("203.0.113.1"), &headers("t3"), INIT).await.status(), 429);
    assert_eq!(handle_post_from(&s, ip("203.0.113.2"), &headers("t4"), INIT).await.status(), 401);
}

#[test]
fn client_key_trusted_proxies() {
    let mut h = HeaderMap::new();
    h.insert("x-forwarded-for", HeaderValue::from_static("1.2.3.4, 10.0.0.5"));
    // XFF from an untrusted peer is ignored
    assert_eq!(client_key(ip("198.51.100.7"), &h, &[]), "ip:198.51.100.7");
    // trusted proxy: rightmost untrusted address
    let trusted = vec!["10.0.0.0/8".to_string(), "127.0.0.1".to_string()];
    assert_eq!(client_key(ip("127.0.0.1"), &h, &trusted), "ip:1.2.3.4");
    assert_eq!(client_key(ip("::ffff:127.0.0.1"), &h, &trusted), "ip:1.2.3.4");
    // invalid entry: abort, last valid address applies
    let mut bad = HeaderMap::new();
    bad.insert("x-forwarded-for", HeaderValue::from_static("garbage, 10.0.0.9"));
    assert_eq!(client_key(ip("127.0.0.1"), &bad, &trusted), "ip:10.0.0.9");
    assert_eq!(client_key(None, &h, &trusted), "ip:unknown");
    assert_eq!(client_key(ip("2001:db8:a:b:c:d:e:f"), &h, &trusted), "ip6:2001:db8:a:b::/64");
}
