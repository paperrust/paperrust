//! S3 target against the replica in `support/s3mock.rs` (without a database): signature, Object Lock,
//! listing over several pages, extending the retention period, setting up and decrypting the key.
//! **Not** tested against real S3/MinIO (see docs/contract/pl-guard.md).

#[path = "support/s3mock.rs"]
mod s3mock;

use chrono::Utc;
use pl_guard::backup::target::Store;
use pl_guard::config::LockMode;

#[tokio::test]
async fn object_lock_and_listing() {
    let (ep, state) = s3mock::start().await;
    let t = s3mock::target(&ep, "nas1/paperrust", LockMode::Compliance);
    let s = Store::open(&t).unwrap();
    let until = s.retain_until(Utc::now());
    assert!(until.is_some());
    for i in 0..7 {
        s.put(&format!("objects/a{i}/x y&{i}"), format!("data{i}").into_bytes(), until).await.unwrap();
    }
    s.put("config.json", b"{}".to_vec(), None).await.unwrap();
    let keys = s.list("objects/").await.unwrap();
    assert_eq!(keys.len(), 7, "Seiten zu je 3 zusammengesetzt: {keys:?}");
    assert!(keys.contains(&"objects/a3/x y&3".to_string()));
    assert_eq!(s.get("objects/a1/x y&1").await.unwrap().as_deref(), Some(&b"data1"[..]));
    assert!(s.exists("config.json").await.unwrap());
    assert!(!s.exists("nope").await.unwrap());
    // locked: neither delete nor overwrite
    assert!(!s.delete("objects/a1/x y&1").await.unwrap(), "Löschen muss an der Sperrfrist scheitern");
    assert!(s.put("objects/a1/x y&1", b"evil".to_vec(), None).await.is_err());
    assert_eq!(s.get("objects/a1/x y&1").await.unwrap().as_deref(), Some(&b"data1"[..]));
    // unlocked: deletable
    assert!(s.delete("config.json").await.unwrap());
    // extend retention period
    let later = Utc::now() + chrono::Duration::days(10);
    s.extend_retention("objects/a2/x y&2", later).await.unwrap();
    let g = state.lock().unwrap();
    let o = &g.objects["nas1/paperrust/objects/a2/x y&2"];
    assert_eq!(o.mode.as_deref(), Some("COMPLIANCE"));
    assert!(o.retain_until.unwrap() > Utc::now() + chrono::Duration::days(9));
    assert_eq!(g.bad_signatures, 0);
}

#[tokio::test]
async fn wrong_secret_is_rejected() {
    let (ep, state) = s3mock::start().await;
    let mut t = s3mock::target(&ep, "", LockMode::Off);
    if let pl_guard::config::Target::S3(c) = &mut t {
        c.secret_key = "wrong".into();
    }
    let s = Store::open(&t).unwrap();
    let e = s.put("x", b"1".to_vec(), None).await.unwrap_err();
    assert!(e.contains("SignatureDoesNotMatch"), "{e}");
    assert_eq!(state.lock().unwrap().bad_signatures, 1);
}

#[tokio::test]
async fn init_and_unlock_identity_on_s3() {
    let (ep, _state) = s3mock::start().await;
    let t = s3mock::target(&ep, "repo", LockMode::Governance);
    let data = tempfile::tempdir().unwrap();
    let (public, secret) = pl_guard::backup::init(&t, data.path(), "a long test passphrase", Some(10)).await.unwrap();
    assert!(public.starts_with("age1") && secret.starts_with("AGE-SECRET-KEY-1"));
    assert!(pl_guard::backup::init(&t, data.path(), "a long test passphrase", Some(10)).await.is_err(), "zweimal einrichten");
    let id = pl_guard::backup::identity(&t, Some("a long test passphrase"), None).await.unwrap();
    assert_eq!(id.to_public().to_string(), public);
    assert!(pl_guard::backup::identity(&t, Some("wrong passphrase!!"), None).await.is_err());
    assert!(pl_guard::backup::init(&t, data.path(), "short", Some(10)).await.is_err());
}
