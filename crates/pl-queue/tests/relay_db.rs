//! DB test for the status relay (`LISTEN paperrust_status`): reconnect after
//! `pg_terminate_backend`, no double delivery (bug from W14).
//!
//! Dedicated test database (never `paperless`):
//! ```text
//! CREATE DATABASE pl_sec_relay OWNER paperless TEMPLATE paperless_snapshot;
//! PAPERRUST_RELAY_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_sec_relay \
//!   cargo test -p pl-queue -j 1 --test relay_db -- --ignored
//! ```

use std::time::Duration;

use pl_queue::events::{self, EventBus, RelayBackoff, WsPayload};
use sqlx::PgPool;
use tokio::sync::{broadcast, watch};

async fn pool() -> PgPool {
    let url = std::env::var("PAPERRUST_RELAY_TEST_DATABASE_URL").expect("PAPERRUST_RELAY_TEST_DATABASE_URL");
    assert!(
        url.trim_end_matches('/').rsplit('/').next().is_some_and(|db| db.starts_with("pl_sec_relay")),
        "refusing to run against a non-test database"
    );
    sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url).await.expect("connect")
}

/// PIDs of the LISTEN connections of this database (except our own).
async fn listener_pids(pool: &PgPool) -> Vec<i32> {
    sqlx::query_scalar(
        "SELECT pid FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid() \
         AND query ILIKE 'LISTEN %paperrust_status%'",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn wait_for_listener(pool: &PgPool, not: &[i32]) -> i32 {
    for _ in 0..200 {
        if let Some(p) = listener_pids(pool).await.into_iter().find(|p| !not.contains(p)) {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("relay did not (re)connect");
}

async fn expect_exactly(rx: &mut broadcast::Receiver<WsPayload>, want: &WsPayload) {
    let got = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("event not relayed").unwrap();
    assert_eq!(&got, want);
    // no double delivery
    assert!(tokio::time::timeout(Duration::from_millis(500), rx.recv()).await.is_err(), "event delivered twice");
}

#[tokio::test]
#[ignore = "braucht PAPERRUST_RELAY_TEST_DATABASE_URL (pl_sec_relay)"]
async fn relay_reconnects_after_terminate_backend() {
    let pool = pool().await;
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let (stop_tx, stop_rx) = watch::channel(false);
    let backoff = RelayBackoff { min: Duration::from_millis(50), max: Duration::from_millis(400) };
    let relay = tokio::spawn(events::relay_notifications_with(pool.clone(), bus.clone(), stop_rx, backoff));

    let first = wait_for_listener(&pool, &[]).await;
    let a = WsPayload::DocumentsDeleted { documents: vec![1] };
    events::notify(&pool, &a).await.unwrap();
    expect_exactly(&mut rx, &a).await;

    // kill our own listener connection twice in a row
    let mut killed = vec![first];
    for round in 2..4 {
        let victim = *killed.last().unwrap();
        let ok: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)").bind(victim).fetch_one(&pool).await.unwrap();
        assert!(ok);
        let next = wait_for_listener(&pool, &killed).await;
        killed.push(next);
        assert!(!relay.is_finished(), "relay task ended after terminate");
        let p = WsPayload::DocumentsDeleted { documents: vec![round] };
        events::notify(&pool, &p).await.unwrap();
        expect_exactly(&mut rx, &p).await;
    }
    // exactly one listener active
    assert_eq!(listener_pids(&pool).await.len(), 1);

    stop_tx.send(true).unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), relay).await.expect("relay did not stop").unwrap();
    assert!(r.is_ok());
}
