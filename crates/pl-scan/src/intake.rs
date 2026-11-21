//! Hand-over to the normal ingestion pipeline (`documents.tasks.consume_file` via
//! `pl_queue::TaskQueue`), exactly like `POST /api/documents/post_document/` (pl-server/upload.rs):
//! file into its own directory under `SCRATCH_DIR`, `ConsumableDocument` + overrides with
//! `owner_id`, then `enqueue_consume`. Workflows, permissions, owner: as with uploading.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use pl_consumer::{ConsumableDocument, DocumentMetadataOverrides, DocumentSource};
use rand::Rng as _;

/// A file for the pipeline.
#[derive(Debug, Clone)]
pub struct IntakeDoc {
    /// File in the spool directory (from [`spool`]).
    pub path: PathBuf,
    /// Original file name (for `overrides.filename`).
    pub filename: String,
    pub title: Option<String>,
    pub owner_id: i64,
    pub source: DocumentSource,
}

/// Target of the hand-over (replaceable for tests).
pub trait Intake: Send + Sync {
    /// → task ID (`PaperlessTask.task_id`).
    fn submit(&self, doc: IntakeDoc) -> BoxFuture<'_, Result<String, String>>;
}

/// Production: via the server's queue.
pub struct QueueIntake(pub Arc<dyn pl_queue::TaskQueue>);

impl Intake for QueueIntake {
    fn submit(&self, doc: IntakeDoc) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            let input = ConsumableDocument::new(doc.source, &doc.path).map_err(|e| e.to_string())?;
            let overrides = DocumentMetadataOverrides {
                filename: Some(doc.filename),
                title: doc.title,
                owner_id: Some(doc.owner_id),
                ..Default::default()
            };
            let id = pl_consumer::task::enqueue_consume(self.0.as_ref(), &input, &overrides).await?;
            Ok(id.to_string())
        })
    }
}

/// Intake from the `AppState` (queue as a service, set by pl-server).
pub fn from_state(state: &pl_core::AppState) -> Option<Arc<dyn Intake>> {
    let q = state.service::<Arc<dyn pl_queue::TaskQueue>>()?;
    Some(Arc::new(QueueIntake(q)))
}

/// Safe file name (Django `sanitize_file_name` + pathvalidate), default `fallback`.
pub fn safe_name(raw: &str, fallback: &str) -> String {
    let base = pl_core::safepath::sanitize_file_name(raw).unwrap_or_default();
    let s = pl_templating::sanitize_filename(&base, "");
    let s: String = s.chars().take(200).collect();
    if s.trim().is_empty() || s.starts_with('.') { fallback.to_string() } else { s }
}

/// New private directory `SCRATCH_DIR/tmpXXXXXXXX/` with the file `name` → path.
/// The directory remains (the consumer cleans up after ingestion, as with upload).
pub fn spool(scratch: &Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    let dir = spool_dir(scratch)?;
    let p = dir.join(name);
    if let Err(e) = std::fs::write(&p, data) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(p)
}

/// Only the directory (for files written directly there).
pub fn spool_dir(scratch: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(scratch)?;
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut rng = rand::thread_rng();
    for _ in 0..100 {
        let name: String = (0..8).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect();
        let p = scratch.join(format!("tmp{name}"));
        let r = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                std::fs::DirBuilder::new().mode(0o700).create(&p)
            }
            #[cfg(not(unix))]
            {
                std::fs::DirBuilder::new().create(&p)
            }
        };
        match r {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no usable temporary directory name"))
}

/// Remove the file together with the spool directory (after a failed hand-over).
pub fn discard(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Queue that only records jobs (checks the real kwargs of `enqueue_consume`).
    use std::sync::Mutex;

    use futures::future::BoxFuture;
    use pl_queue::{EnqueueOptions, QueueError, TaskInfo};
    use serde_json::{Map, Value};
    use uuid::Uuid;

    /// (task, kwargs, trigger_source)
    pub type Call = (String, Map<String, Value>, Option<String>);

    #[derive(Default)]
    pub struct RecordingQueue {
        pub calls: Mutex<Vec<Call>>,
    }

    impl pl_queue::TaskQueue for RecordingQueue {
        fn enqueue<'a>(
            &'a self,
            task_name: &'a str,
            kwargs: Map<String, Value>,
            opts: EnqueueOptions,
        ) -> BoxFuture<'a, Result<Uuid, QueueError>> {
            Box::pin(async move {
                if let Ok(mut c) = self.calls.lock() {
                    c.push((task_name.to_string(), kwargs, opts.trigger_source.clone()));
                }
                Ok(Uuid::new_v4())
            })
        }
        fn status(&self, _task_id: Uuid) -> BoxFuture<'_, Result<Option<TaskInfo>, QueueError>> {
            Box::pin(async { Ok(None) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(safe_name("../../etc/passwd", "x.pdf"), "passwd");
        assert_eq!(safe_name("C:\\Scans\\Rechnung 01.pdf", "x.pdf"), "Rechnung 01.pdf");
        assert_eq!(safe_name("", "x.pdf"), "x.pdf");
        assert_eq!(safe_name("..", "x.pdf"), "x.pdf");
        assert_eq!(safe_name(".hidden", "x.pdf"), "x.pdf");
    }

    #[tokio::test]
    async fn queue_intake_sets_owner_and_source() {
        let t = tempfile::tempdir().unwrap();
        let p = spool(t.path(), "scan.pdf", b"%PDF-1.4\n%%EOF\n").unwrap();
        assert!(p.parent().unwrap().file_name().unwrap().to_string_lossy().starts_with("tmp"));
        let q = Arc::new(testing::RecordingQueue::default());
        let intake = QueueIntake(q.clone());
        let id = intake
            .submit(IntakeDoc {
                path: p,
                filename: "scan.pdf".into(),
                title: Some("Brief".into()),
                owner_id: 7,
                source: DocumentSource::WebUI,
            })
            .await
            .unwrap();
        assert!(!id.is_empty());
        let calls = q.calls.lock().unwrap();
        let (name, kw, trig) = &calls[0];
        assert_eq!(name, "documents.tasks.consume_file");
        assert_eq!(trig.as_deref(), Some("web_ui"));
        assert_eq!(kw["overrides"]["owner_id"], 7);
        assert_eq!(kw["overrides"]["title"], "Brief");
        assert_eq!(kw["input_doc"]["mime_type"], "application/pdf");
    }
}
