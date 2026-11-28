//! PdfCraft integration (child process only, feature `engine`).
//!
//! Building blocks from PdfCraft (https://github.com/storytold/pdfcraft, MIT OR Apache-2.0):
//! `pdfcraft-cos` (lenient reading, repairing, full rewrite), `pdfcraft-annot`
//! (create redaction marks), `pdfcraft-redact` (remove the content under the marks, with its own
//! verification; metadata), `pdfcraft-edit` (text lines with word rectangles for the search),
//! `pdfcraft-optimize` (shrinking).
//!
//! Everything is rewritten **fully** (`write_full`): an incremental save would leave the
//! old objects – and with them the redacted text – in the file.

use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::{Document, SaveOptions, write_full};
use pdfcraft_redact::sanitize::{Hidden, remove_hidden};

use crate::job::{Area, Hit, Op, RedactReport, RepairReport, Reply, ShrinkPreset, ShrinkReport};
use crate::matching;

/// Domain error with user text.
#[derive(Debug)]
pub struct EngineError(pub String);

impl<E: std::fmt::Display> From<E> for EngineError {
    fn from(e: E) -> Self {
        EngineError(e.to_string())
    }
}

type R<T> = Result<T, EngineError>;

/// Maximum size of an input file (do not even read beyond it).
pub const MAX_INPUT: u64 = 1024 * 1024 * 1024;

fn read_input(path: &str) -> R<Vec<u8>> {
    let meta = std::fs::metadata(path).map_err(|_| EngineError("The PDF file could not be read.".into()))?;
    if meta.len() > MAX_INPUT {
        return Err(EngineError("The PDF file is too large.".into()));
    }
    std::fs::read(path).map_err(|_| EngineError("The PDF file could not be read.".into()))
}

fn open(data: Vec<u8>) -> R<Document> {
    Document::open(Arc::new(data)).map_err(|e| EngineError(format!("The PDF could not be opened: {e}")))
}

fn write_output(path: &str, bytes: &[u8]) -> R<()> {
    std::fs::write(path, bytes).map_err(|_| EngineError("The result could not be written.".into()))
}

fn save(doc: &Document, object_streams: bool) -> R<Vec<u8>> {
    let opts = SaveOptions { object_streams, ..SaveOptions::default() };
    write_full(doc, &opts).map_err(|e| EngineError(format!("The PDF could not be saved: {e}")))
}

/// Run a job.
pub fn run(op: Op) -> Reply {
    let r = match op {
        Op::Redact { input, output, areas, terms, remove_metadata, ocr_input } => {
            redact(&input, &output, &areas, &terms, remove_metadata, ocr_input.as_deref()).map(Reply::Redacted)
        }
        Op::Search { input, terms, max_hits, ocr_input } => read_input(&input)
            .and_then(open)
            .and_then(|doc| {
                let ocr = match ocr_input.as_deref() {
                    Some(p) => Some(open(read_input(p)?)?),
                    None => None,
                };
                search_with_ocr(&doc, ocr.as_ref(), &terms, max_hits)
            })
            .map(|(hits, truncated)| Reply::Hits { hits, truncated }),
        Op::Shrink { input, output, preset } => shrink(&input, &output, preset).map(Reply::Shrunk),
        Op::Repair { input, output } => repair(&input, &output).map(Reply::Repaired),
        Op::Office { input, output, format } => {
            crate::office::convert(&input, &output, &format).map(Reply::Converted).map_err(|e| EngineError(e.0))
        }
        Op::Slides { input, output } => crate::office::slides(&input, &output).map(Reply::Converted).map_err(|e| EngineError(e.0)),
        Op::Heic { input, output, max_pixels } => {
            crate::heif::convert(&input, &output, max_pixels).map(Reply::Converted).map_err(|e| EngineError(e.0))
        }
        Op::SelfTest { .. } => Err(EngineError("self test is handled by the tool".into())),
    };
    match r {
        Ok(v) => v,
        Err(EngineError(error)) => Reply::Failed { error },
    }
}

// ------------------------------------------------------------------ Search

/// Search terms on all pages of the text layer, per page across line breaks (see
/// [`matching::page_matches`]: whitespace, terms split over lines, line-end hyphenation).
pub fn search(doc: &Document, terms: &[String], max_hits: usize) -> R<(Vec<Hit>, bool)> {
    let terms: Vec<&str> = terms.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect();
    let mut hits = Vec::new();
    if terms.is_empty() {
        return Ok((hits, false));
    }
    let pages = pdfcraft_model::pages(doc).len();
    for page in 0..pages {
        let lines = pdfcraft_edit::text_lines(doc, page).map_err(|e| EngineError(format!("page {} can't be read: {e}", page + 1)))?;
        let refs: Vec<matching::Line<'_>> = lines.iter().map(|l| matching::Line { text: &l.text, rect: l.rect, ink: &l.ink }).collect();
        for m in matching::page_matches(&refs, &terms) {
            if hits.len() >= max_hits {
                return Ok((hits, true));
            }
            let term = terms.get(m.term).copied().unwrap_or_default().to_string();
            hits.push(Hit { page, term, context: m.context, rects: m.rects, from_ocr: false });
        }
    }
    Ok((hits, false))
}

fn geoms(doc: &Document) -> Vec<matching::PageGeom> {
    pdfcraft_model::pages(doc).iter().map(|p| matching::PageGeom { view: p.view_matrix(doc), size: p.display_size(doc) }).collect()
}

const OCR_MISMATCH: &str = "The OCR text layer (archive PDF) does not match the pages of the original; nothing was changed.";

/// Map rectangles of page `page` between two documents' geometries (fail-closed).
fn map_rects(rects: &[[f64; 4]], page: usize, from: &[matching::PageGeom], to: &[matching::PageGeom]) -> R<Vec<[f64; 4]>> {
    let (Some(f), Some(t)) = (from.get(page), to.get(page)) else { return Err(EngineError(OCR_MISMATCH.into())) };
    rects.iter().map(|r| matching::map_rect(*r, f, t).ok_or_else(|| EngineError(OCR_MISMATCH.into()))).collect()
}

/// Hits in the OCR text layer, rectangles mapped onto the original's pages. The page count must
/// be the same, and every page with a hit must show the same picture (aspect ratio).
fn ocr_hits(doc: &Document, ocr: &Document, terms: &[String], max_hits: usize) -> R<(Vec<Hit>, bool)> {
    let (go, ga) = (geoms(doc), geoms(ocr));
    if go.len() != ga.len() {
        return Err(EngineError(OCR_MISMATCH.into()));
    }
    let (mut hits, truncated) = search(ocr, terms, max_hits)?;
    for h in &mut hits {
        h.rects = map_rects(&h.rects, h.page, &ga, &go)?;
        h.from_ocr = true;
    }
    Ok((hits, truncated))
}

/// Search in the original and – if given – in the OCR text layer of the same pages (archive PDF):
/// **per page both**, so a document with text pages and scanned pages gets the hits of both kinds.
/// Archive hits that coincide with an original hit are reported once.
pub fn search_with_ocr(doc: &Document, ocr: Option<&Document>, terms: &[String], max_hits: usize) -> R<(Vec<Hit>, bool)> {
    let (hits, truncated) = search(doc, terms, max_hits)?;
    let Some(ocr) = ocr else { return Ok((hits, truncated)) };
    let (from_ocr, t2) = ocr_hits(doc, ocr, terms, max_hits)?;
    let mut merged = matching::merge_hits(hits, from_ocr);
    let truncated = truncated || t2 || merged.len() > max_hits;
    merged.truncate(max_hits);
    Ok((merged, truncated))
}

// ------------------------------------------------------------------ Redaction

fn add_mark(doc: &mut Document, page: usize, rects: &[[f64; 4]]) -> R<()> {
    let shape = Shape::Redact { quads: rects.iter().map(|r| rect_quad(*r)).collect(), overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(doc, &NewAnnotation { page, shape, style, contents: String::new(), author: String::new() }, &Meta::default())
        .map_err(|e| EngineError(format!("page {}: the area could not be marked: {e}", page + 1)))?;
    Ok(())
}

const NOT_VERIFIED: &str = "The redaction could not be verified (a search term is still readable); nothing was changed.";

/// Internal hit limit of a redaction (above: error, nothing changed).
const REDACT_MAX_HITS: usize = 10_000;

/// Redact areas and terms. Terms are searched per page in the original's text layer **and** in the
/// OCR text layer (`ocr_input`, archive PDF), every hit of both is redacted on the original (on a
/// scanned page that is the image area under the OCR word boxes). Fail-closed: the written result
/// is searched again (across line breaks), and the same marks are applied to a copy of the OCR text
/// layer which must not contain a term any more either; otherwise nothing is written.
pub fn redact(
    input: &str,
    output: &str,
    areas: &[Area],
    terms: &[String],
    remove_metadata: bool,
    ocr_input: Option<&str>,
) -> R<RedactReport> {
    let mut doc = open(read_input(input)?)?;
    let pages = pdfcraft_model::pages(&doc).len();
    let mut report = RedactReport::default();
    // do not apply existing foreign redaction marks as well: only ours
    if !pdfcraft_redact::marks(&doc).is_empty() {
        pdfcraft_redact::clear_marks(&mut doc, None)?;
    }
    let mut by_page: std::collections::BTreeMap<usize, Vec<[f64; 4]>> = std::collections::BTreeMap::new();
    for a in areas {
        if a.page >= pages {
            return Err(EngineError(format!("page {} does not exist (the PDF has {pages} pages)", a.page + 1)));
        }
        if !matching::valid_rect(&a.rect) {
            return Err(EngineError(format!("page {}: invalid area", a.page + 1)));
        }
        by_page.entry(a.page).or_default().push(matching::padded(a.rect, 0.0));
    }
    // OCR text layer only matters for terms
    let ocr_bytes = match ocr_input {
        Some(p) if terms.iter().any(|t| !t.trim().is_empty()) => Some(read_input(p)?),
        _ => None,
    };
    let ocr = ocr_bytes.as_ref().map(|b| open(b.clone())).transpose()?;
    let (hits, truncated) = search(&doc, terms, REDACT_MAX_HITS)?;
    let (from_ocr, t2) = match &ocr {
        Some(o) => ocr_hits(&doc, o, terms, REDACT_MAX_HITS)?,
        None => (Vec::new(), false),
    };
    if truncated || t2 {
        return Err(EngineError("Too many matches for the search terms; nothing was changed.".into()));
    }
    // every rectangle of both kinds is marked (also duplicates: a slightly different OCR box
    // must not be dropped); the report counts each occurrence once
    for h in hits.iter().chain(&from_ocr) {
        by_page.entry(h.page).or_default().extend(h.rects.iter().copied());
    }
    report.term_hits = matching::merge_hits(hits, from_ocr).len();
    if by_page.is_empty() && !remove_metadata {
        return Err(EngineError("There is nothing to redact (no areas and no matches).".into()));
    }
    for (page, rects) in &by_page {
        add_mark(&mut doc, *page, rects)?;
    }
    if !by_page.is_empty() {
        // PaperRust (C-068): prepare JPEG scans so that only the pixels under the marks
        // are redacted instead of the whole image (see `scanimages`).
        let pages: Vec<usize> = by_page.keys().copied().collect();
        let prepared = crate::scanimages::prepare(&mut doc, &pages);
        let r = pdfcraft_redact::apply(&mut doc, None)?;
        crate::scanimages::finish(&mut doc, prepared);
        report.marks = r.marks;
        report.pages = r.pages;
        report.glyphs = r.glyphs;
        report.images_removed = r.images_removed;
        report.images_cleared = r.images_cleared;
        report.paths_removed = r.paths_removed;
        report.paths_clipped = r.paths_clipped;
        report.annotations_removed = r.annotations;
        report.metadata_removed = r.metadata;
    }
    if remove_metadata {
        // PaperRust (C-066b): no (further) metadata – `apply` already removes the document XMP
        // itself – is reported by PdfCraft as `NothingToApply` ("there are no redaction marks to apply").
        // That is not an error here: there is simply nothing left to remove.
        match remove_hidden(&mut doc, &[Hidden::Metadata]) {
            Ok(removed) => report.metadata_removed += removed.iter().map(|(_, n)| *n).sum::<usize>(),
            Err(pdfcraft_redact::RedactError::NothingToApply) => {}
            Err(e) => return Err(e.into()),
        }
    }
    let bytes = save(&doc, true)?;
    // Verification 1 on the written result: no search term may be findable any more (also not
    // across line breaks).
    if !terms.is_empty() {
        let check = open(bytes.clone())?;
        let (left, _) = search(&check, terms, 1)?;
        if !left.is_empty() {
            return Err(EngineError(NOT_VERIFIED.into()));
        }
    }
    // Verification 2 for scanned pages (no text layer in the result): the same marks, mapped onto
    // the OCR text layer, must remove every OCR occurrence there.
    if let (Some(b), Some(o)) = (ocr_bytes, &ocr) {
        verify_ocr(b, &geoms(&doc), &geoms(o), &by_page, terms)?;
    }
    write_output(output, &bytes)?;
    Ok(report)
}

/// Apply `by_page` (original coordinates) to a fresh copy of the OCR text layer and search it again.
fn verify_ocr(
    ocr_bytes: Vec<u8>,
    g_orig: &[matching::PageGeom],
    g_ocr: &[matching::PageGeom],
    by_page: &std::collections::BTreeMap<usize, Vec<[f64; 4]>>,
    terms: &[String],
) -> R<()> {
    let mut copy = open(ocr_bytes)?;
    if !pdfcraft_redact::marks(&copy).is_empty() {
        pdfcraft_redact::clear_marks(&mut copy, None)?;
    }
    for (page, rects) in by_page {
        add_mark(&mut copy, *page, &map_rects(rects, *page, g_orig, g_ocr)?)?;
    }
    if !by_page.is_empty() {
        pdfcraft_redact::apply(&mut copy, None).map_err(|_| EngineError(NOT_VERIFIED.into()))?;
    }
    let check = open(save(&copy, false)?)?;
    let (left, _) = search(&check, terms, 1)?;
    if !left.is_empty() {
        return Err(EngineError(NOT_VERIFIED.into()));
    }
    Ok(())
}
// ------------------------------------------------------------------ Shrinking

fn settings_for(preset: ShrinkPreset) -> pdfcraft_optimize::Settings {
    use pdfcraft_optimize::{Compression, ImageSettings, MonoCompression, MonoSettings, Preset};
    let mut s = match preset {
        ShrinkPreset::Lossless => {
            let keep = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
            pdfcraft_optimize::Settings {
                color: keep,
                gray: keep,
                mono: MonoSettings { downsample: false, target_ppi: 300.0, above_ppi: 450.0, compression: MonoCompression::Retain },
                ..pdfcraft_optimize::Settings::default()
            }
        }
        ShrinkPreset::Standard => Preset::Standard.settings(),
        ShrinkPreset::Strong => Preset::Mobile.settings(),
    };
    // one thread: the machine is shared, the result is the same with any number
    s.workers = Some(1);
    s
}

/// Shrinking. If the result does not get smaller, it is written anyway; the caller
/// decides based on `size_after` (before/after report).
pub fn shrink(input: &str, output: &str, preset: ShrinkPreset) -> R<ShrinkReport> {
    let data = read_input(input)?;
    let before = data.len() as u64;
    let mut doc = open(data)?;
    let r = pdfcraft_optimize::optimize(&mut doc, &settings_for(preset))?;
    let bytes = save(&doc, true)?;
    write_output(output, &bytes)?;
    Ok(ShrinkReport {
        preset: preset.name().into(),
        size_before: before,
        size_after: bytes.len() as u64,
        images: r.images,
        images_resampled: r.images_resampled,
        images_recompressed: r.images_recompressed,
        unused_objects_removed: r.unused_xobjects,
        streams_compressed: r.streams_compressed,
    })
}

// ------------------------------------------------------------------ Repair

/// Lenient reading (rebuild cross references, skip garbage) and classic rewrite
/// (cross-reference table without object streams, every reader can read it).
pub fn repair(input: &str, output: &str) -> R<RepairReport> {
    let data = read_input(input)?;
    let before = data.len() as u64;
    let doc = open(data)?;
    if pdfcraft_model::pages(&doc).is_empty() {
        return Err(EngineError("The PDF has no readable pages.".into()));
    }
    let log = doc.repair_log().to_vec();
    let bytes = save(&doc, false)?;
    write_output(output, &bytes)?;
    Ok(RepairReport { log, size_before: before, size_after: bytes.len() as u64 })
}
