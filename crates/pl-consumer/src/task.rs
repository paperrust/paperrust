// Task `documents.tasks.consume_file` for the own queue (pl-queue, JSON kwargs like
// docs/contract/pl-queue.md B5) and enqueueing (input folder, barcode splitting).
//
// upstream: src/documents/tasks.py@v3.4.2 (consume_file), src/documents/management/commands/
//           document_consumer.py@v3.4.2 (_consume_file: apply_async with trigger_source folder_consume),
//           src/documents/barcodes.py@v3.4.2 (_SOURCE_TO_TRIGGER)

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::consumer::Consumer;
use crate::data::{ConsumableDocument, DocumentMetadataOverrides, DocumentSource};

pub const TASK_NAME: &str = "documents.tasks.consume_file";

/// Wait until the first retry if the database or remote OCR were unreachable during ingestion
/// (doubled afterwards: 15, 30, 60 s).
pub const TRANSIENT_RETRY_SECS: f64 = 15.0;
/// Maximum number of such retries (Celery default `max_retries=3`).
pub const TRANSIENT_MAX_RETRIES: u32 = 3;

/// Wait before retry number `retries + 1`.
pub fn transient_countdown(retries: i32) -> f64 {
    TRANSIENT_RETRY_SECS * f64::from(1u32 << retries.clamp(0, 4) as u32)
}

/// `PaperlessTask.TriggerSource` je `DocumentSource` (`_SOURCE_TO_TRIGGER`).
pub fn trigger_source_for(source: DocumentSource) -> &'static str {
    match source {
        DocumentSource::ConsumeFolder => "folder_consume",
        DocumentSource::ApiUpload => "api_upload",
        DocumentSource::MailFetch => "email_consume",
        DocumentSource::WebUI => "web_ui",
    }
}

/// kwargs `{"input_doc": ..., "overrides": ...}`.
pub fn consume_kwargs(doc: &ConsumableDocument, overrides: &DocumentMetadataOverrides) -> Result<Map<String, Value>, String> {
    let mut m = Map::new();
    m.insert("input_doc".into(), serde_json::to_value(doc).map_err(|e| e.to_string())?);
    m.insert("overrides".into(), serde_json::to_value(overrides).map_err(|e| e.to_string())?);
    Ok(m)
}

/// `consume_file.apply_async(kwargs=..., headers={"trigger_source": ...})`.
pub async fn enqueue_consume(
    queue: &dyn pl_queue::TaskQueue,
    doc: &ConsumableDocument,
    overrides: &DocumentMetadataOverrides,
) -> Result<uuid::Uuid, String> {
    let kwargs = consume_kwargs(doc, overrides)?;
    let opts = pl_queue::EnqueueOptions {
        trigger_source: Some(trigger_source_for(doc.source()).to_string()),
        ..Default::default()
    };
    queue.enqueue(TASK_NAME, kwargs, opts).await.map_err(|e| e.to_string())
}

/// Reads the kwargs of a `consume_file` task.
pub fn parse_kwargs(kwargs: &Map<String, Value>) -> Result<(ConsumableDocument, Option<DocumentMetadataOverrides>), String> {
    let doc = kwargs.get("input_doc").ok_or("missing input_doc")?;
    let doc: ConsumableDocument = serde_json::from_value(doc.clone()).map_err(|e| format!("invalid input_doc: {e}"))?;
    let overrides = match kwargs.get("overrides") {
        None | Some(Value::Null) => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| format!("invalid overrides: {e}"))?),
    };
    Ok((doc, overrides))
}

/// Handler for `pl_queue::Worker` (`Handlers::register(TASK_NAME, handler(consumer))`).
pub fn handler(consumer: Arc<Consumer>) -> impl pl_queue::TaskHandler + 'static {
    move |ctx: pl_queue::TaskContext| {
        let consumer = consumer.clone();
        async move {
            let (mut doc, overrides) = parse_kwargs(&ctx.kwargs).map_err(|e| pl_queue::TaskError::fail("ValueError", e))?;
            if doc.mime_type.is_empty() {
                doc.mime_type = pl_parsers::mime::detect_file(&doc.original_file).unwrap_or_default();
            }
            let doc_file = doc.original_file.clone();
            match consumer.consume_file(doc, overrides, Some(ctx.task_id.clone())).await {
                Ok(outcome) => Ok(outcome.to_value()),
                // tests/chaos C1: database gone – file is still there, retry in 15 s. If enqueueing
                // does not work either (DB still gone), the queue's cleaner redelivers the task
                // after the lease expires.
                Err(e @ crate::consumer::ConsumeError::Transient(_)) => {
                    if ctx.retries >= TRANSIENT_MAX_RETRIES as i32 {
                        // last attempt: finally failed, but do NOT delete the file (the cause was
                        // disk/DB/service, not the file) – nothing is lost. Path only in the log.
                        tracing::error!(
                            target: "paperless.consumer",
                            "Giving up on {} after {} retries; the file is kept at {}",
                            doc_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                            ctx.retries,
                            doc_file.display()
                        );
                        return Err(pl_queue::TaskError::fail(
                            e.error_type(),
                            format!("{e} Retries exhausted; the uploaded file was kept on the server (see the log for its location)."),
                        ));
                    }
                    Err(pl_queue::TaskError::retry(e.error_type(), e.to_string(), Some(transient_countdown(ctx.retries))))
                }
                Err(e) => Err(pl_queue::TaskError::fail(e.error_type(), e.to_string())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kwargs_roundtrip() {
        let doc = ConsumableDocument {
            source: crate::data::DocumentSourceRepr(DocumentSource::ConsumeFolder),
            original_file: "/c/x.pdf".into(),
            root_document_id: None,
            original_path: None,
            mailrule_id: None,
            mime_type: "application/pdf".into(),
        };
        let o = DocumentMetadataOverrides { tag_ids: Some(vec![1]), ..Default::default() };
        let k = consume_kwargs(&doc, &o).unwrap();
        assert_eq!(k["input_doc"]["source"], Value::from(1));
        let (d2, o2) = parse_kwargs(&k).unwrap();
        assert_eq!(d2, doc);
        assert_eq!(o2.unwrap().tag_ids, Some(vec![1]));
        assert_eq!(trigger_source_for(DocumentSource::MailFetch), "email_consume");
    }

    #[test]
    fn transient_backoff() {
        assert_eq!(transient_countdown(0), 15.0);
        assert_eq!(transient_countdown(1), 30.0);
        assert_eq!(transient_countdown(2), 60.0);
        assert_eq!(transient_countdown(99), 240.0);
    }
}
