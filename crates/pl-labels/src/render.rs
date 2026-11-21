//! Draw list (mm, origin top left) – shared basis for PDF and SVG preview so that
//! preview and print look exactly the same.

use crate::codes::{CodeType, Symbol};
use crate::font::{self, Face};

pub const PT_PER_MM: f64 = 72.0 / 25.4;

pub fn mm_to_pt(mm: f64) -> f64 {
    mm * PT_PER_MM
}

pub fn pt_to_mm(pt: f64) -> f64 {
    pt / PT_PER_MM
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
}

/// Basic element.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    /// Black module grid: origin `(x, y)`, module width `mx`, module height `my`, runs
    /// `(row, x_start, length)` in modules.
    Modules { x: f64, y: f64, mx: f64, my: f64, runs: Vec<(u32, u32, u32)> },
    /// Text, `y` = baseline, `size` in pt.
    Text { x: f64, y: f64, size: f64, face: Face, anchor: Anchor, text: String },
    /// Thin grey outline (test print/preview).
    Outline { x: f64, y: f64, w: f64, h: f64, r: f64 },
    /// Dashed cut/guide line.
    Line { x1: f64, y1: f64, x2: f64, y2: f64 },
}

/// One page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub prims: Vec<Prim>,
}

/// Readability hint.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
}

/// How a label was drawn (for hints).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelMetrics {
    /// Modulbreite in mm.
    pub module: f64,
    /// Height of the code in mm (1D: bar height).
    pub code_height: f64,
    /// Font size in pt (0 without text).
    pub text_size: f64,
}

/// Smallest module width that is still read reliably at 300 dpi (≈ 3 pixels).
pub const MIN_MODULE_MM: f64 = 0.25;
/// Code 128: narrowest bar (≈ 2.2 pixels at 300 dpi).
pub const MIN_MODULE_1D_MM: f64 = 0.19;

fn text_size_fit(text: &str, max_w_mm: f64, max_cap_mm: f64) -> f64 {
    let w_em = font::text_width_em(Face::Bold, text).max(0.1);
    let by_w = mm_to_pt(max_w_mm) / w_em;
    let by_h = mm_to_pt(max_cap_mm) / font::CAP_HEIGHT;
    by_w.min(by_h).min(16.0)
}

/// Smallest font of the extra line (number range) in pt; if it does not fit, it is shortened ("…") or
/// the line is dropped.
pub const MIN_CAPTION_PT: f64 = 3.5;

/// Fit the extra line: (text, size in pt) or `None` if not even one character fits.
pub fn fit_caption(text: &str, max_w_mm: f64, max_cap_mm: f64) -> Option<(String, f64)> {
    let by_h = (mm_to_pt(max_cap_mm) / font::CAP_HEIGHT).min(7.0);
    if by_h < MIN_CAPTION_PT || max_w_mm <= 0.0 {
        return None;
    }
    let width_mm = |s: &str, size: f64| pt_to_mm(size) * font::text_width_em(Face::Regular, s);
    let by_w = mm_to_pt(max_w_mm) / font::text_width_em(Face::Regular, text).max(0.1);
    let size = by_h.min(by_w);
    if size >= MIN_CAPTION_PT {
        return Some((text.to_string(), size));
    }
    // shorten at the smallest font
    let chars: Vec<char> = text.chars().collect();
    for n in (1..chars.len()).rev() {
        let cut: String = chars[..n].iter().collect();
        let cut = cut.trim_end().trim_end_matches('·').trim_end();
        if cut.is_empty() {
            break;
        }
        let s = format!("{cut}…");
        if width_mm(&s, MIN_CAPTION_PT) <= max_w_mm {
            return Some((s, MIN_CAPTION_PT));
        }
    }
    None
}

/// Draws a label (code + optional text) into the rectangle `(x, y, w, h)`. `caption` = small
/// extra line below the number (only in the "code left, text right" layout and only if it fits).
pub fn draw_label(
    out: &mut Vec<Prim>,
    (x, y, w, h): (f64, f64, f64, f64),
    sym: &Symbol,
    text: Option<&str>,
    caption: Option<&str>,
) -> LabelMetrics {
    let pad = (w.min(h) * 0.08).clamp(0.8, 2.5);
    let (iw, ih) = (w - 2.0 * pad, h - 2.0 * pad);
    let qz = f64::from(sym.kind.quiet_modules());
    if sym.kind.is_1d() {
        // Bars across the width, text below
        let module = iw / (f64::from(sym.width) + 2.0 * qz);
        let (bar_h, text_size) = match text {
            Some(t) => {
                let size = text_size_fit(t, iw, ih * 0.24);
                let cap = pt_to_mm(size) * font::CAP_HEIGHT;
                let gap = (cap * 0.35).max(0.4);
                let bar_h = ih - cap - gap;
                out.push(Prim::Text {
                    x: x + w / 2.0,
                    y: y + pad + bar_h + gap + cap,
                    size,
                    face: Face::Bold,
                    anchor: Anchor::Middle,
                    text: t.into(),
                });
                (bar_h, size)
            }
            None => (ih, 0.0),
        };
        out.push(Prim::Modules { x: x + pad + qz * module, y: y + pad, mx: module, my: bar_h, runs: sym.runs() });
        return LabelMetrics { module, code_height: bar_h, text_size };
    }
    let n = f64::from(sym.width) + 2.0 * qz;
    match text {
        Some(t) if iw >= ih * 1.6 => {
            // Code left, text to the right
            let side = ih;
            let module = side / n;
            out.push(Prim::Modules { x: x + pad + qz * module, y: y + pad + qz * module, mx: module, my: module, runs: sym.runs() });
            let tx = x + pad + side + pad * 0.5;
            let tw = x + w - pad - tx;
            let size = text_size_fit(t, tw, ih * 0.42);
            let cap = pt_to_mm(size) * font::CAP_HEIGHT;
            let text_w = pt_to_mm(size) * font::text_width_em(Face::Bold, t);
            let cx = tx + (tw - text_w).max(0.0) / 2.0 + text_w / 2.0;
            // Extra line: at most 30 % of the height, block (number + line) vertically centred
            let gap = (cap * 0.35).max(0.4);
            let extra = caption.and_then(|c| fit_caption(c, tw, (ih - cap - gap).min(ih * 0.3)));
            match extra {
                Some((s, sz)) => {
                    let ccap = pt_to_mm(sz) * font::CAP_HEIGHT;
                    let block = cap + gap + ccap;
                    let top = y + (h - block) / 2.0;
                    out.push(Prim::Text { x: cx, y: top + cap, size, face: Face::Bold, anchor: Anchor::Middle, text: t.into() });
                    let cw = pt_to_mm(sz) * font::text_width_em(Face::Regular, &s);
                    let ccx = tx + (tw - cw).max(0.0) / 2.0 + cw / 2.0;
                    out.push(Prim::Text { x: ccx, y: top + block, size: sz, face: Face::Regular, anchor: Anchor::Middle, text: s });
                }
                None => {
                    // centred in the text field
                    out.push(Prim::Text {
                        x: cx,
                        y: y + h / 2.0 + cap / 2.0,
                        size,
                        face: Face::Bold,
                        anchor: Anchor::Middle,
                        text: t.into(),
                    });
                }
            }
            LabelMetrics { module, code_height: side - 2.0 * qz * module, text_size: size }
        }
        Some(t) => {
            // Code on top, text below, block vertically centred
            let size = text_size_fit(t, iw, ih * 0.2);
            let cap = pt_to_mm(size) * font::CAP_HEIGHT;
            let gap = (cap * 0.3).max(0.3);
            let side = iw.min(ih - cap - gap);
            let module = side / n;
            let block = side + gap + cap;
            let top = y + pad + (ih - block) / 2.0;
            let left = x + (w - side) / 2.0;
            out.push(Prim::Modules { x: left + qz * module, y: top + qz * module, mx: module, my: module, runs: sym.runs() });
            // Quiet zone belongs to the code→text gap
            out.push(Prim::Text {
                x: x + w / 2.0,
                y: top + side + gap + cap - qz * module * 0.5,
                size,
                face: Face::Bold,
                anchor: Anchor::Middle,
                text: t.into(),
            });
            LabelMetrics { module, code_height: side - 2.0 * qz * module, text_size: size }
        }
        None => {
            let side = iw.min(ih);
            let module = side / n;
            out.push(Prim::Modules {
                x: x + (w - side) / 2.0 + qz * module,
                y: y + (h - side) / 2.0 + qz * module,
                mx: module,
                my: module,
                runs: sym.runs(),
            });
            LabelMetrics { module, code_height: side - 2.0 * qz * module, text_size: 0.0 }
        }
    }
}

/// Readability hints of a label.
pub fn metrics_warnings(kind: CodeType, m: &LabelMetrics, with_text: bool) -> Vec<Warning> {
    let mut w = Vec::new();
    let min = if kind.is_1d() { MIN_MODULE_1D_MM } else { MIN_MODULE_MM };
    if m.module < min {
        w.push(Warning {
            code: "module_too_small",
            message: format!(
                "The code modules are only {:.2} mm wide (recommended ≥ {min:.2} mm). Use a larger label, turn off the text or choose {}.",
                m.module,
                if kind.is_1d() { "a 2D code (QR/DataMatrix)" } else { "DataMatrix" }
            ),
        });
    }
    if kind.is_1d() && m.code_height < 5.0 {
        w.push(Warning {
            code: "bars_too_short",
            message: format!("The bars are only {:.1} mm high (recommended ≥ 5 mm).", m.code_height),
        });
    }
    if with_text && m.text_size > 0.0 && m.text_size < 4.0 {
        w.push(Warning { code: "text_too_small", message: format!("The text is only {:.1} pt high.", m.text_size) });
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::encode;

    fn bbox(prims: &[Prim]) -> (f64, f64, f64, f64) {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in prims {
            match p {
                Prim::Modules { x, y, mx, my, runs } => {
                    for (r, s, l) in runs {
                        x0 = x0.min(x + f64::from(*s) * mx);
                        x1 = x1.max(x + f64::from(s + l) * mx);
                        y0 = y0.min(y + f64::from(*r) * my);
                        y1 = y1.max(y + f64::from(r + 1) * my);
                    }
                }
                Prim::Text { x, y, size, anchor, text, face } => {
                    let tw = pt_to_mm(*size) * font::text_width_em(*face, text);
                    let left = if *anchor == Anchor::Middle { x - tw / 2.0 } else { *x };
                    x0 = x0.min(left);
                    x1 = x1.max(left + tw);
                    y0 = y0.min(y - pt_to_mm(*size) * font::CAP_HEIGHT);
                    y1 = y1.max(*y);
                }
                _ => {}
            }
        }
        (x0, y0, x1, y1)
    }

    #[test]
    fn label_content_stays_inside_label() {
        let sizes = [(25.4, 10.0), (38.0, 21.2), (45.7, 21.2), (48.5, 25.4), (66.675, 25.4), (44.45, 12.7), (20.0, 30.0), (70.0, 37.0)];
        for kind in [CodeType::Qr, CodeType::DataMatrix, CodeType::Code128] {
            let sym = encode(kind, "ASN00042").unwrap();
            for (w, h) in sizes {
                for text in [None, Some("ASN00042")] {
                    let mut prims = Vec::new();
                    let m = draw_label(&mut prims, (100.0, 50.0, w, h), &sym, text, None);
                    let (x0, y0, x1, y1) = bbox(&prims);
                    assert!(
                        x0 >= 100.0 && y0 >= 50.0 && x1 <= 100.0 + w + 1e-9 && y1 <= 50.0 + h + 1e-9,
                        "{kind:?} {w}x{h} {text:?}: {x0},{y0},{x1},{y1}"
                    );
                    assert!(m.module > 0.0 && m.code_height > 0.0);
                    if text.is_some() {
                        assert!(m.text_size >= 3.0, "{kind:?} {w}x{h}: text {}", m.text_size);
                    }
                }
            }
        }
    }

    #[test]
    fn tiny_label_qr_is_readable_size() {
        // L4731 (25.4 × 10 mm) with text: QR left ≥ 0.3 mm/module
        let sym = encode(CodeType::Qr, "ASN00042").unwrap();
        let mut prims = Vec::new();
        let m = draw_label(&mut prims, (0.0, 0.0, 25.4, 10.0), &sym, Some("ASN00042"), None);
        assert!(m.module >= MIN_MODULE_MM, "{}", m.module);
        assert!(metrics_warnings(CodeType::Qr, &m, true).is_empty());
        // Code 128 on 25.4 mm is too dense → hint
        let sym = encode(CodeType::Code128, "ASN00042").unwrap();
        let m = draw_label(&mut Vec::new(), (0.0, 0.0, 25.4, 10.0), &sym, Some("ASN00042"), None);
        assert!(!metrics_warnings(CodeType::Code128, &m, true).is_empty());
    }

    #[test]
    fn caption_fits_inside_or_is_dropped() {
        let sizes = [(25.4, 10.0), (38.0, 21.2), (45.7, 21.2), (48.5, 25.4), (66.675, 25.4), (70.0, 37.0)];
        let sym = encode(CodeType::Qr, "ASN00042").unwrap();
        for (w, h) in sizes {
            let mut prims = Vec::new();
            let m = draw_label(&mut prims, (10.0, 20.0, w, h), &sym, Some("ASN00042"), Some("ZH · Archiv Zürich, Regal 3"));
            let (x0, y0, x1, y1) = bbox(&prims);
            assert!(x0 >= 10.0 && y0 >= 20.0 && x1 <= 10.0 + w + 1e-9 && y1 <= 20.0 + h + 1e-9, "{w}x{h}: {x0},{y0},{x1},{y1}");
            // Code stays the same size
            let m0 = draw_label(&mut Vec::new(), (10.0, 20.0, w, h), &sym, Some("ASN00042"), None);
            assert_eq!(m.module, m0.module);
            let texts = prims.iter().filter(|p| matches!(p, Prim::Text { .. })).count();
            if w >= 45.0 {
                assert_eq!(texts, 2, "{w}x{h}: Zusatzzeile erwartet");
            }
        }
        // without room the line is dropped, too long gets shortened
        assert!(fit_caption("ZH · Archiv", 10.0, 0.5).is_none());
        let (s, sz) = fit_caption("ZH · Archiv Zürich, Regal 3", 12.0, 3.0).unwrap();
        assert!(s.ends_with('…') && sz == MIN_CAPTION_PT, "{s} {sz}");
        assert_eq!(fit_caption("ZH", 30.0, 3.0).unwrap().0, "ZH");
    }

    #[test]
    fn mm_pt_roundtrip() {
        assert!((mm_to_pt(25.4) - 72.0).abs() < 1e-12);
        assert!((pt_to_mm(72.0) - 25.4).abs() < 1e-12);
        assert!((mm_to_pt(210.0) - 595.2756).abs() < 1e-3, "A4 width 595.28 pt");
        assert!((mm_to_pt(297.0) - 841.8898).abs() < 1e-3, "A4 height 841.89 pt");
        assert!((mm_to_pt(crate::sheets::LETTER.0) - 612.0).abs() < 1e-9);
    }
}
