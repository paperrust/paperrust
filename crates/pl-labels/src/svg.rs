//! SVG preview of the same draw list (viewBox in mm). Codes as one `<path>` per symbol in
//! module coordinates (compact), text in Helvetica/Arial.

use std::fmt::Write as _;

use crate::font::Face;
use crate::pdf::num;
use crate::render::{Anchor, Page, Prim, pt_to_mm};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// SVG of a page. `outlines = true` additionally draws all label outlines of the draw list.
pub fn write(page: &Page) -> String {
    let mut s = String::with_capacity(64 * 1024);
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}mm" height="{h}mm" shape-rendering="crispEdges"><rect width="{w}" height="{h}" fill="#fff"/>"##,
        w = num(page.width),
        h = num(page.height)
    );
    let mut outlines = String::new();
    let mut lines = String::new();
    for p in &page.prims {
        match p {
            Prim::Outline { x, y, w, h, r } => {
                let _ = write!(outlines, r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}"/>"#, num(*x), num(*y), num(*w), num(*h), num(*r));
            }
            Prim::Line { x1, y1, x2, y2 } => {
                let _ = write!(lines, r#"<line x1="{}" y1="{}" x2="{}" y2="{}"/>"#, num(*x1), num(*y1), num(*x2), num(*y2));
            }
            _ => {}
        }
    }
    if !outlines.is_empty() || !lines.is_empty() {
        let _ = write!(s, r##"<g fill="none" stroke="#9aa0a6" stroke-width="0.2">{outlines}<g stroke-dasharray="1 1">{lines}</g></g>"##);
    }
    for p in &page.prims {
        match p {
            Prim::Modules { x, y, mx, my, runs } => {
                let _ = write!(s, r#"<path transform="translate({} {}) scale({} {})" d=""#, num(*x), num(*y), num(*mx), num(*my));
                for (row, start, len) in runs {
                    let _ = write!(s, "M{start} {row}h{len}v1h-{len}z");
                }
                s.push_str(r#""/>"#);
            }
            Prim::Text { x, y, size, face, anchor, text } => {
                let _ = write!(
                    s,
                    r#"<text x="{}" y="{}" font-family="Helvetica,Arial,sans-serif" font-size="{}"{}{}>{}</text>"#,
                    num(*x),
                    num(*y),
                    num(pt_to_mm(*size)),
                    if *face == Face::Bold { r#" font-weight="bold""# } else { "" },
                    if *anchor == Anchor::Middle { r#" text-anchor="middle""# } else { "" },
                    esc(text)
                );
            }
            _ => {}
        }
    }
    s.push_str("</svg>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_shape() {
        let page = Page {
            width: 210.0,
            height: 297.0,
            prims: vec![
                Prim::Outline { x: 1.0, y: 2.0, w: 3.0, h: 4.0, r: 0.5 },
                Prim::Modules { x: 10.0, y: 10.0, mx: 0.5, my: 0.5, runs: vec![(0, 2, 3)] },
                Prim::Text { x: 5.0, y: 6.0, size: 7.2, face: Face::Bold, anchor: Anchor::Middle, text: "A<B".into() },
            ],
        };
        let s = write(&page);
        assert!(s.starts_with("<svg") && s.ends_with("</svg>"));
        assert!(s.contains(r#"viewBox="0 0 210 297""#));
        assert!(s.contains("M2 0h3v1h-3z"));
        assert!(s.contains("A&lt;B"));
        assert!(s.contains(r#"font-size="2.54""#));
        assert!(s.contains(r#"rx="0.5""#));
    }
}
