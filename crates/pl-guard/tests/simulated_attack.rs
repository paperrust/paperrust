//! Simulated ransomware attack on a temporary media folder (without a database: catalogue in
//! memory, receiver in memory). Checks:
//! * real file system events + main loop trigger the alarm,
//! * the review alone (without events) detects the attack,
//! * normal PaperRust operations (new version, renaming via storage path, deleting) trigger nothing,
//! * a silent modification without a name change is found by the sampling.
//!
//! The same procedure exists as a script for manual experiments: `tests/simulate-ransomware.mjs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use pl_guard::config::GuardConfig;
use pl_guard::guard::{Engine, Expected, MemoryCatalog, MemoryReporter, Reason, sha256_file};

fn keystream(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 32) as u8
        })
        .collect()
}

fn pdf(i: usize) -> Vec<u8> {
    let mut v = format!("%PDF-1.7\n% Dokument {i}\n").into_bytes();
    v.extend(keystream(i as u64 + 7, 4000));
    v.extend(b"\n%%EOF\n");
    v
}

fn webp(i: usize) -> Vec<u8> {
    let mut v = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    v.extend(keystream(i as u64 + 99, 800));
    v
}

/// Media folder with `n` documents (original, archive, thumbnail) + matching catalogue.
fn media(n: usize) -> (tempfile::TempDir, Arc<MemoryCatalog>) {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    for d in ["documents/originals/2024", "documents/archive/2024", "documents/thumbnails"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(root.join(".paperrust-test-media"), b"").unwrap();
    let cat = MemoryCatalog::default();
    for i in 1..=n {
        let name = format!("2024/Rechnung {i:03}.pdf");
        let o = root.join("documents/originals").join(&name);
        let a = root.join("documents/archive").join(&name);
        std::fs::write(&o, pdf(i)).unwrap();
        std::fs::write(&a, pdf(i + 10_000)).unwrap();
        std::fs::write(root.join(format!("documents/thumbnails/{i:07}.webp")), webp(i)).unwrap();
        let id = i as i64;
        cat.originals.lock().unwrap().insert(name.clone(), Expected { doc_id: id, checksum: Some(sha256_file(&o).unwrap()) });
        cat.archive.lock().unwrap().insert(name.clone(), Expected { doc_id: id, checksum: Some(sha256_file(&a).unwrap()) });
        cat.thumbnails.lock().unwrap().insert(format!("{i:07}.webp"), Expected { doc_id: id, checksum: None });
    }
    (t, Arc::new(cat))
}

/// The "attack": read files, "encrypt" with a key stream, write as `<name>.locked`,
/// delete the original; plus a ransom note. Like `tests/simulate-ransomware.mjs`.
fn attack(root: &Path, count: usize) -> Vec<PathBuf> {
    assert!(root.join(".paperrust-test-media").exists(), "nur auf Testordnern");
    let mut hit = Vec::new();
    let dir = root.join("documents/originals/2024");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    for p in files.into_iter().take(count) {
        let data = std::fs::read(&p).unwrap();
        let ks = keystream(0xDEADBEEF, data.len());
        let enc: Vec<u8> = data.iter().zip(ks).map(|(a, b)| a ^ b).collect();
        let mut locked = p.clone().into_os_string();
        locked.push(".locked");
        std::fs::write(&locked, enc).unwrap();
        std::fs::remove_file(&p).unwrap();
        hit.push(p);
    }
    std::fs::write(dir.join("README_DECRYPT.txt"), b"Your files are encrypted. Pay 1 BTC.").unwrap();
    hit
}

fn cfg(threshold: i64) -> GuardConfig {
    let mut c = GuardConfig::from_lookup(|_| None);
    c.threshold = threshold;
    c.window_minutes = 10;
    c.grace_secs = 0;
    c.scan_interval_secs = 1;
    c
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attack_detected_by_running_guard() {
    let (t, cat) = media(40);
    let rep = Arc::new(MemoryReporter::default());
    let engine = Engine::new(t.path().to_path_buf(), cfg(20), cat.clone(), rep.clone());
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let h = tokio::spawn(engine.run(stop_rx));
    // wait for the initial state (first review after ~5 s), then attack
    tokio::time::sleep(Duration::from_secs(7)).await;
    attack(t.path(), 30);
    let start = Instant::now();
    while !rep.triggered.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(40) {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let _ = stop_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(10), h).await;
    assert!(rep.triggered.load(Ordering::SeqCst), "read-only mode not triggered; findings: {:?}", rep.findings.lock().unwrap().len());
    assert_eq!(rep.trigger_count.load(Ordering::SeqCst), 1, "nur einmal auslösen");
    let f = rep.findings.lock().unwrap();
    assert!(f.iter().any(|f| f.reason == Reason::SuspiciousExtension && f.path.ends_with(".pdf.locked")), "{f:?}");
    assert!(f.iter().any(|f| f.reason == Reason::Deleted), "{f:?}");
    assert!(f.len() >= 20);
}

#[tokio::test]
async fn attack_detected_by_scan_only() {
    let (t, cat) = media(30);
    let rep = Arc::new(MemoryReporter::default());
    let mut e = Engine::new(t.path().to_path_buf(), cfg(20), cat, rep.clone());
    let now = Instant::now();
    let base = e.scan(now).await;
    assert_eq!(base.files, 90);
    assert!(base.changed.is_empty());
    attack(t.path(), 25);
    let r = e.scan(now).await;
    // 25 new .locked files + note; the review only counts the deleted originals
    assert_eq!(r.changed.len(), 26, "{:?}", r.changed);
    assert_eq!(r.removed, 25);
    let step = e.process_due(now + Duration::from_secs(1)).await;
    assert_eq!(step.findings, 26);
    assert!(step.triggered);
    let f = rep.findings.lock().unwrap();
    assert!(f.iter().any(|f| f.reason == Reason::RansomNote), "{f:?}");
}

#[tokio::test]
async fn normal_operations_are_not_suspicious() {
    let (t, cat) = media(30);
    let root = t.path();
    let rep = Arc::new(MemoryReporter::default());
    let mut e = Engine::new(root.to_path_buf(), cfg(1), cat.clone(), rep.clone());
    let now = Instant::now();
    e.scan(now).await;
    // 1. rename via storage path: move file, update catalogue (like pl-documents-write)
    for i in 1..=10 {
        let old = format!("2024/Rechnung {i:03}.pdf");
        let new = format!("Firma/Rechnung {i:03}.pdf");
        std::fs::create_dir_all(root.join("documents/originals/Firma")).unwrap();
        std::fs::rename(root.join("documents/originals").join(&old), root.join("documents/originals").join(&new)).unwrap();
        let mut g = cat.originals.lock().unwrap();
        let v = g.remove(&old).unwrap();
        g.insert(new, v);
    }
    // 2. reprocessed: archive file replaced, new checksum in the DB
    for i in 11..=20 {
        let name = format!("2024/Rechnung {i:03}.pdf");
        let p = root.join("documents/archive").join(&name);
        std::fs::write(&p, pdf(i + 50_000)).unwrap();
        cat.archive.lock().unwrap().insert(name, Expected { doc_id: i as i64, checksum: Some(sha256_file(&p).unwrap()) });
    }
    // 3. trash emptied: files and rows gone
    for i in 21..=30 {
        let name = format!("2024/Rechnung {i:03}.pdf");
        std::fs::remove_file(root.join("documents/originals").join(&name)).unwrap();
        std::fs::remove_file(root.join("documents/archive").join(&name)).unwrap();
        std::fs::remove_file(root.join(format!("documents/thumbnails/{i:07}.webp"))).unwrap();
        cat.originals.lock().unwrap().remove(&name);
        cat.archive.lock().unwrap().remove(&name);
        cat.thumbnails.lock().unwrap().remove(&format!("{i:07}.webp"));
    }
    // 4. New document ingested
    let p = root.join("documents/originals/2024/Neu.pdf");
    std::fs::write(&p, pdf(777)).unwrap();
    cat.originals.lock().unwrap().insert("2024/Neu.pdf".into(), Expected { doc_id: 99, checksum: Some(sha256_file(&p).unwrap()) });
    // events of the deleted files would come from the watcher: note them as well
    for i in 21..=30 {
        e.note_change(format!("documents/originals/2024/Rechnung {i:03}.pdf"), now);
    }
    e.scan(now).await;
    let step = e.process_due(now + Duration::from_secs(1)).await;
    assert!(step.evaluated >= 30, "{step:?}");
    assert_eq!(step.findings, 0, "{:?}", rep.findings.lock().unwrap());
    assert!(!rep.triggered.load(Ordering::SeqCst));
}

#[tokio::test]
async fn silent_in_place_encryption_found_by_sample() {
    let (t, cat) = media(5);
    let root = t.path();
    let rep = Arc::new(MemoryReporter::default());
    let mut c = cfg(3);
    c.sample_files = 100;
    let mut e = Engine::new(root.to_path_buf(), c, cat, rep.clone());
    // same size, modification time reset: the review sees nothing
    for i in 1..=3 {
        let p = root.join(format!("documents/archive/2024/Rechnung {i:03}.pdf"));
        let m = std::fs::metadata(&p).unwrap();
        let mtime = m.modified().unwrap();
        let data = std::fs::read(&p).unwrap();
        let enc: Vec<u8> = data.iter().zip(keystream(5, data.len())).map(|(a, b)| a ^ b).collect();
        std::fs::write(&p, enc).unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(mtime).unwrap();
    }
    let step = e.sample(Instant::now()).await;
    assert_eq!(step.findings, 3, "{:?}", rep.findings.lock().unwrap());
    assert!(step.triggered);
    assert!(rep.findings.lock().unwrap().iter().all(|f| f.reason == Reason::Encrypted), "{:?}", rep.findings.lock().unwrap());
}

#[tokio::test]
async fn full_scan_command_lists_damage() {
    let (t, cat) = media(10);
    attack(t.path(), 4);
    let found = pl_guard::cli::scan_with(cat.as_ref(), &cfg(20), t.path()).await.unwrap();
    // 4 deleted + 4 .locked + note
    assert_eq!(found.len(), 9, "{found:?}");
}
