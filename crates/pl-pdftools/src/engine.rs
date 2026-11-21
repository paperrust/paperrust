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
        Op::Redact { input, output, areas, terms, remove_metadata } => {
            redact(&input, &output, &areas, &terms, remove_metadata).map(Reply::Redacted)
        }
        Op::Search { input, terms, max_hits } => read_input(&input)
            .and_then(open)
            .and_then(|doc| search(&doc, &terms, max_hits))
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

/// Search terms on all pages. Matches within one line (terms across a
/// line break are not found).
pub fn search(doc: &Document, terms: &[String], max_hits: usize) -> R<(Vec<Hit>, bool)> {
    let terms: Vec<&str> = terms.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect();
    let mut hits = Vec::new();
    if terms.is_empty() {
        return Ok((hits, false));
    }
    let pages = pdfcraft_model::pages(doc).len();
    for page in 0..pages {
        let lines = pdfcraft_edit::text_lines(doc, page).map_err(|e| EngineError(format!("page {} can't be read: {e}", page + 1)))?;
        for line in &lines {
            for term in &terms {
                for range in matching::find_ranges(&line.text, term) {
                    if hits.len() >= max_hits {
                        return Ok((hits, true));
                    }
                    let rects = matching::rects_for(&line.text, line.rect, &line.ink, range)
                        .into_iter()
                        .map(|r| matching::padded(r, 0.5))
                        .collect();
                    hits.push(Hit { page, term: (*term).to_string(), context: line.text.chars().take(200).collect(), rects });
                }
            }
        }
    }
    Ok((hits, false))
}

// ------------------------------------------------------------------ Redaction

fn add_mark(doc: &mut Document, page: usize, rects: &[[f64; 4]]) -> R<()> {
    let shape = Shape::Redact { quads: rects.iter().map(|r| rect_quad(*r)).collect(), overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(doc, &NewAnnotation { page, shape, style, contents: String::new(), author: String::new() }, &Meta::default())
        .map_err(|e| EngineError(format!("page {}: the area could not be marked: {e}", page + 1)))?;
    Ok(())
}

/// Redact areas and terms. Fail-closed: if the verification afterwards still finds a term,
/// nothing is written.
pub fn redact(input: &str, output: &str, areas: &[Area], terms: &[String], remove_metadata: bool) -> R<RedactReport> {
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
    let (hits, truncated) = search(&doc, terms, 10_000)?;
    if truncated {
        return Err(EngineError("Too many matches for the search terms; nothing was changed.".into()));
    }
    report.term_hits = hits.len();
    for h in &hits {
        by_page.entry(h.page).or_default().extend(h.rects.iter().copied());
    }
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
    // Verification on the written result: no search term may be findable any more.
    if !terms.is_empty() {
        let check = open(bytes.clone())?;
        let (left, _) = search(&check, terms, 1)?;
        if !left.is_empty() {
            return Err(EngineError("The redaction could not be verified (a search term is still readable); nothing was changed.".into()));
        }
    }
    write_output(output, &bytes)?;
    Ok(report)
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
