//! Review finding 3: the full timeline recompute (backfill) is guarded by a PostgreSQL advisory lock
//! (`refresh::BACKFILL_LOCK_KEY`), not by process memory. Only takes and inspects advisory locks (no table
//! is read or written), but still only against a local **test** database:
//! ```text
//! PAPERRUST_TIMELINE_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_timeline_test \
//!   cargo test -p pl-timeline --test backfill_lock -j 2 -- --ignored --test-threads 1
//! ```

use pl_timeline::extract::DateParse;
use pl_timeline::refresh::{self, BACKFILL_LOCK_KEY};
use sqlx::{Connection, PgConnection, PgPool};

fn url() -> String {
    let url = std::env::var("PAPERRUST_TIMELINE_TEST_DATABASE_URL").expect("PAPERRUST_TIMELINE_TEST_DATABASE_URL");
    let db = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    assert!(db.starts_with("pl_") && db.contains("test"), "refusing to run against a non-test database ({db})");
    url
}

async fn pool() -> PgPool {
    sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url()).await.expect("connect")
}

/// Another server/worker process holds the lock: this process must neither start a second backfill nor
/// claim none is running. Before the fix the lock lived in a process-local `AtomicBool`, so a second
/// process started its own full recompute.
#[tokio::test]
#[ignore = "needs a local test PostgreSQL, see the module comment"]
async fn lock_held_by_another_process_blocks_backfill() {
    let pool = pool().await;
    let mut other = PgConnection::connect(&url()).await.expect("connect");
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
        .bind(BACKFILL_LOCK_KEY.0)
        .bind(BACKFILL_LOCK_KEY.1)
        .fetch_one(&mut other)
        .await
        .unwrap();
    assert!(got, "lock free at test start");
    assert!(refresh::backfill_running_any(&pool).await.unwrap(), "seen as running across processes");
    assert!(refresh::try_lock_backfill(&pool).await.unwrap().is_none());
    let dp = DateParse::from_lookup(|_| None);
    assert!(refresh::backfill(&pool, &dp, false).await.unwrap().is_none(), "no second backfill");
    assert!(!refresh::backfill_running(), "nothing runs in this process");

    // the other process ends → the lock is free again
    other.close().await.unwrap();
    let lock = refresh::try_lock_backfill(&pool).await.unwrap().expect("lock free after the other session ended");
    assert!(refresh::backfill_running());
    lock.release().await;
    assert!(!refresh::backfill_running());
    assert!(!refresh::backfill_running_any(&pool).await.unwrap());
}

/// Two concurrent starts (the `POST /timeline/backfill` race): exactly one gets the lock. Before the fix
/// both handler calls passed the `backfill_running()` check and both answered "started".
#[tokio::test]
#[ignore = "needs a local test PostgreSQL, see the module comment"]
async fn concurrent_starts_get_the_lock_once() {
    let pool = pool().await;
    let (a, b) = tokio::join!(refresh::try_lock_backfill(&pool), refresh::try_lock_backfill(&pool));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.is_some() as u8 + b.is_some() as u8, 1, "exactly one start wins");
    // a dropped (not released) lock is freed too: its connection is closed, never returned to the pool
    drop(a);
    drop(b);
    let mut again = None;
    for _ in 0..50 {
        again = refresh::try_lock_backfill(&pool).await.unwrap();
        if again.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let lock = again.expect("lock free after drop");
    lock.release().await;
}
