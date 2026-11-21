//! Text of digital PDFs in reading order like `pdftotext -layout` (gap E14).
//!
//! upstream: src/paperless/parsers/utils.py@v3.4.2 `extract_pdf_text` (`pdftotext -q -layout -enc
//! UTF-8`); then `post_process_text` (whitespace runs → one space).
//!
//! poppler assembles each line from the glyphs sitting on the same baseline, from
//! left to right, the lines from top to bottom; gaps between words become
//! spaces (with `-layout` several, which `post_process_text` shortens to one again). Here
//! the same principle with lopdf: content streams (including form XObjects) are executed with
//! text state (`Tm`, `Td`/`TD`/`T*`, `Tf`, `Tc`/`Tw`/`Tz`/`TL`, `cm`/`q`/`Q`), each shown
//! text piece gets a start/end position (widths from `Widths`/`W` of the font, otherwise ½ em),
//! then pieces are grouped into lines and sorted.
//!
//! Limits: no font programs (widths only from the PDF dictionary), vertical/rotated
//! text is sorted in by its anchor position, columns are – as with `-layout` – not
//! separated, but output line by line next to each other.

use std::collections::BTreeMap;

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Encoding, Object, ObjectId};

/// 2D-Matrix `[a b c d e f]`.
type M = [f64; 6];

const IDENTITY: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn mul(m1: &M, m2: &M) -> M {
    [
        m1[0] * m2[0] + m1[1] * m2[2],
        m1[0] * m2[1] + m1[1] * m2[3],
        m1[2] * m2[0] + m1[3] * m2[2],
        m1[2] * m2[1] + m1[3] * m2[3],
        m1[4] * m2[0] + m1[5] * m2[2] + m2[4],
        m1[4] * m2[1] + m1[5] * m2[3] + m2[5],
    ]
}

fn num(o: &Object) -> f64 {
    match o {
        Object::Integer(i) => *i as f64,
        Object::Real(r) => f64::from(*r),
        _ => 0.0,
    }
}

fn matrix(ops: &[Object]) -> Option<M> {
    if ops.len() < 6 {
        return None;
    }
    let mut m = [0.0; 6];
    for (i, o) in ops.iter().take(6).enumerate() {
        m[i] = num(o);
    }
    Some(m)
}

/// A shown text piece (device coordinates, y upwards).
#[derive(Debug, Clone)]
struct Frag {
    x0: f64,
    x1: f64,
    y: f64,
    size: f64,
    text: String,
}

/// Font: encoding and widths (1/1000 em).
struct Font<'a> {
    enc: Option<Encoding<'a>>,
    two_byte: bool,
    first_char: i64,
    widths: Vec<f64>,
    cid_widths: BTreeMap<u32, f64>,
    default_width: f64,
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(o),
        _ => o,
    }
}

fn font_info<'a>(doc: &'a Document, f: &'a Dictionary, limit: usize) -> Font<'a> {
    let enc = f.get_font_encoding_with_limit(doc, limit).ok();
    let subtype = f.get(b"Subtype").ok().and_then(|s| s.as_name().ok()).unwrap_or(b"");
    let two_byte = subtype == b"Type0";
    let first_char = f.get(b"FirstChar").map(|o| num(deref(doc, o)) as i64).unwrap_or(0);
    let widths = f
        .get(b"Widths")
        .ok()
        .map(|o| deref(doc, o))
        .and_then(|o| o.as_array().ok())
        .map(|a| a.iter().map(|w| num(deref(doc, w))).collect())
        .unwrap_or_default();
    let mut cid_widths = BTreeMap::new();
    let mut default_width = if two_byte { 1000.0 } else { 500.0 };
    if two_byte
        && let Some(desc) = f
            .get(b"DescendantFonts")
            .ok()
            .map(|o| deref(doc, o))
            .and_then(|o| o.as_array().ok())
            .and_then(|a| a.first())
            .map(|o| deref(doc, o))
            .and_then(|o| o.as_dict().ok())
    {
        if let Ok(dw) = desc.get(b"DW") {
            default_width = num(deref(doc, dw));
        }
        if let Some(w) = desc.get(b"W").ok().map(|o| deref(doc, o)).and_then(|o| o.as_array().ok()) {
            // [c [w1 w2 …]] or [c_first c_last w]
            let mut i = 0;
            while i < w.len() && cid_widths.len() < 65_536 {
                let start = num(deref(doc, &w[i])) as u32;
                match w.get(i + 1).map(|o| deref(doc, o)) {
                    Some(Object::Array(list)) => {
                        for (k, v) in list.iter().enumerate().take(65_536) {
                            cid_widths.insert(start + k as u32, num(deref(doc, v)));
                        }
                        i += 2;
                    }
                    Some(last) => {
                        let last = num(last) as u32;
                        let width = w.get(i + 2).map(|o| num(deref(doc, o))).unwrap_or(default_width);
                        for c in start..=last.min(start.saturating_add(65_535)) {
                            cid_widths.insert(c, width);
                        }
                        i += 3;
                    }
                    None => break,
                }
            }
        }
    }
    Font { enc, two_byte, first_char, widths, cid_widths, default_width }
}

impl Font<'_> {
    fn decode(&self, bytes: &[u8]) -> String {
        match &self.enc {
            Some(e) => Document::decode_text(e, bytes).unwrap_or_default(),
            None => bytes.iter().map(|b| char::from(*b)).collect(),
        }
    }
    /// Codes with width (1/1000 em) and whether it is the space character (code 32, one byte).
    fn codes(&self, bytes: &[u8]) -> Vec<(f64, bool)> {
        if self.two_byte {
            bytes
                .chunks(2)
                .map(|c| {
                    let code = if c.len() == 2 { u32::from(c[0]) << 8 | u32::from(c[1]) } else { u32::from(c[0]) };
                    (self.cid_widths.get(&code).copied().unwrap_or(self.default_width), false)
                })
                .collect()
        } else {
            bytes
                .iter()
                .map(|b| {
                    let idx = i64::from(*b) - self.first_char;
                    let w = if idx >= 0 { self.widths.get(idx as usize).copied() } else { None };
                    (w.filter(|w| *w > 0.0).unwrap_or(self.default_width), *b == 32)
                })
                .collect()
        }
    }
}

#[derive(Clone)]
struct TextState {
    ctm: M,
    tc: f64,
    tw: f64,
    th: f64,
    tl: f64,
    font: Option<Vec<u8>>,
    size: f64,
}

struct Ctx<'a, 'd> {
    doc: &'d Document,
    fonts: &'a BTreeMap<Vec<u8>, Font<'d>>,
    out: &'a mut Vec<Frag>,
    limit: usize,
    budget: usize,
}

fn fonts_of<'d>(doc: &'d Document, res: Option<&'d Dictionary>, limit: usize) -> BTreeMap<Vec<u8>, Font<'d>> {
    let mut m = BTreeMap::new();
    let Some(res) = res else { return m };
    let Some(fd) = res.get(b"Font").ok().map(|o| deref(doc, o)).and_then(|o| o.as_dict().ok()) else { return m };
    for (name, f) in fd.iter() {
        if let Ok(d) = deref(doc, f).as_dict() {
            m.insert(name.clone(), font_info(doc, d, limit));
        }
    }
    m
}

/// Execute content stream and collect text pieces.
fn run(cx: &mut Ctx<'_, '_>, content: &[u8], res: Option<&Dictionary>, start: M, depth: usize) {
    let Ok(content) = Content::decode(content) else { return };
    let mut gs = TextState { ctm: start, tc: 0.0, tw: 0.0, th: 1.0, tl: 0.0, font: None, size: 0.0 };
    let mut stack: Vec<TextState> = Vec::new();
    let mut tm = IDENTITY;
    let mut tlm = IDENTITY;
    for op in &content.operations {
        if cx.budget == 0 {
            return;
        }
        cx.budget -= 1;
        let o = &op.operands;
        match op.operator.as_str() {
            "q" => {
                if stack.len() < 64 {
                    stack.push(gs.clone());
                }
            }
            "Q" => {
                if let Some(s) = stack.pop() {
                    gs = s;
                }
            }
            "cm" => {
                if let Some(m) = matrix(o) {
                    gs.ctm = mul(&m, &gs.ctm);
                }
            }
            "BT" => {
                tm = IDENTITY;
                tlm = IDENTITY;
            }
            "Tf" => {
                gs.font = o.first().and_then(|n| n.as_name().ok()).map(<[u8]>::to_vec);
                gs.size = o.get(1).map(num).unwrap_or(0.0);
            }
            "Tc" => gs.tc = o.first().map(num).unwrap_or(0.0),
            "Tw" => gs.tw = o.first().map(num).unwrap_or(0.0),
            "Tz" => gs.th = o.first().map(num).unwrap_or(100.0) / 100.0,
            "TL" => gs.tl = o.first().map(num).unwrap_or(0.0),
            "Td" | "TD" => {
                let (tx, ty) = (o.first().map(num).unwrap_or(0.0), o.get(1).map(num).unwrap_or(0.0));
                if op.operator == "TD" {
                    gs.tl = -ty;
                }
                tlm = mul(&[1.0, 0.0, 0.0, 1.0, tx, ty], &tlm);
                tm = tlm;
            }
            "Tm" => {
                if let Some(m) = matrix(o) {
                    tlm = m;
                    tm = m;
                }
            }
            "T*" => {
                tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -gs.tl], &tlm);
                tm = tlm;
            }
            "Tj" | "'" | "\"" | "TJ" => {
                if op.operator == "'" || op.operator == "\"" {
                    if op.operator == "\"" {
                        gs.tw = o.first().map(num).unwrap_or(gs.tw);
                        gs.tc = o.get(1).map(num).unwrap_or(gs.tc);
                    }
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -gs.tl], &tlm);
                    tm = tlm;
                }
                let items: Vec<&Object> = match op.operator.as_str() {
                    "TJ" => o.first().and_then(|a| a.as_array().ok()).map(|a| a.iter().collect()).unwrap_or_default(),
                    "\"" => o.get(2).into_iter().collect(),
                    _ => o.first().into_iter().collect(),
                };
                let Some(font) = gs.font.as_ref().and_then(|n| cx.fonts.get(n)) else { continue };
                for it in items {
                    match it {
                        Object::String(bytes, _) => {
                            let trm = mul(&tm, &gs.ctm);
                            let scale = (trm[0] * trm[3] - trm[1] * trm[2]).abs().sqrt();
                            let size = (gs.size * scale).abs();
                            let mut adv = 0.0;
                            for (w, space) in font.codes(bytes) {
                                adv += (w / 1000.0 * gs.size + gs.tc + if space { gs.tw } else { 0.0 }) * gs.th;
                            }
                            let end = mul(&[1.0, 0.0, 0.0, 1.0, adv, 0.0], &tm);
                            let end = mul(&end, &gs.ctm);
                            let text = font.decode(bytes);
                            if !text.is_empty() && cx.out.len() < 2_000_000 {
                                let (x0, x1) = if end[4] >= trm[4] { (trm[4], end[4]) } else { (end[4], trm[4]) };
                                cx.out.push(Frag { x0, x1, y: trm[5], size: size.max(1.0), text });
                            }
                            tm = mul(&[1.0, 0.0, 0.0, 1.0, adv, 0.0], &tm);
                        }
                        n @ (Object::Integer(_) | Object::Real(_)) => {
                            let tx = -num(n) / 1000.0 * gs.size * gs.th;
                            tm = mul(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], &tm);
                        }
                        _ => {}
                    }
                }
            }
            "Do" if depth < 8 => {
                let Some(name) = o.first().and_then(|n| n.as_name().ok()) else { continue };
                let Some(xo) = res.and_then(|r| r.get(b"XObject").ok()).map(|x| deref(cx.doc, x)).and_then(|x| x.as_dict().ok()) else {
                    continue;
                };
                let Some(Object::Stream(s)) = xo.get(name).ok().map(|x| deref(cx.doc, x)) else { continue };
                if s.dict.get(b"Subtype").ok().and_then(|t| t.as_name().ok()) != Some(b"Form") {
                    continue;
                }
                let Ok(data) = s
                    .decompressed_content_with_limit(cx.limit)
                    .or_else(|_| if s.dict.get(b"Filter").is_err() { Ok(s.content.clone()) } else { Err(()) })
                else {
                    continue;
                };
                let fm = s.dict.get(b"Matrix").ok().and_then(|m| m.as_array().ok()).and_then(|a| matrix(a)).unwrap_or(IDENTITY);
                let form_res = s.dict.get(b"Resources").ok().map(|r| deref(cx.doc, r)).and_then(|r| r.as_dict().ok());
                let start = mul(&fm, &gs.ctm);
                match form_res {
                    Some(fr) if fr.get(b"Font").is_ok() => {
                        let fonts = fonts_of(cx.doc, Some(fr), cx.limit);
                        let mut sub = Ctx { doc: cx.doc, fonts: &fonts, out: cx.out, limit: cx.limit, budget: cx.budget };
                        run(&mut sub, &data, Some(fr), start, depth + 1);
                        cx.budget = sub.budget;
                    }
                    _ => run(cx, &data, form_res.or(res), start, depth + 1),
                }
            }
            _ => {}
        }
    }
}

/// Pieces to lines: same baseline (tolerance ½ font size), top to bottom, left to
/// right; gap > 0.15 em → space.
fn layout(mut frags: Vec<Frag>) -> String {
    frags.sort_by(|a, b| {
        b.y.partial_cmp(&a.y).unwrap_or(std::cmp::Ordering::Equal).then(a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut lines: Vec<(f64, f64, Vec<Frag>)> = Vec::new();
    for f in frags {
        match lines.last_mut() {
            Some((y, size, items)) if (*y - f.y).abs() <= 0.5 * size.min(f.size) => {
                *size = size.max(f.size);
                items.push(f);
            }
            _ => lines.push((f.y, f.size, vec![f])),
        }
    }
    let mut out = String::new();
    for (_, _, mut items) in lines {
        items.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal));
        let mut line = String::new();
        let mut prev_end: Option<f64> = None;
        for it in items {
            if let Some(end) = prev_end
                && it.x0 - end > 0.15 * it.size
                && !line.ends_with(char::is_whitespace)
                && !it.text.starts_with(char::is_whitespace)
            {
                line.push(' ');
            }
            push_expanded(&mut line, &it.text);
            prev_end = Some(prev_end.map_or(it.x1, |e: f64| e.max(it.x1)));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Split ligatures into single letters like poppler (U+FB01 → `fi`).
fn push_expanded(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '\u{FB00}' => out.push_str("ff"),
            '\u{FB01}' => out.push_str("fi"),
            '\u{FB02}' => out.push_str("fl"),
            '\u{FB03}' => out.push_str("ffi"),
            '\u{FB04}' => out.push_str("ffl"),
            '\u{FB05}' | '\u{FB06}' => out.push_str("st"),
            c => out.push(c),
        }
    }
}

/// Text of a page in reading order (`None`: page not readable).
pub fn page_text(doc: &Document, page: ObjectId, limit: usize) -> Option<String> {
    let content = doc.get_page_content_with_limit(page, limit).ok()?;
    let (res, inherited) = doc.get_page_resources(page).ok()?;
    // Resources: own first, otherwise inherited
    let res_dict: Option<&Dictionary> = res.or_else(|| inherited.first().and_then(|id| doc.get_dictionary(*id).ok()));
    let fonts = doc.get_page_fonts(page).ok()?.into_iter().map(|(n, f)| (n, font_info(doc, f, limit))).collect::<BTreeMap<_, _>>();
    let mut frags = Vec::new();
    let mut cx = Ctx { doc, fonts: &fonts, out: &mut frags, limit, budget: 5_000_000 };
    run(&mut cx, &content, res_dict, IDENTITY, 0);
    Some(layout(frags))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};

    /// PDF with Helvetica (WinAnsi, widths 500) and the given content stream.
    fn pdf(content: &str) -> (Document, ObjectId) {
        let mut d = Document::with_version("1.5");
        let pages_id = d.new_object_id();
        let font = d.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
            "FirstChar" => 32, "Widths" => (32..=255).map(|_| Object::Integer(500)).collect::<Vec<_>>(),
        });
        let c = d.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
        let page = d.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } }, "Contents" => c,
        });
        d.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
        let cat = d.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        d.trailer.set("Root", cat);
        (d, page)
    }

    #[test]
    fn lines_top_to_bottom_left_to_right() {
        // Lines written in reverse order, right column before the left one
        let (d, p) =
            pdf("BT /F1 10 Tf 300 700 Td (rechts) Tj ET BT /F1 10 Tf 100 600 Td (unten) Tj ET BT /F1 10 Tf 100 700 Td (links) Tj ET");
        assert_eq!(page_text(&d, p, 1 << 20).unwrap(), "links rechts\nunten\n");
    }

    #[test]
    fn word_gaps_from_td_and_tj_kerning() {
        // Words offset via Td (no spaces in the text) and via TJ spacing
        let (d, p) = pdf(
            "BT /F1 10 Tf 100 700 Td (Rech) Tj 30 0 Td (nung) Tj ET BT /F1 10 Tf 100 680 Td [(Betrag) -600 (12,50)] TJ ET BT /F1 10 Tf 100 660 Td [(Zu) -20 (sammen)] TJ ET",
        );
        // "Rech" = 4 × 5 pt = 20 pt wide, next start at +30 → gap 10 pt > 1.5 pt
        assert_eq!(page_text(&d, p, 1 << 20).unwrap(), "Rech nung\nBetrag 12,50\nZusammen\n");
    }

    #[test]
    fn tstar_and_quote_operators_and_ctm() {
        let (d, p) = pdf("q 2 0 0 2 0 0 cm BT /F1 10 Tf 12 TL 50 300 Td (A) Tj T* (B) Tj (C) ' ET Q");
        assert_eq!(page_text(&d, p, 1 << 20).unwrap(), "A\nB\nC\n");
    }
}
