//! Minimal PDF 1.4 writer for the draw list: pure vector graphics (rectangles), standard 14
//! fonts without embedding, content streams with Flate. `/ViewerPreferences /PrintScaling /None`
//! asks viewers to print at original size (100 %).

use std::fmt::Write as _;
use std::io::Write as _;

use crate::font;
use crate::render::{Anchor, Page, Prim, mm_to_pt};

/// Number for PDF (at most 3 decimal places, without superfluous zeros).
pub fn num(v: f64) -> String {
    let v = if v.abs() < 0.0005 { 0.0 } else { v };
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// Content stream of a page (uncompressed).
pub fn content_stream(page: &Page) -> Vec<u8> {
    let ph = page.height;
    let mut s = String::new();
    let mut out: Vec<u8> = Vec::new();
    // Outlines/lines first (grey, thin), then black areas, then text
    let mut has_outline = false;
    for p in &page.prims {
        match p {
            Prim::Outline { x, y, w, h, r } => {
                if !has_outline {
                    s.push_str("q 0.6 G 0.25 w\n");
                    has_outline = true;
                }
                rounded_rect(&mut s, mm_to_pt(*x), mm_to_pt(ph - y - h), mm_to_pt(*w), mm_to_pt(*h), mm_to_pt(*r));
                s.push_str("S\n");
            }
            Prim::Line { x1, y1, x2, y2 } => {
                if !has_outline {
                    s.push_str("q 0.6 G 0.25 w\n");
                    has_outline = true;
                }
                let _ = writeln!(
                    s,
                    "[2 2] 0 d {} {} m {} {} l S [] 0 d",
                    num(mm_to_pt(*x1)),
                    num(mm_to_pt(ph - y1)),
                    num(mm_to_pt(*x2)),
                    num(mm_to_pt(ph - y2))
                );
            }
            _ => {}
        }
    }
    if has_outline {
        s.push_str("Q\n");
    }
    s.push_str("0 g\n");
    for p in &page.prims {
        if let Prim::Modules { x, y, mx, my, runs } = p {
            for (row, start, len) in runs {
                let rx = x + f64::from(*start) * mx;
                let ry = y + f64::from(*row) * my;
                let _ = writeln!(
                    s,
                    "{} {} {} {} re",
                    num(mm_to_pt(rx)),
                    num(mm_to_pt(ph - ry - my)),
                    num(mm_to_pt(f64::from(*len) * mx)),
                    num(mm_to_pt(*my))
                );
            }
            s.push_str("f\n");
        }
    }
    out.extend_from_slice(s.as_bytes());
    for p in &page.prims {
        if let Prim::Text { x, y, size, face, anchor, text } = p {
            let w = size * font::text_width_em(*face, text);
            let left = match anchor {
                Anchor::Start => mm_to_pt(*x),
                Anchor::Middle => mm_to_pt(*x) - w / 2.0,
            };
            let _ = write!(out, "BT /{} {} Tf {} {} Td ", face.resource(), num(*size), num(left), num(mm_to_pt(ph - y)));
            out.extend_from_slice(&font::pdf_string(text));
            out.extend_from_slice(b" Tj ET\n");
        }
    }
    out
}

fn rounded_rect(s: &mut String, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    if r <= 0.01 {
        let _ = writeln!(s, "{} {} {} {} re", num(x), num(y), num(w), num(h));
        return;
    }
    // Circular arc approximation with Bézier (k = 0.5523)
    let k = r * 0.552_284_75;
    let (x1, y1) = (x + w, y + h);
    let _ = writeln!(
        s,
        "{} {} m {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c h",
        num(x + r),
        num(y),
        num(x1 - r),
        num(y),
        num(x1 - r + k),
        num(y),
        num(x1),
        num(y + r - k),
        num(x1),
        num(y + r),
        num(x1),
        num(y1 - r),
        num(x1),
        num(y1 - r + k),
        num(x1 - r + k),
        num(y1),
        num(x1 - r),
        num(y1),
        num(x + r),
        num(y1),
        num(x + r - k),
        num(y1),
        num(x),
        num(y1 - r + k),
        num(x),
        num(y1 - r),
        num(x),
        num(y + r),
        num(x),
        num(y + r - k),
        num(x + r - k),
        num(y),
        num(x + r),
        num(y),
    );
}

fn pdf_text_string(s: &str) -> Vec<u8> {
    font::pdf_string(s)
}

/// Writes the pages as PDF.
pub fn write(pages: &[Page], title: &str) -> Vec<u8> {
    // Objects: 1 catalog, 2 page tree, 3 Helvetica, 4 Helvetica-Bold, 5 Info, then 2 per page.
    let mut objects: Vec<Vec<u8>> = Vec::new();
    let page_ids: Vec<usize> = (0..pages.len()).map(|i| 6 + 2 * i).collect();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R /ViewerPreferences << /PrintScaling /None >> >>".to_vec());
    let kids: Vec<String> = page_ids.iter().map(|id| format!("{id} 0 R")).collect();
    objects.push(format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()).into_bytes());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec());
    let mut info = b"<< /Producer (PaperRust) /Creator (PaperRust ASN labels) /Title ".to_vec();
    info.extend_from_slice(&pdf_text_string(title));
    info.extend_from_slice(b" >>");
    objects.push(info);
    for (i, page) in pages.iter().enumerate() {
        let content_id = page_ids[i] + 1;
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> /ProcSet [/PDF /Text] >> /Contents {content_id} 0 R >>",
                num(mm_to_pt(page.width)),
                num(mm_to_pt(page.height))
            )
            .into_bytes(),
        );
        let raw = content_stream(page);
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let compressed = enc.write_all(&raw).and_then(|_| enc.finish()).ok();
        let mut obj = Vec::new();
        match compressed {
            Some(c) => {
                let _ = write!(obj, "<< /Length {} /Filter /FlateDecode >>\nstream\n", c.len());
                obj.extend_from_slice(&c);
            }
            None => {
                let _ = write!(obj, "<< /Length {} >>\nstream\n", raw.len());
                obj.extend_from_slice(&raw);
            }
        }
        obj.extend_from_slice(b"\nendstream");
        objects.push(obj);
    }
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        let _ = writeln!(out, "{} 0 obj", i + 1);
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for off in offsets {
        let _ = writeln!(out, "{off:010} 00000 n ");
    }
    let _ = write!(out, "trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::Face;

    #[test]
    fn numbers() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(1.23456), "1.235");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(595.2756), "595.276");
        assert_eq!(num(0.5), "0.5");
    }

    #[test]
    fn writes_valid_structure() {
        let page = Page {
            width: 210.0,
            height: 297.0,
            prims: vec![
                Prim::Outline { x: 10.0, y: 10.0, w: 50.0, h: 20.0, r: 2.0 },
                Prim::Modules { x: 10.0, y: 10.0, mx: 1.0, my: 1.0, runs: vec![(0, 0, 3)] },
                Prim::Text { x: 30.0, y: 25.0, size: 10.0, face: Face::Bold, anchor: Anchor::Middle, text: "ASN(1)".into() },
            ],
        };
        let raw = String::from_utf8(content_stream(&page)).unwrap();
        // 10 mm from the top, 1 mm high → y = (297 - 11) mm
        assert!(raw.contains(&format!("28.346 {} 8.504 2.835 re", num(mm_to_pt(286.0)))), "{raw}");
        assert!(raw.contains("(ASN\\(1\\)) Tj"));
        let pdf = write(&[page.clone(), page], "Test");
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/Count 2"));
        assert!(s.contains("/MediaBox [0 0 595.276 841.89]"));
        assert!(s.contains("/PrintScaling /None"));
        // xref offsets point to the objects (in bytes, the stream is binary)
        let xref_pos: usize = s.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert!(pdf[xref_pos..].starts_with(b"xref\n"));
        let table = String::from_utf8_lossy(&pdf[xref_pos..]).to_string();
        for (i, line) in table.lines().skip(3).take(9).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(pdf[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()), "object {}", i + 1);
        }
    }
}
