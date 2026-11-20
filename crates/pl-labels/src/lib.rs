//! pl-labels – ASN label generator (PaperRust extension, no upstream counterpart).
//!
//! Produces print-ready label sheets with ASN barcodes (`<prefix><number>`, e.g. `ASN00042`) and
//! separator sheets (`PAPERLESS_CONSUMER_BARCODE_STRING`, default `PATCHT`) as vector PDF plus an
//! SVG preview from the same draw list. The consumer (pl-pdfops `barcodes`, like upstream
//! `documents/barcodes.py@v3.4.2`) reads these codes: with `PAPERLESS_CONSUMER_ENABLE_BARCODES` +
//! `PAPERLESS_CONSUMER_ENABLE_ASN_BARCODE` every page with an ASN label (except page 1) starts a
//! new document and the document gets the number (`BarcodePlugin.get_separation_pages`/`asn`).
//!
//! Modules: [`sheets`] (templates/grid), [`codes`] (symbols), [`render`] (draw list),
//! [`pdf`]/[`svg`] (output), [`store`] (`paperrust_asn_print`),
//! [`ranges`] (optional number ranges with storage location, `paperrust_asn_range`/`paperrust_asn_config`). Contract: docs/contract/pl-labels.md.

pub mod codes;
pub mod font;
pub mod pdf;
pub mod ranges;
pub mod render;
pub mod sheets;
pub mod store;
pub mod svg;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub use codes::CodeType;
use font::Face;
use render::{Anchor, LabelMetrics, Page, Prim, Warning};
pub use sheets::SheetLayout;

/// `Document.ARCHIVE_SERIAL_NUMBER_MAX` (upstream validator, 0xFFFFFFFF).
pub const ARCHIVE_SERIAL_NUMBER_MAX: u64 = 0xFFFF_FFFF;
/// Largest storable ASN: the column `documents_document.archive_serial_number` is
/// PostgreSQL `integer` (Django `PositiveIntegerField`), so at most 2³¹−1.
pub const ASN_MAX_STORABLE: u64 = i32::MAX as u64;
/// Limits per job.
pub const MAX_LABELS: u32 = 20_000;
pub const MAX_PAGES: u32 = 200;
pub const MAX_OFFSET_MM: f64 = 20.0;
pub const DEFAULT_DIGITS: u32 = 5;

/// Error (field, message) – the API turns it into 400/422 with `fields`.
#[derive(Debug, thiserror::Error)]
pub enum LabelError {
    #[error("{0}: {1}")]
    Invalid(String, String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub type Result<T, E = LabelError> = std::result::Result<T, E>;

fn invalid<T>(field: &str, msg: impl Into<String>) -> Result<T> {
    Err(LabelError::Invalid(field.into(), msg.into()))
}

/// Job as the API accepts it (missing values are filled by [`LabelJob::resolve`]).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LabelJob {
    /// Template identifier (`sheets::presets`); `custom` or empty + `sheet` = own grid.
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub sheet: Option<SheetLayout>,
    #[serde(default)]
    pub code: CodeType,
    /// Number as plain text on the label (default on).
    #[serde(default)]
    pub show_text: Option<bool>,
    /// Prefix (default: `PAPERLESS_CONSUMER_ASN_BARCODE_PREFIX`).
    #[serde(default)]
    pub prefix: Option<String>,
    /// Digits of the number (zero-padded), 1–10, default 5.
    #[serde(default)]
    pub digits: Option<u32>,
    /// First ASN (default: next free).
    #[serde(default)]
    pub start: Option<u64>,
    /// Number of labels …
    #[serde(default)]
    pub count: Option<u32>,
    /// … or number of sheets (with neither: one sheet).
    #[serde(default)]
    pub sheets: Option<u32>,
    /// Already used positions on the first sheet (0-based, row by row).
    #[serde(default)]
    pub skip: Vec<u32>,
    /// Skip the first n positions of the first sheet (short form of `skip`).
    #[serde(default)]
    pub start_offset: Option<u32>,
    /// Printer correction in mm (+ = to the right/down).
    #[serde(default)]
    pub offset_x: f64,
    #[serde(default)]
    pub offset_y: f64,
    /// Also print label outlines (test print on plain paper).
    #[serde(default)]
    pub outline: bool,
    /// Number range (`paperrust_asn_range.id`, only with number ranges enabled): start suggestion
    /// within the range, range limits are checked, extra line `<abbreviation> · <storage location>`. The API evaluates
    /// this; [`LabelJob::resolve`] ignores it.
    #[serde(default)]
    pub range: Option<i64>,
}

/// Checked job.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub sheet: SheetLayout,
    pub code: CodeType,
    pub show_text: bool,
    pub prefix: String,
    pub digits: u32,
    pub start: u64,
    pub count: u32,
    pub skip: BTreeSet<u32>,
    pub offset_x: f64,
    pub offset_y: f64,
    pub outline: bool,
    /// Small extra line below the number (number range: `<abbreviation> · <storage location>`), only if there is room.
    /// The barcode content stays unchanged.
    pub caption: Option<String>,
}

/// Checks an ASN prefix (printable ASCII, 1–32 characters, no leading/trailing
/// spaces – the consumer compares with `startswith`).
pub fn validate_prefix(p: &str) -> Result<()> {
    if p.is_empty() || p.len() > 32 {
        return invalid("prefix", "The prefix must have 1–32 characters.");
    }
    if !p.chars().all(|c| (' '..='~').contains(&c)) {
        return invalid("prefix", "The prefix may only contain printable ASCII characters.");
    }
    if p.trim() != p {
        return invalid("prefix", "The prefix must not start or end with a space.");
    }
    Ok(())
}

/// Content of a label: prefix + number zero-padded to `digits` places.
pub fn content(prefix: &str, digits: u32, asn: u64) -> String {
    format!("{prefix}{asn:0width$}", width = digits as usize)
}

impl LabelJob {
    /// Fills in defaults and checks everything. `default_prefix` = consumer setting,
    /// `default_start` = next free ASN.
    pub fn resolve(&self, default_prefix: &str, default_start: u64) -> Result<Resolved> {
        let sheet = match (self.preset.as_deref(), &self.sheet) {
            (Some(id), _) if id != "custom" && !id.is_empty() => match sheets::preset(id) {
                Some(s) => s,
                None => return invalid("preset", format!("Unknown label sheet {id:?}.")),
            },
            (_, Some(s)) => {
                let mut s = s.clone();
                s.id = "custom".into();
                if s.name.is_empty() {
                    s.name = "Custom".into();
                }
                s
            }
            (Some(_), None) => return invalid("sheet", "A custom layout needs the sheet dimensions."),
            (None, None) => sheets::preset(sheets::DEFAULT_PRESET).ok_or_else(|| LabelError::Invalid("preset".into(), "missing default".into()))?,
        };
        sheet.validate()?;
        let prefix = self.prefix.clone().unwrap_or_else(|| default_prefix.to_string());
        validate_prefix(&prefix)?;
        let digits = self.digits.unwrap_or(DEFAULT_DIGITS);
        if !(1..=10).contains(&digits) {
            return invalid("digits", "Must be between 1 and 10.");
        }
        for (f, v) in [("offset_x", self.offset_x), ("offset_y", self.offset_y)] {
            if !v.is_finite() || v.abs() > MAX_OFFSET_MM {
                return invalid(f, format!("Must be between -{MAX_OFFSET_MM} and {MAX_OFFSET_MM} mm."));
            }
        }
        let per = sheet.per_sheet();
        let mut skip: BTreeSet<u32> = BTreeSet::new();
        for &i in &self.skip {
            if i >= per {
                return invalid("skip", format!("Label position {i} does not exist on this sheet (0–{}).", per - 1));
            }
            skip.insert(i);
        }
        if let Some(n) = self.start_offset {
            if n >= per {
                return invalid("start_offset", format!("Must be less than {per} (labels per sheet)."));
            }
            skip.extend(0..n);
        }
        let free_first = per - skip.len() as u32;
        if free_first == 0 {
            return invalid("skip", "All labels of the first sheet are marked as used.");
        }
        let count = match (self.count, self.sheets) {
            (Some(_), Some(_)) => return invalid("count", "Give either count or sheets, not both."),
            (Some(c), None) => c,
            (None, Some(s)) => {
                if s == 0 || s > MAX_PAGES {
                    return invalid("sheets", format!("Must be between 1 and {MAX_PAGES}."));
                }
                free_first + (s - 1) * per
            }
            (None, None) => free_first,
        };
        if count == 0 || count > MAX_LABELS {
            return invalid("count", format!("Must be between 1 and {MAX_LABELS}."));
        }
        let start = self.start.unwrap_or(default_start);
        let last = start + u64::from(count) - 1;
        if last > ASN_MAX_STORABLE {
            return invalid("start", format!("The last number ({last}) would exceed the largest storable ASN ({ASN_MAX_STORABLE})."));
        }
        let r = Resolved {
            sheet,
            code: self.code,
            show_text: self.show_text.unwrap_or(true),
            prefix,
            digits,
            start,
            count,
            skip,
            offset_x: self.offset_x,
            offset_y: self.offset_y,
            outline: self.outline,
            caption: None,
        };
        if r.pages() > MAX_PAGES {
            return invalid("count", format!("At most {MAX_PAGES} sheets per print job."));
        }
        // Content must be encodable (e.g. Code 128 only ASCII)
        codes::encode(r.code, &content(&r.prefix, r.digits, r.last()))?;
        Ok(r)
    }
}

/// Occupancy: per page (position, ASN).
pub type Plan = Vec<Vec<(u32, u64)>>;

impl Resolved {
    pub fn last(&self) -> u64 {
        self.start + u64::from(self.count) - 1
    }

    pub fn content(&self, asn: u64) -> String {
        content(&self.prefix, self.digits, asn)
    }

    /// Number of sheets.
    pub fn pages(&self) -> u32 {
        let per = self.sheet.per_sheet();
        let first = per - self.skip.len() as u32;
        if self.count <= first { 1 } else { 1 + (self.count - first).div_ceil(per) }
    }

    /// Which number at which position: first sheet without `skip`, then all positions.
    pub fn plan(&self) -> Plan {
        let per = self.sheet.per_sheet();
        let mut pages: Plan = Vec::new();
        let mut asn = self.start;
        let mut left = self.count;
        let mut page = 0;
        while left > 0 {
            let mut cur = Vec::new();
            for slot in 0..per {
                if left == 0 {
                    break;
                }
                if page == 0 && self.skip.contains(&slot) {
                    continue;
                }
                cur.push((slot, asn));
                asn += 1;
                left -= 1;
            }
            pages.push(cur);
            page += 1;
        }
        pages
    }

    /// Draws the pages (`only_page` = only this one, for the preview). `preview` draws all
    /// label outlines (also empty positions). Returns pages + hints.
    pub fn render(&self, only_page: Option<usize>, preview: bool) -> Result<(Vec<Page>, Vec<Warning>)> {
        let plan = self.plan();
        let mut pages = Vec::new();
        let mut metrics: Option<LabelMetrics> = None;
        let text_on = self.show_text;
        for (pi, slots) in plan.iter().enumerate() {
            if only_page.is_some_and(|p| p != pi) {
                continue;
            }
            let mut prims = Vec::new();
            let (dx, dy) = (self.offset_x, self.offset_y);
            if preview || self.outline {
                for s in self.sheet.slots() {
                    prims.push(Prim::Outline { x: s.x + dx, y: s.y + dy, w: s.w, h: s.h, r: self.sheet.corner_radius });
                }
            }
            for &(slot, asn) in slots {
                let s = self.sheet.slot(slot);
                let text = self.content(asn);
                let sym = codes::encode(self.code, &text)?;
                let m = render::draw_label(
                    &mut prims,
                    (s.x + dx, s.y + dy, s.w, s.h),
                    &sym,
                    text_on.then_some(text.as_str()),
                    self.caption.as_deref().filter(|c| !c.is_empty()),
                );
                // the longest content (last number) determines the hints
                if metrics.is_none_or(|old| m.module <= old.module) {
                    metrics = Some(m);
                }
            }
            pages.push(Page { width: self.sheet.page_width, height: self.sheet.page_height, prims });
        }
        let warnings = match metrics {
            Some(m) => render::metrics_warnings(self.code, &m, text_on),
            None => Vec::new(),
        };
        Ok((pages, warnings))
    }

    /// Finished PDF.
    pub fn pdf(&self) -> Result<Vec<u8>> {
        let (pages, _) = self.render(None, false)?;
        Ok(pdf::write(&pages, &format!("ASN labels {} – {}", self.content(self.start), self.content(self.last()))))
    }

    /// SVG preview of a page (0-based).
    pub fn preview_svg(&self, page: usize) -> Result<(String, Vec<Warning>)> {
        let (pages, warnings) = self.render(Some(page), true)?;
        let page = pages.into_iter().next().ok_or_else(|| LabelError::Invalid("page".into(), "No such page.".into()))?;
        Ok((svg::write(&page), warnings))
    }
}

/// Separator sheet job.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeparatorJob {
    /// `a4` (default) or `letter`.
    #[serde(default)]
    pub paper: Option<String>,
    #[serde(default)]
    pub code: CodeType,
    /// Number of sheets (1–50, default 1).
    #[serde(default)]
    pub copies: Option<u32>,
    /// Language of the explanation: `de` or `en` (default).
    #[serde(default)]
    pub lang: Option<String>,
    /// Content (default: `PAPERLESS_CONSUMER_BARCODE_STRING`). Only superusers may deviate
    /// (checked by the API) – otherwise the sheet is ineffective for this server.
    #[serde(default)]
    pub value: Option<String>,
}

/// Texts of the separator sheet.
struct SepTexts {
    title: &'static str,
    body: String,
    footer: &'static str,
}

fn sep_texts(lang: &str, value: &str, retain: bool) -> SepTexts {
    if lang == "de" {
        SepTexts {
            title: "Trennblatt",
            body: format!(
                "Legen Sie dieses Blatt im Scanner-Stapel jeweils vor ein neues Dokument. PaperRust teilt den Scan an diesem Barcode („{value}“) auf. {} Voraussetzung: Barcode-Aufteilung ist eingeschaltet (PAPERRUST_CONSUMER_ENABLE_BARCODES). Das Blatt ist wiederverwendbar – bitte nicht beschriften oder knicken.",
                if retain { "Das Trennblatt bleibt als erste Seite des neuen Dokuments erhalten." } else { "Das Trennblatt selbst wird verworfen." }
            ),
            footer: "Mit 100 % (Originalgröße) drucken – nicht „An Seite anpassen“. Erstellt mit PaperRust.",
        }
    } else {
        SepTexts {
            title: "Separator sheet",
            body: format!(
                "Put this sheet in front of each new document in the scanner stack. PaperRust splits the scan at this barcode (\"{value}\"). {} Requirement: barcode splitting is enabled (PAPERRUST_CONSUMER_ENABLE_BARCODES). The sheet is reusable – please do not write on it or fold it.",
                if retain { "The separator sheet is kept as the first page of the new document." } else { "The separator sheet itself is discarded." }
            ),
            footer: "Print at 100 % (actual size) – not \"fit to page\". Created with PaperRust.",
        }
    }
}

impl SeparatorJob {
    /// Pages of the separator sheet. `default_value` = configured separator string, `retain` =
    /// `BARCODE_RETAIN_SPLIT_PAGES` (for the explanatory text only).
    pub fn pages(&self, default_value: &str, retain: bool) -> Result<Vec<Page>> {
        let (pw, ph) = match self.paper.as_deref().unwrap_or("a4") {
            "a4" => sheets::A4,
            "letter" => sheets::LETTER,
            other => return invalid("paper", format!("Unknown paper size {other:?} (a4, letter).")),
        };
        let copies = self.copies.unwrap_or(1);
        if !(1..=50).contains(&copies) {
            return invalid("copies", "Must be between 1 and 50.");
        }
        let lang = self.lang.as_deref().unwrap_or("en");
        if lang != "de" && lang != "en" {
            return invalid("lang", "Must be de or en.");
        }
        let value = self.value.clone().unwrap_or_else(|| default_value.to_string());
        if value.is_empty() || value.len() > 64 {
            return invalid("value", "The separator value must have 1–64 characters.");
        }
        let sym = codes::encode(self.code, &value)?;
        let t = sep_texts(lang, &value, retain);
        let mut prims = Vec::new();
        let cx = pw / 2.0;
        prims.push(Prim::Text { x: cx, y: 32.0, size: 30.0, face: Face::Bold, anchor: Anchor::Middle, text: t.title.into() });
        prims.push(Prim::Text { x: cx, y: 42.0, size: 11.0, face: Face::Regular, anchor: Anchor::Middle, text: "PaperRust".into() });
        // large code centred
        let code_bottom = if sym.kind.is_1d() {
            let w = 150.0;
            let module = w / f64::from(sym.width + 2 * sym.kind.quiet_modules());
            let x = cx - w / 2.0 + f64::from(sym.kind.quiet_modules()) * module;
            prims.push(Prim::Modules { x, y: 70.0, mx: module, my: 45.0, runs: sym.runs() });
            115.0
        } else {
            let side = 80.0;
            let module = side / f64::from(sym.width);
            prims.push(Prim::Modules { x: cx - side / 2.0, y: 60.0, mx: module, my: module, runs: sym.runs() });
            140.0
        };
        prims.push(Prim::Text { x: cx, y: code_bottom + 14.0, size: 22.0, face: Face::Bold, anchor: Anchor::Middle, text: value.clone() });
        // Explanation
        let size = 11.0;
        let max_em = render::mm_to_pt(pw - 50.0) / size;
        let mut y = code_bottom + 32.0;
        for line in font::wrap(Face::Regular, &t.body, max_em) {
            prims.push(Prim::Text { x: 25.0, y, size, face: Face::Regular, anchor: Anchor::Start, text: line });
            y += 5.6;
        }
        prims.push(Prim::Line { x1: 20.0, y1: ph - 22.0, x2: pw - 20.0, y2: ph - 22.0 });
        prims.push(Prim::Text { x: cx, y: ph - 15.0, size: 8.5, face: Face::Regular, anchor: Anchor::Middle, text: t.footer.into() });
        let page = Page { width: pw, height: ph, prims };
        Ok(vec![page; copies as usize])
    }

    pub fn pdf(&self, default_value: &str, retain: bool) -> Result<Vec<u8>> {
        let pages = self.pages(default_value, retain)?;
        Ok(pdf::write(&pages, "PaperRust separator sheet"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> LabelJob {
        LabelJob { preset: Some("avery-4780".into()), ..Default::default() }
    }

    #[test]
    fn content_is_prefix_and_padded_number() {
        assert_eq!(content("ASN", 5, 42), "ASN00042");
        assert_eq!(content("ASN", 5, 1234567), "ASN1234567");
        assert_eq!(content("DOC-", 7, 1), "DOC-0000001");
        // the consumer reads exactly this number back (pl-pdfops `located_asn`, like upstream)
        let cfg = pl_like_asn("ASN00042", "ASN");
        assert_eq!(cfg, Some(42));
    }

    /// Modelled on `BarcodePlugin.asn` (strip prefix, strip, remove non-digits, int).
    fn pl_like_asn(value: &str, prefix: &str) -> Option<u64> {
        let rest = value.strip_prefix(prefix)?.trim();
        rest.chars().filter(char::is_ascii_digit).collect::<String>().parse().ok()
    }

    #[test]
    fn defaults_fill_one_sheet() {
        let r = job().resolve("ASN", 1001).unwrap();
        assert_eq!((r.start, r.count, r.last(), r.pages()), (1001, 40, 1040, 1));
        assert_eq!(r.prefix, "ASN");
        assert!(r.show_text);
        let plan = r.plan();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].first(), Some(&(0, 1001)));
        assert_eq!(plan[0].last(), Some(&(39, 1040)));
    }

    #[test]
    fn skip_and_offset_on_first_sheet_only() {
        let j = LabelJob { skip: vec![5, 1], start_offset: Some(3), sheets: Some(2), start: Some(7), ..job() };
        let r = j.resolve("ASN", 1).unwrap();
        // Positions 0,1,2 (offset) and 5 occupied → 36 free on sheet 1, plus 40 on sheet 2
        assert_eq!(r.count, 36 + 40);
        let plan = r.plan();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0][0], (3, 7));
        assert_eq!(plan[0][1], (4, 8));
        assert_eq!(plan[0][2], (6, 9));
        assert_eq!(plan[1][0], (0, 7 + 36));
        assert_eq!(plan[1].len(), 40);
        // consecutive without gaps
        let all: Vec<u64> = plan.iter().flatten().map(|x| x.1).collect();
        assert!(all.windows(2).all(|w| w[1] == w[0] + 1));
    }

    #[test]
    fn count_spills_to_more_pages() {
        let r = LabelJob { count: Some(41), ..job() }.resolve("ASN", 1).unwrap();
        assert_eq!(r.pages(), 2);
        assert_eq!(r.plan()[1], vec![(0, 41)]);
        let r = LabelJob { count: Some(40), start_offset: Some(1), ..job() }.resolve("ASN", 1).unwrap();
        assert_eq!(r.pages(), 2);
    }

    #[test]
    fn validation_errors() {
        let e = |j: LabelJob| match j.resolve("ASN", 1) {
            Err(LabelError::Invalid(f, _)) => f,
            other => panic!("{other:?}"),
        };
        assert_eq!(e(LabelJob { preset: Some("nope".into()), ..Default::default() }), "preset");
        assert_eq!(e(LabelJob { skip: vec![40], ..job() }), "skip");
        assert_eq!(e(LabelJob { skip: (0..40).collect(), ..job() }), "skip");
        assert_eq!(e(LabelJob { count: Some(0), ..job() }), "count");
        assert_eq!(e(LabelJob { count: Some(1), sheets: Some(1), ..job() }), "count");
        assert_eq!(e(LabelJob { digits: Some(11), ..job() }), "digits");
        assert_eq!(e(LabelJob { prefix: Some("".into()), ..job() }), "prefix");
        assert_eq!(e(LabelJob { prefix: Some("ÄSN".into()), ..job() }), "prefix");
        assert_eq!(e(LabelJob { prefix: Some("ASN ".into()), ..job() }), "prefix");
        assert_eq!(e(LabelJob { offset_x: 25.0, ..job() }), "offset_x");
        assert_eq!(e(LabelJob { start: Some(ASN_MAX_STORABLE), count: Some(2), ..job() }), "start");
        assert_eq!(e(LabelJob { sheets: Some(MAX_PAGES + 1), ..job() }), "sheets");
        assert_eq!(e(LabelJob { preset: Some("custom".into()), ..Default::default() }), "sheet");
        // exactly at the limit it works
        LabelJob { start: Some(ASN_MAX_STORABLE), count: Some(1), ..job() }.resolve("ASN", 1).unwrap();
    }

    #[test]
    fn custom_sheet_and_calibration_offset() {
        let sheet = SheetLayout {
            id: String::new(),
            name: String::new(),
            vendor: String::new(),
            paper: "a4".into(),
            page_width: 210.0,
            page_height: 297.0,
            label_width: 60.0,
            label_height: 30.0,
            cols: 3,
            rows: 9,
            margin_left: 10.0,
            margin_top: 13.5,
            gap_x: 5.0,
            gap_y: 0.0,
            corner_radius: 0.0,
            verified: false,
            source: String::new(),
        };
        let r = LabelJob { sheet: Some(sheet), offset_x: 1.5, offset_y: -0.5, outline: true, count: Some(2), ..Default::default() }
            .resolve("ASN", 1)
            .unwrap();
        assert_eq!(r.sheet.id, "custom");
        let (pages, _) = r.render(None, false).unwrap();
        // Outlines of all 27 positions, shifted by the correction
        let outlines: Vec<_> = pages[0].prims.iter().filter(|p| matches!(p, Prim::Outline { .. })).collect();
        assert_eq!(outlines.len(), 27);
        assert!(matches!(outlines[0], Prim::Outline { x, y, .. } if (*x - 11.5).abs() < 1e-9 && (*y - 13.0).abs() < 1e-9));
        let mods = pages[0].prims.iter().filter(|p| matches!(p, Prim::Modules { .. })).count();
        assert_eq!(mods, 2);
    }

    #[test]
    fn pdf_and_preview_render() {
        for code in [CodeType::Qr, CodeType::DataMatrix, CodeType::Code128] {
            let r = LabelJob { preset: Some("avery-l4731".into()), code, ..Default::default() }.resolve("ASN", 1).unwrap();
            let pdf = r.pdf().unwrap();
            assert!(pdf.starts_with(b"%PDF-1.4"));
            let (svg, warnings) = r.preview_svg(0).unwrap();
            assert_eq!(svg.matches("<path").count(), 189);
            if code == CodeType::Code128 {
                assert!(warnings.iter().any(|w| w.code == "module_too_small"), "{warnings:?}");
            } else {
                assert!(warnings.is_empty(), "{code:?}: {warnings:?}");
            }
            assert!(r.preview_svg(1).is_err());
        }
    }

    #[test]
    fn separator_sheet() {
        let j = SeparatorJob { copies: Some(3), lang: Some("de".into()), ..Default::default() };
        let pages = j.pages("PATCHT", false).unwrap();
        assert_eq!(pages.len(), 3);
        let texts: Vec<&str> = pages[0]
            .prims
            .iter()
            .filter_map(|p| if let Prim::Text { text, .. } = p { Some(text.as_str()) } else { None })
            .collect();
        assert!(texts.contains(&"Trennblatt") && texts.contains(&"PATCHT"));
        assert!(j.pdf("PATCHT", true).unwrap().starts_with(b"%PDF"));
        assert!(SeparatorJob { paper: Some("a5".into()), ..Default::default() }.pages("PATCHT", false).is_err());
        assert!(SeparatorJob { copies: Some(0), ..Default::default() }.pages("PATCHT", false).is_err());
        let letter = SeparatorJob { paper: Some("letter".into()), code: CodeType::Code128, ..Default::default() }.pages("PATCHT", false).unwrap();
        assert_eq!(letter[0].width, sheets::LETTER.0);
    }
}
