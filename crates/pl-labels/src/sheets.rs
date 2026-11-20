//! Label sheets: templates of known manufacturers and custom grids (all dimensions in mm).
//!
//! Labels are numbered row by row (left → right, top → bottom), index 0 = top left.
//! Dimension sources are given per template in `source`; `verified = false` means: label size and count
//! come from the manufacturer, margins/gaps are computed from page format and pitch and should be
//! checked with a test print (option "Outlines").

use serde::{Deserialize, Serialize};

use crate::{LabelError, Result};

pub const A4: (f64, f64) = (210.0, 297.0);
pub const LETTER: (f64, f64) = (215.9, 279.4);
const INCH: f64 = 25.4;

/// Sheet geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetLayout {
    /// Template identifier (`custom` for custom grids).
    #[serde(default = "custom_id")]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    /// `a4`, `letter` or `custom`.
    #[serde(default)]
    pub paper: String,
    pub page_width: f64,
    pub page_height: f64,
    pub label_width: f64,
    pub label_height: f64,
    pub cols: u32,
    pub rows: u32,
    pub margin_left: f64,
    pub margin_top: f64,
    #[serde(default)]
    pub gap_x: f64,
    #[serde(default)]
    pub gap_y: f64,
    #[serde(default)]
    pub corner_radius: f64,
    /// Margins/gaps checked against manufacturer data.
    #[serde(default)]
    pub verified: bool,
    /// Origin of the dimensions.
    #[serde(default)]
    pub source: String,
}

fn custom_id() -> String {
    "custom".into()
}

/// Position of a label on the page (mm, origin top left).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Slot {
    pub index: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Limits of custom grids.
pub const MAX_COLS_ROWS: u32 = 60;
pub const MAX_PAGE_MM: f64 = 600.0;
const EPS: f64 = 0.05;

impl SheetLayout {
    pub fn per_sheet(&self) -> u32 {
        self.cols * self.rows
    }

    /// Position of label `index` (0-based, row by row).
    pub fn slot(&self, index: u32) -> Slot {
        let col = index % self.cols.max(1);
        let row = index / self.cols.max(1);
        Slot {
            index,
            x: self.margin_left + f64::from(col) * (self.label_width + self.gap_x),
            y: self.margin_top + f64::from(row) * (self.label_height + self.gap_y),
            w: self.label_width,
            h: self.label_height,
        }
    }

    pub fn slots(&self) -> impl Iterator<Item = Slot> + '_ {
        (0..self.per_sheet()).map(|i| self.slot(i))
    }

    /// Checks a (custom) grid: everything finite, positive, labels within the page.
    pub fn validate(&self) -> Result<()> {
        let bad = |f: &str, m: &str| Err(LabelError::Invalid(format!("sheet.{f}"), m.into()));
        let nums = [
            ("page_width", self.page_width),
            ("page_height", self.page_height),
            ("label_width", self.label_width),
            ("label_height", self.label_height),
            ("margin_left", self.margin_left),
            ("margin_top", self.margin_top),
            ("gap_x", self.gap_x),
            ("gap_y", self.gap_y),
            ("corner_radius", self.corner_radius),
        ];
        for (f, v) in nums {
            if !v.is_finite() || v < 0.0 {
                return bad(f, "Must be a number ≥ 0.");
            }
        }
        if !(10.0..=MAX_PAGE_MM).contains(&self.page_width) {
            return bad("page_width", "Must be between 10 and 600 mm.");
        }
        if !(10.0..=MAX_PAGE_MM).contains(&self.page_height) {
            return bad("page_height", "Must be between 10 and 600 mm.");
        }
        if self.label_width < 5.0 {
            return bad("label_width", "Labels must be at least 5 mm wide.");
        }
        if self.label_height < 5.0 {
            return bad("label_height", "Labels must be at least 5 mm high.");
        }
        if !(1..=MAX_COLS_ROWS).contains(&self.cols) {
            return bad("cols", "Must be between 1 and 60.");
        }
        if !(1..=MAX_COLS_ROWS).contains(&self.rows) {
            return bad("rows", "Must be between 1 and 60.");
        }
        let right = self.margin_left + f64::from(self.cols) * self.label_width + f64::from(self.cols - 1) * self.gap_x;
        if right > self.page_width + EPS {
            return bad("cols", &format!("The labels are {right:.1} mm wide in total – more than the page width."));
        }
        let bottom = self.margin_top + f64::from(self.rows) * self.label_height + f64::from(self.rows - 1) * self.gap_y;
        if bottom > self.page_height + EPS {
            return bad("rows", &format!("The labels are {bottom:.1} mm high in total – more than the page height."));
        }
        Ok(())
    }
}

/// Grid centred evenly on the page (margins computed from pitch).
#[allow(clippy::too_many_arguments)]
fn centered(
    id: &str,
    vendor: &str,
    name: &str,
    paper: &str,
    (pw, ph): (f64, f64),
    (lw, lh): (f64, f64),
    (cols, rows): (u32, u32),
    (gx, gy): (f64, f64),
    radius: f64,
    verified: bool,
    source: &str,
) -> SheetLayout {
    let used_w = f64::from(cols) * lw + f64::from(cols - 1) * gx;
    let used_h = f64::from(rows) * lh + f64::from(rows - 1) * gy;
    SheetLayout {
        id: id.into(),
        name: name.into(),
        vendor: vendor.into(),
        paper: paper.into(),
        page_width: pw,
        page_height: ph,
        label_width: lw,
        label_height: lh,
        cols,
        rows,
        margin_left: round3((pw - used_w) / 2.0),
        margin_top: round3((ph - used_h) / 2.0),
        gap_x: gx,
        gap_y: gy,
        corner_radius: radius,
        verified,
        source: source.into(),
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Bundled templates (order = display; first = default).
pub fn presets() -> Vec<SheetLayout> {
    vec![
        centered(
            "avery-l4731",
            "Avery Zweckform",
            "L4731REV-25 – 25.4 × 10 mm, 189 per sheet (removable)",
            "a4",
            A4,
            (25.4, 10.0),
            (7, 27),
            (2.54, 0.0),
            1.0,
            true,
            "Avery Zweckform L4731REV-25 data sheet: 25.4 × 10 mm, 7 × 27, horizontal pitch 27.94 mm (gap 2.54 mm), vertical pitch 10 mm, margins 8.48 / 13.5 mm",
        ),
        centered(
            "herma-10001",
            "HERMA",
            "10001 – 25.4 × 10 mm, 189 per sheet (movable)",
            "a4",
            A4,
            (25.4, 10.0),
            (7, 27),
            (2.54, 0.0),
            1.0,
            false,
            "HERMA 10001: 25.4 × 10 mm, 189 per A4 sheet; pitch assumed identical to Avery L4731 (same format family) – verify with a test print",
        ),
        centered(
            "avery-l4736",
            "Avery Zweckform",
            "L4736REV-25 – 45.7 × 21.2 mm, 48 per sheet (removable)",
            "a4",
            A4,
            (45.7, 21.2),
            (4, 12),
            (2.6, 0.0),
            1.0,
            false,
            "Avery Zweckform L4736REV-25: 45.7 × 21.2 mm, 4 × 12; horizontal gap ≈ 2.6 mm, no vertical gap (from pitch 48.3 / 21.2 mm) – verify with a test print",
        ),
        centered(
            "herma-4346",
            "HERMA",
            "4346 – 45.7 × 21.2 mm, 48 per sheet (movable)",
            "a4",
            A4,
            (45.7, 21.2),
            (4, 12),
            (2.6, 0.0),
            1.0,
            false,
            "HERMA 4346: 45.7 × 21.2 mm, 48 per A4 sheet; gaps assumed like Avery L4736 – verify with a test print",
        ),
        centered(
            "avery-3666",
            "Avery Zweckform",
            "3666 – 38 × 21.2 mm, 65 per sheet",
            "a4",
            A4,
            (38.0, 21.2),
            (5, 13),
            (0.0, 0.0),
            0.0,
            false,
            "Avery Zweckform 3666: 38 × 21.2 mm, 5 × 13, edge-to-edge columns (margins 10 / 10.7 mm computed) – verify with a test print",
        ),
        centered(
            "avery-4780",
            "Avery Zweckform",
            "4780 – 48.5 × 25.4 mm, 40 per sheet",
            "a4",
            A4,
            (48.5, 25.4),
            (4, 10),
            (0.0, 0.0),
            0.0,
            true,
            "Avery Zweckform 4780 data sheet: 48.5 × 25.4 mm, 4 × 10, no gaps, margins 8 / 21.5 mm",
        ),
        SheetLayout {
            id: "avery-5160".into(),
            name: "5160 – 1\" × 2⅝\", 30 per sheet".into(),
            vendor: "Avery (US)".into(),
            paper: "letter".into(),
            page_width: LETTER.0,
            page_height: LETTER.1,
            label_width: 2.625 * INCH,
            label_height: INCH,
            cols: 3,
            rows: 10,
            margin_left: 0.1875 * INCH,
            margin_top: 0.5 * INCH,
            gap_x: 0.125 * INCH,
            gap_y: 0.0,
            corner_radius: 0.0625 * INCH,
            verified: true,
            source: "Avery 5160 template: 2.625 × 1 in, 3 × 10, side margin 3/16 in, top margin 1/2 in, horizontal pitch 2.75 in".into(),
        },
        SheetLayout {
            id: "avery-5167".into(),
            name: "5167 – ½\" × 1¾\", 80 per sheet".into(),
            vendor: "Avery (US)".into(),
            paper: "letter".into(),
            page_width: LETTER.0,
            page_height: LETTER.1,
            label_width: 1.75 * INCH,
            label_height: 0.5 * INCH,
            cols: 4,
            rows: 20,
            margin_left: 0.3 * INCH,
            margin_top: 0.5 * INCH,
            gap_x: 0.3 * INCH,
            gap_y: 0.0,
            corner_radius: 0.0625 * INCH,
            verified: true,
            source: "Avery 5167 template: 1.75 × 0.5 in, 4 × 20, side margin 0.3 in, top margin 0.5 in, horizontal pitch 2.05 in".into(),
        },
        centered(
            "a4-plain-3x8",
            "Generic",
            "A4 plain paper – 70 × 37 mm, 24 per sheet (cut or 3 × 8 labels)",
            "a4",
            A4,
            (70.0, 37.0),
            (3, 8),
            (0.0, 0.0),
            0.0,
            true,
            "Common A4 3 × 8 format (e.g. Avery 3474): 70 × 37 mm, edge to edge, margins 0 / 0.5 mm",
        ),
        centered(
            "letter-plain-4x10",
            "Generic",
            "Letter plain paper – 2\" × 1\", 40 per sheet (cut)",
            "letter",
            LETTER,
            (2.0 * INCH, INCH),
            (4, 10),
            (0.0, 0.0),
            0.0,
            true,
            "Simple grid on Letter paper for cutting by hand",
        ),
    ]
}

/// Template by identifier.
pub fn preset(id: &str) -> Option<SheetLayout> {
    presets().into_iter().find(|p| p.id == id)
}

pub const DEFAULT_PRESET: &str = "avery-l4731";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_fit_their_pages() {
        let all = presets();
        assert!(all.len() >= 10);
        let mut ids = std::collections::BTreeSet::new();
        for p in &all {
            assert!(ids.insert(p.id.clone()), "duplicate id {}", p.id);
            p.validate().unwrap_or_else(|e| panic!("{}: {e}", p.id));
            for s in p.slots() {
                assert!(s.x >= -EPS && s.y >= -EPS, "{} slot {} negative", p.id, s.index);
                assert!(s.x + s.w <= p.page_width + EPS, "{} slot {} right edge {}", p.id, s.index, s.x + s.w);
                assert!(s.y + s.h <= p.page_height + EPS, "{} slot {} bottom edge {}", p.id, s.index, s.y + s.h);
            }
        }
        assert_eq!(preset(DEFAULT_PRESET).unwrap().per_sheet(), 189);
    }

    #[test]
    fn known_dimensions() {
        let l = preset("avery-l4731").unwrap();
        assert!((l.margin_left - 8.48).abs() < 0.01 && (l.margin_top - 13.5).abs() < 0.01);
        assert!((l.slot(1).x - l.slot(0).x - 27.94).abs() < 1e-9, "horizontal pitch 27.94 mm");
        let l = preset("avery-4780").unwrap();
        assert!((l.margin_left - 8.0).abs() < 0.01 && (l.margin_top - 21.5).abs() < 0.01);
        let l = preset("avery-5160").unwrap();
        assert_eq!(l.per_sheet(), 30);
        // right margin = left margin (3/16")
        let last = l.slot(2);
        assert!((l.page_width - (last.x + last.w) - 0.1875 * INCH).abs() < 1e-6);
        let l = preset("avery-5167").unwrap();
        assert_eq!(l.per_sheet(), 80);
        let last = l.slot(79);
        assert!((l.page_height - (last.y + last.h) - 0.5 * INCH).abs() < 1e-6);
        assert!((l.page_width - (last.x + last.w) - 0.3 * INCH).abs() < 1e-6);
    }

    #[test]
    fn slot_order_is_row_major() {
        let l = preset("avery-4780").unwrap();
        assert_eq!((l.slot(0).x, l.slot(0).y), (l.margin_left, l.margin_top));
        assert_eq!(l.slot(1).y, l.slot(0).y);
        assert_eq!(l.slot(4).x, l.slot(0).x);
        assert!((l.slot(4).y - l.slot(0).y - 25.4).abs() < 1e-9);
    }

    #[test]
    fn custom_validation() {
        let mut c = preset("avery-4780").unwrap();
        c.id = "custom".into();
        c.cols = 5;
        let e = c.validate().unwrap_err();
        assert!(matches!(e, LabelError::Invalid(ref f, _) if f == "sheet.cols"), "{e:?}");
        c.cols = 4;
        c.label_height = f64::NAN;
        assert!(c.validate().is_err());
        c.label_height = 25.4;
        c.rows = 0;
        assert!(c.validate().is_err());
        c.rows = 10;
        c.validate().unwrap();
        // deserialisable without template fields
        let j = serde_json::json!({"page_width": 210, "page_height": 297, "label_width": 50, "label_height": 20,
            "cols": 3, "rows": 10, "margin_left": 10, "margin_top": 10, "gap_x": 5});
        let s: SheetLayout = serde_json::from_value(j).unwrap();
        assert_eq!(s.id, "custom");
        s.validate().unwrap();
    }
}
