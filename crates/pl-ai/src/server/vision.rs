//! KI-Abschrift (AI transcription) on the server: plan, transcribe, record (docs/OCR-NEXT.md 2.7/2.8).
//! pl-server hooks these into pl-consumer as pl-core `vision::VisionOcr` (`worker::AiVisionOcr`).
//!
//! * [`plan`]: AI on + option on → mode (`vision_mode`, legacy `vision_when`), sensitivity, page limit,
//!   image size. "Mit KI neu lesen" (`requested`) → mode "always".
//! * [`transcribe`]: whole pages and line crops to the image model ([`crate::vision::recognize_items`],
//!   loop guard, budget).
//! * [`record`]: after storing the document the transcript goes to `paperrust_ai_transcript` (original
//!   OCR text next to the AI text, rows of an earlier run replaced) and, if any AI text went into the
//!   content, the document gets one system note starting with [`NOTE_MARKER`] ("KI-Abschrift"). A note is
//!   the least invasive label that survives an export/migration back to paperless-ngx; the content stays
//!   free of markers (search, exports) and the PDF text layer stays from the classic OCR.

use pl_core::AppState;
use pl_core::vision::{ItemResult, ItemStatus, Sensitivity, Transcript, VisionMode, VisionPlan, VisionRequest, VisionResponse};

use crate::llm::LlmClient;
use crate::options::AiOptions;
use crate::prompt_store::PromptKind;
use crate::vision::{PageImage, VisionItem};
use crate::vision_guard::Verdict;

/// First word of the system note that labels a document with AI text.
pub const NOTE_MARKER: &str = "KI-Abschrift";

/// Mode from the option value.
pub fn parse_mode(s: &str) -> VisionMode {
    match s {
        "off" => VisionMode::Off,
        "uncertain_lines" => VisionMode::UncertainLines,
        "always" => VisionMode::Always,
        _ => VisionMode::UncertainPages,
    }
}

/// Sensitivity from the option value.
pub fn parse_sensitivity(s: &str) -> Sensitivity {
    match s {
        "sparing" => Sensitivity::Sparing,
        "thorough" => Sensitivity::Thorough,
        _ => Sensitivity::Normal,
    }
}

/// Pure decision for [`plan`]: AI off/option off → never; `requested` → always; mode off → no.
pub fn plan_decision(enabled: bool, opts: &AiOptions, requested: bool) -> Option<VisionPlan> {
    if !enabled {
        return None;
    }
    let mode = if requested { VisionMode::Always } else { parse_mode(opts.effective_vision_mode()) };
    if mode == VisionMode::Off {
        return None;
    }
    Some(VisionPlan {
        mode,
        sensitivity: parse_sensitivity(&opts.vision_sensitivity),
        max_pages: usize::try_from(opts.vision_limits.max_pages).unwrap_or(usize::MAX).max(1),
        max_side: opts.vision_max_side,
    })
}

/// Plan for a document with the effective configuration.
pub async fn plan(state: &AppState, requested: bool) -> Option<VisionPlan> {
    let cfg = super::effective_config(state).await.ok()?;
    let opts = super::store::load_options(state.db()).await;
    plan_decision(cfg.ai_enabled && opts.vision_enabled, &opts, requested)
}

fn status_of(v: Option<Verdict>) -> ItemStatus {
    match v {
        Some(Verdict::Accepted) => ItemStatus::Accepted,
        Some(Verdict::Loop) => ItemStatus::Loop,
        Some(Verdict::TokenLimit) => ItemStatus::TokenLimit,
        // Markup is an implausible answer like an over-long one; stored as `too_long` because the status
        // set is fixed by the CHECK constraint of migration 0004.
        Some(Verdict::TooLong | Verdict::Markup) => ItemStatus::TooLong,
        Some(Verdict::Empty) => ItemStatus::Empty,
        None => ItemStatus::Skipped,
    }
}

/// Transcribe pages and crops. `Ok(None)` if AI or the option is off.
pub async fn transcribe(state: &AppState, req: VisionRequest) -> Result<Option<VisionResponse>, String> {
    let cfg = super::effective_config(state).await.map_err(|e| e.status().to_string())?;
    let opts = super::store::load_options(state.db()).await;
    if !cfg.ai_enabled || !opts.vision_enabled {
        return Ok(None);
    }
    let prompts = super::store::load_prompts(state.db()).await;
    let lang = super::store::language_name(&cfg, None);
    let prompt = crate::prompt_store::render(prompts.get(PromptKind::VisionOcr), &[("language", &lang)]);
    let client = LlmClient::from_config(&cfg).map_err(|e| e.to_string())?.with_model(&opts.vision_model);
    let mut items: Vec<VisionItem> = req
        .pages
        .into_iter()
        .map(|p| VisionItem {
            page: p.page,
            line: None,
            image: PageImage { mime: p.image.mime, data: p.image.data },
            ocr_chars: p.ocr_chars,
            area_fraction: 1.0,
        })
        .collect();
    items.extend(req.crops.into_iter().map(|c| VisionItem {
        page: c.page,
        line: Some(c.line),
        ocr_chars: Some(c.ocr_text.chars().filter(|ch| !ch.is_whitespace()).count()),
        image: PageImage { mime: c.image.mime, data: c.image.data },
        area_fraction: c.area_fraction,
    }));
    let out = crate::vision::recognize_items(&client, &prompt, &items, &opts.vision_limits).await.map_err(|e| e.to_string())?;
    Ok(Some(VisionResponse {
        model: client.model.clone(),
        remote: !crate::vision::endpoint_is_local(&client.endpoint),
        items: out.into_iter().map(|o| ItemResult { page: o.page, line: o.line, status: status_of(o.verdict), text: o.text }).collect(),
    }))
}

fn status_str(s: ItemStatus) -> &'static str {
    match s {
        ItemStatus::Accepted => "accepted",
        ItemStatus::Loop => "loop",
        ItemStatus::TokenLimit => "token_limit",
        ItemStatus::TooLong => "too_long",
        ItemStatus::Empty => "empty",
        ItemStatus::Skipped => "skipped",
    }
}

fn mode_str(m: VisionMode) -> &'static str {
    match m {
        VisionMode::Off => "off",
        VisionMode::UncertainLines => "uncertain_lines",
        VisionMode::UncertainPages => "uncertain_pages",
        VisionMode::Always => "always",
    }
}

/// Text of the label note, e.g. "KI-Abschrift (Modell qwen3.5:4b): Seite 2 ganz; Seite 1: 2 Zeilen. …".
pub fn note_text(t: &Transcript) -> String {
    use std::collections::BTreeMap;
    let mut whole: Vec<usize> = Vec::new();
    let mut lines: BTreeMap<usize, usize> = BTreeMap::new();
    for e in t.entries.iter().filter(|e| e.status == ItemStatus::Accepted) {
        match e.line {
            None => whole.push(e.page),
            Some(_) => *lines.entry(e.page).or_default() += 1,
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if !whole.is_empty() {
        let p = whole.iter().map(usize::to_string).collect::<Vec<_>>().join(", ");
        parts.push(if whole.len() == 1 { format!("Seite {p} ganz") } else { format!("Seiten {p} ganz") });
    }
    for (p, n) in &lines {
        parts.push(format!("Seite {p}: {n} {}", if *n == 1 { "Zeile" } else { "Zeilen" }));
    }
    format!(
        "{NOTE_MARKER} (AI transcript, Modell {}): {}. Dieser Text wurde von einem KI-Bildmodell gelesen und kann Fehler \
         enthalten. Die markierbare Textebene im PDF stammt weiter aus der normalen Texterkennung; der ursprüngliche \
         OCR-Text ist gespeichert.",
        t.model,
        parts.join("; ")
    )
}

/// Stores the transcript and labels the document (errors are logged, never passed on).
pub async fn record(state: &AppState, document_id: i64, t: Transcript) {
    if let Err(e) = record_inner(state.db(), document_id, &t).await {
        tracing::warn!(target: "paperless.ai", document_id, error = %e, "could not store the AI transcript");
    }
}

async fn record_inner(db: &sqlx::PgPool, document_id: i64, t: &Transcript) -> Result<(), sqlx::Error> {
    let doc = i32::try_from(document_id).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM paperrust_ai_transcript WHERE document_id = $1").bind(doc).execute(&mut *tx).await?;
    let model: String = t.model.chars().take(128).collect();
    for e in &t.entries {
        let bbox = e.bbox.map(|b| serde_json::json!(b));
        sqlx::query(
            "INSERT INTO paperrust_ai_transcript (document_id, model, mode, page, line, bbox, ocr_text, ai_text, status) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(doc)
        .bind(&model)
        .bind(mode_str(t.mode))
        .bind(i32::try_from(e.page).unwrap_or(i32::MAX))
        .bind(e.line.map(|l| i32::try_from(l).unwrap_or(i32::MAX)))
        .bind(bbox)
        .bind(&e.ocr_text)
        .bind(&e.ai_text)
        .bind(status_str(e.status))
        .execute(&mut *tx)
        .await?;
    }
    if t.accepted() > 0 {
        let text = note_text(t);
        // one system note per document: update an earlier one instead of adding another
        let existing: Option<i32> = sqlx::query_scalar(
            "SELECT id FROM documents_note WHERE document_id = $1 AND user_id IS NULL AND note LIKE $2 ORDER BY id DESC LIMIT 1",
        )
        .bind(doc)
        .bind(format!("{NOTE_MARKER} (%"))
        .fetch_optional(&mut *tx)
        .await?;
        match existing {
            Some(id) => {
                sqlx::query("UPDATE documents_note SET note = $1 WHERE id = $2").bind(&text).bind(id).execute(&mut *tx).await?;
            }
            None => {
                let has_format: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
                     WHERE table_schema = current_schema() AND table_name = 'documents_note' AND column_name = 'format')",
                )
                .fetch_one(&mut *tx)
                .await?;
                let sql = if has_format {
                    "INSERT INTO documents_note (note, format, created, document_id, user_id) VALUES ($1, 'plain', now(), $2, NULL)"
                } else {
                    "INSERT INTO documents_note (note, created, document_id, user_id) VALUES ($1, now(), $2, NULL)"
                };
                sqlx::query(sql).bind(&text).bind(doc).execute(&mut *tx).await?;
            }
        }
    }
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use pl_core::vision::TranscriptEntry;

    #[test]
    fn plan_rules() {
        let mut o = AiOptions::default();
        assert!(plan_decision(false, &o, true).is_none());
        // default (legacy "difficult") → whole page if uncertain
        let p = plan_decision(true, &o, false).unwrap();
        assert_eq!((p.mode, p.max_pages, p.max_side), (VisionMode::UncertainPages, 10, 1540));
        o.vision_when = "off".into();
        assert!(plan_decision(true, &o, false).is_none());
        // "Mit KI neu lesen" works even with mode off and reads every page
        assert_eq!(plan_decision(true, &o, true).unwrap().mode, VisionMode::Always);
        o.vision_mode = "uncertain_lines".into();
        o.vision_sensitivity = "thorough".into();
        let p = plan_decision(true, &o, false).unwrap();
        assert_eq!((p.mode, p.sensitivity), (VisionMode::UncertainLines, Sensitivity::Thorough));
    }

    #[test]
    fn note_lists_pages_and_lines() {
        let e = |page, line, status| TranscriptEntry { page, line, bbox: None, ocr_text: String::new(), ai_text: String::new(), status };
        let t = Transcript {
            model: "qwen3.5:4b".into(),
            mode: VisionMode::UncertainLines,
            entries: vec![
                e(2, None, ItemStatus::Accepted),
                e(1, Some(3), ItemStatus::Accepted),
                e(1, Some(5), ItemStatus::Accepted),
                e(3, None, ItemStatus::Loop),
            ],
        };
        let n = note_text(&t);
        assert!(n.starts_with("KI-Abschrift (AI transcript, Modell qwen3.5:4b): Seite 2 ganz; Seite 1: 2 Zeilen."), "{n}");
        assert!(!n.contains("Seite 3"));
    }
}
