//! Isolation proof: crash, infinite loop, memory hog, `abort`, stack overflow and
//! malicious/broken files in the child process never hit the calling process; every run ends
//! within the time limit with an error instead of hanging.

mod common;

use std::time::{Duration, Instant};

use common::*;
use pl_pdftools::job::{Op, SelfTest};
use pl_pdftools::runner::{Limits, RunError, run_with};

fn short(secs: u64, mb: u64) -> Limits {
    Limits { timeout: Duration::from_secs(secs), memory_mb: mb }
}

#[tokio::test]
async fn panic_in_tool_is_a_contained_error() {
    let e = run_with(&tool(), Op::SelfTest { mode: SelfTest::Panic }, short(30, 256)).await.unwrap_err();
    assert!(matches!(e, RunError::Failed(_)), "{e:?}");
}

#[tokio::test]
async fn hang_is_killed_at_the_time_limit() {
    let start = Instant::now();
    let e = run_with(&tool(), Op::SelfTest { mode: SelfTest::Hang }, short(2, 256)).await.unwrap_err();
    assert_eq!(e, RunError::Timeout(2));
    assert!(start.elapsed() < Duration::from_secs(20), "{:?}", start.elapsed());
}

#[tokio::test]
async fn memory_hog_hits_the_memory_guard() {
    let e = run_with(&tool(), Op::SelfTest { mode: SelfTest::Oom }, short(60, 128)).await.unwrap_err();
    assert!(matches!(e, RunError::Crashed(_)), "{e:?}");
}

#[tokio::test]
async fn abort_and_stack_overflow_only_end_the_child() {
    for mode in [SelfTest::Abort, SelfTest::StackOverflow] {
        let e = run_with(&tool(), Op::SelfTest { mode }, short(60, 512)).await.unwrap_err();
        assert!(matches!(e, RunError::Crashed(_)), "{mode:?}: {e:?}");
    }
    // the calling process is alive and can continue working immediately
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("ok.pdf");
    text_pdf(&src);
    let ok = run_with(&tool(), Op::Search { input: src.to_string_lossy().into(), terms: vec!["4711".into()], max_hits: 5 }, limits()).await;
    assert!(ok.is_ok(), "{ok:?}");
}

/// Broken and hostile files: every operation ends with an error (or a result),
/// never with a hang beyond the time limit or a crash of the caller.
#[tokio::test]
async fn hostile_and_broken_files_never_hang_or_crash_the_caller() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path();
    let mut files: Vec<(&str, Vec<u8>)> = vec![
        ("empty.pdf", Vec::new()),
        ("text.pdf", b"just some text, not a pdf".to_vec()),
        ("header-only.pdf", b"%PDF-1.7\n%%EOF\n".to_vec()),
        ("truncated.pdf", b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R".to_vec()),
    ];
    // deep nesting (stack), huge length value, self reference in the page tree
    let mut deep = b"%PDF-1.4\n1 0 obj\n".to_vec();
    deep.extend(std::iter::repeat_n(b'[', 200_000));
    deep.extend_from_slice(b"\nendobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
    files.push(("deep.pdf", deep));
    files.push((
        "loop.pdf",
        b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [2 0 R] /Count 1 >> endobj\n\
          3 0 obj << /Length 999999999999 >> stream\nxx\nendstream endobj\ntrailer << /Root 1 0 R /Size 4 >>\n%%EOF\n"
            .to_vec(),
    ));
    // Zip bomb: 64 MiB of zeros via Flate as content stream (small file, large content)
    {
        use std::io::Write;
        let mut enc = flate2_stub::Encoder::new();
        for _ in 0..64 {
            enc.write_all(&[0u8; 1024 * 1024]).unwrap();
        }
        let z = enc.finish();
        let mut bomb = format!(
            "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >> endobj\n4 0 obj << /Length {} /Filter /FlateDecode >> stream\n",
            z.len()
        )
        .into_bytes();
        bomb.extend_from_slice(&z);
        bomb.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R /Size 5 >>\n%%EOF\n");
        files.push(("bomb.pdf", bomb));
    }
    let lim = short(60, 512);
    for (name, data) in &files {
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        let input: String = p.to_string_lossy().into();
        let out: String = dir.join(format!("{name}.out")).to_string_lossy().into();
        let ops = vec![
            Op::Repair { input: input.clone(), output: out.clone() },
            Op::Search { input: input.clone(), terms: vec!["x".into()], max_hits: 10 },
            Op::Shrink { input: input.clone(), output: out.clone(), preset: pl_pdftools::ShrinkPreset::Standard },
            Op::Redact { input: input.clone(), output: out.clone(), areas: vec![], terms: vec!["x".into()], remove_metadata: true },
        ];
        for op in ops {
            let start = Instant::now();
            let r = run_with(&tool(), op.clone(), lim).await;
            assert!(start.elapsed() < Duration::from_secs(90), "{name}: {op:?} took {:?}", start.elapsed());
            // The result does not matter (error expected, with "bomb" possibly success) – the main thing is controlled.
            if let Err(e) = &r {
                assert!(!e.user_message().contains(dir.to_string_lossy().as_ref()), "no paths in messages: {e:?}");
            }
        }
    }
}

/// Minimal Flate encoder via lopdf (flate2 already comes with it) – avoids an own
/// test dependency.
mod flate2_stub {
    pub struct Encoder(Vec<u8>);
    impl Encoder {
        pub fn new() -> Self {
            Encoder(Vec::new())
        }
        pub fn finish(self) -> Vec<u8> {
            let mut s = lopdf::Stream::new(lopdf::Dictionary::new(), self.0);
            let _ = s.compress();
            s.content
        }
    }
    impl std::io::Write for Encoder {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
