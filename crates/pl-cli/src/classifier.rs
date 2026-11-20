//! `document_create_classifier` (= task `train_classifier`) with pl-classifier.
//!
//! upstream: src/documents/management/commands/document_create_classifier.py,
//! src/documents/tasks.py (`train_classifier`), src/documents/classifier.py (`train`: data,
//! labels, content in blocks) – v3.4.2. Model file: PaperRust format (pl-classifier §5),
//! `PAPERRUST_CLASSIFIER_MODEL_FILE` or `DATA_DIR/paperrust_classifier.model` (like pl-consumer).

use std::path::PathBuf;
use std::time::Instant;

use sqlx::{PgPool, Row};

use crate::args;
use crate::ctx::Ctx;
use crate::{CliError, CliResult};

pub const MATCH_AUTO: i16 = 6;

pub fn model_file(c: &Ctx) -> PathBuf {
    pl_core::config::classifier_model_file(pl_core::config::var, &c.cfg.data_dir)
}

pub fn match_threshold() -> f64 {
    pl_core::config::var("PAPERLESS_CLASSIFIER_MATCH_THRESHOLD").and_then(|v| v.trim().parse().ok()).unwrap_or(0.3)
}

/// Result like the return value of the task.
pub const NO_AUTO: &str = "No automatic matching items, not training";
pub const TRAINED: &str = "Training completed successfully";
pub const UNCHANGED: &str = "Training data unchanged";

/// `train_classifier(status_callback=…)`. Needs a multi-thread runtime (content is reloaded
/// blocking in the synchronous callback, like upstream in blocks of 1000).
pub async fn train_classifier(c: &Ctx, notify: &mut dyn FnMut(&str)) -> CliResult<&'static str> {
    let pool = &c.pool;
    let path = model_file(c);
    let any_auto: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM documents_tag WHERE matching_algorithm = 6)
             OR EXISTS (SELECT 1 FROM documents_documenttype WHERE matching_algorithm = 6)
             OR EXISTS (SELECT 1 FROM documents_correspondent WHERE matching_algorithm = 6)
             OR EXISTS (SELECT 1 FROM documents_storagepath WHERE matching_algorithm = 6)",
    )
    .fetch_one(pool)
    .await?;
    if !any_auto {
        tracing::info!(target: "paperless.tasks", "{NO_AUTO}");
        if path.exists() {
            tracing::info!(target: "paperless.tasks", "Removing {} so it won't be used", path.display());
            let _ = std::fs::remove_file(&path);
        }
        return Ok(NO_AUTO);
    }
    let key = c.cfg.secret_key.as_bytes().to_vec();
    let mut model = pl_classifier::load_classifier(&path, &key, false)
        .map_err(|e| CliError::Other(e.to_string()))?
        .unwrap_or_default();
    let docs = training_docs(pool).await?;
    let language = pl_classifier::Language::from_ocr_language(&c.cfg.ocr_language);
    let handle = tokio::runtime::Handle::current();
    let pool2 = pool.clone();
    let fetch = move |chunk: &[i64]| -> Vec<Option<String>> {
        let ids: Vec<i32> = chunk.iter().map(|i| *i as i32).collect();
        // runs inside block_in_place (see below)
        let res = handle.block_on(async {
            sqlx::query("SELECT id, content FROM documents_document WHERE id = ANY($1)").bind(&ids).fetch_all(&pool2).await
        });
        let mut map = std::collections::HashMap::new();
        if let Ok(rows) = res {
            for r in rows {
                if let (Ok(id), Ok(content)) = (r.try_get::<i32, _>(0), r.try_get::<String, _>(1)) {
                    map.insert(i64::from(id), content);
                }
            }
        }
        chunk.iter().map(|id| map.remove(id)).collect()
    };
    let trained = tokio::task::block_in_place(|| {
        model.train(docs, language, fetch, &pl_classifier::TrainOptions::default(), Some(notify))
    })
    .map_err(|e| CliError::Other(e.to_string()))?;
    if trained {
        tracing::info!(target: "paperless.tasks", "Saving updated classifier model to {}...", path.display());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        model.save(&path, &key).map_err(|e| CliError::Other(e.to_string()))?;
        Ok(TRAINED)
    } else {
        Ok(UNCHANGED)
    }
}

/// `Document.objects.exclude(tags__is_inbox_tag=True).order_by("pk")` + AUTO labels.
async fn training_docs(pool: &PgPool) -> CliResult<Vec<pl_classifier::TrainingDoc>> {
    let rows = sqlx::query(
        "SELECT d.id, d.modified,
                CASE WHEN c.matching_algorithm = 6 THEN c.id END,
                CASE WHEN t.matching_algorithm = 6 THEN t.id END,
                CASE WHEN s.matching_algorithm = 6 THEN s.id END,
                COALESCE((SELECT array_agg(g.id ORDER BY g.id) FROM documents_document_tags dt
                          JOIN documents_tag g ON g.id = dt.tag_id WHERE dt.document_id = d.id AND g.matching_algorithm = 6), '{}')
         FROM documents_document d
         LEFT JOIN documents_correspondent c ON c.id = d.correspondent_id
         LEFT JOIN documents_documenttype t ON t.id = d.document_type_id
         LEFT JOIN documents_storagepath s ON s.id = d.storage_path_id
         WHERE d.deleted_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM documents_document_tags dt JOIN documents_tag g ON g.id = dt.tag_id
                           WHERE dt.document_id = d.id AND g.is_inbox_tag)
         ORDER BY d.id",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let modified: chrono::DateTime<chrono::Utc> = r.try_get(1)?;
        let tags: Vec<i32> = r.try_get(5)?;
        out.push(pl_classifier::TrainingDoc {
            pk: i64::from(r.try_get::<i32, _>(0)?),
            modified_us: modified.timestamp_micros(),
            correspondent: r.try_get::<Option<i32>, _>(2)?.map(i64::from),
            document_type: r.try_get::<Option<i32>, _>(3)?.map(i64::from),
            storage_path: r.try_get::<Option<i32>, _>(4)?.map(i64::from),
            tags: tags.into_iter().map(i64::from).collect(),
        });
    }
    Ok(out)
}

/// Loaded model as `pl_matching::Classifier` (retagger).
pub fn load_for_matching(c: &Ctx) -> Option<pl_consumer::native::ThresholdClassifier> {
    let key = c.cfg.secret_key.as_bytes().to_vec();
    match pl_classifier::load_classifier(&model_file(c), &key, false) {
        Ok(Some(model)) => Some(pl_consumer::native::ThresholdClassifier { model, threshold: match_threshold() }),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(target: "paperless.classifier", "Unknown error while loading document classification model: {e}");
            None
        }
    }
}

pub async fn command(argv: Vec<String>) -> CliResult {
    args::parse(&argv, &[], 0)?;
    let c = Ctx::connect().await?;
    let start = Instant::now();
    let mut notify = |m: &str| println!("  {m}");
    let result = train_classifier(&c, &mut notify).await;
    match result {
        Ok(_) => {
            println!("✓ Classifier training complete ({:.1}s)", start.elapsed().as_secs_f64());
            Ok(())
        }
        Err(e) => Err(e),
    }
}
