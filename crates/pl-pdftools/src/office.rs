//! Office/text → PDF with WordCraft (child process only, feature `office`).
//!
//! Building blocks from WordCraft (https://github.com/storytold/wordcraft, MIT OR Apache-2.0, fixed
//! revision, see NOTICE): `wordcraft-doc` (document model), `wordcraft-docx` (Word import),
//! `wordcraft-formats` (ODT, RTF, Markdown, HTML), `wordcraft-pdf` (layout and PDF export with
//! embedded fonts, text stays searchable). No UI, no branding.
//!
//! WordCraft is alpha software. Therefore, like PdfCraft, it runs **only** in `paperrust-pdftool`
//! (own process, time and memory limit, `catch_unwind`).
//!
//! Fonts: WordCraft ships Source Sans 3, Source Serif 4, Inter and JetBrains Mono (SIL
//! OFL 1.1) and otherwise uses installed system fonts; if a requested font is missing (e.g.
//! Calibri), it substitutes a metrically similar one.

use crate::job::{ConvertReport, office_format_known};

/// Domain error with user text (without paths).
#[derive(Debug)]
pub struct OfficeError(pub String);

/// Maximum size of an input file.
pub const MAX_INPUT: u64 = 256 * 1024 * 1024;

/// A4 portrait in points – for formats without their own page size (Markdown, HTML).
const A4: (f32, f32) = (595.276, 841.89);

/// Margin for tables and slides (½ inch): more room for columns.
const NARROW_MARGIN: f32 = 36.0;

/// From this many columns a spreadsheet document is set in landscape.
const LANDSCAPE_FROM_COLUMNS: usize = 7;

/// Maximum number of cells set into the archive PDF of a spreadsheet (rest: note line).
pub const MAX_SHEET_CELLS: usize = 200_000;

fn a4(landscape: bool, margin: Option<f32>) -> wordcraft_doc::SectionProps {
    let (w, h) = if landscape { (A4.1, A4.0) } else { A4 };
    let mut p = wordcraft_doc::SectionProps { page_w: w, page_h: h, landscape, ..Default::default() };
    if let Some(m) = margin {
        p.margin_top = m;
        p.margin_bottom = m;
        p.margin_left = m;
        p.margin_right = m;
    }
    p
}

/// Read bytes and convert them into a WordCraft document.
pub fn import(format: &str, bytes: &[u8]) -> Result<wordcraft_doc::Document, OfficeError> {
    let bad = |e: String| OfficeError(format!("The {format} file could not be read: {}", clean(&e)));
    match format {
        "docx" | "docm" | "dotx" | "dotm" => wordcraft_docx::read(bytes).map_err(|e| bad(e.to_string())),
        "doc" | "dot" => {
            if wordcraft_docbin::is_encrypted(bytes) {
                return Err(OfficeError("The doc file is password protected.".into()));
            }
            wordcraft_docbin::read(bytes).map_err(|e| bad(e.to_string()))
        }
        "xlsx" | "xlsm" | "xls" | "ods" => {
            let (html, landscape) = sheet_html(bytes).map_err(bad)?;
            Ok(html_document(&html, &a4(landscape, Some(NARROW_MARGIN))))
        }
        other => {
            let page = a4(false, None);
            match wordcraft_formats::import_on(other, bytes, &page) {
                Some(r) => r.map_err(bad),
                None => Err(OfficeError(format!("The format \"{other}\" cannot be converted."))),
            }
        }
    }
}

/// Set HTML (without external images) onto a specific page.
fn html_document(html: &str, page: &wordcraft_doc::SectionProps) -> wordcraft_doc::Document {
    let mut doc = wordcraft_formats::html::import_on(html, &|_| None, page);
    doc.ensure_nonempty();
    doc
}

/// Escape text for HTML.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("<br>"),
            c if c.is_control() && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// Spreadsheet document as HTML: one heading and one bordered table per sheet, from the
/// second sheet on a new page. Also returns whether to set it in landscape.
pub fn sheet_html(bytes: &[u8]) -> Result<(String, bool), String> {
    use wordcraft_formats::sheet;
    let names = sheet::sheet_names(bytes)?;
    let mut html = String::from("<html><body>");
    let mut budget = MAX_SHEET_CELLS;
    let mut widest = 0usize;
    for (i, name) in names.iter().enumerate() {
        let brk = if i > 0 { " style=\"page-break-before:always\"" } else { "" };
        html.push_str(&format!("<h2{brk}>{}</h2>", escape_html(name)));
        let rows = match sheet::read_rows(bytes, sheet::Pick::Index(i)) {
            Ok((_, rows)) => rows,
            Err(e) => {
                html.push_str(&format!("<p><i>{}</i></p>", escape_html(&format!("This sheet could not be read: {}", clean(&e)))));
                continue;
            }
        };
        if rows.is_empty() {
            html.push_str("<p><i>(empty)</i></p>");
            continue;
        }
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
        widest = widest.max(cols);
        html.push_str("<table border=\"1\" style=\"border-collapse:collapse;font-size:8pt\">");
        let mut shown = 0usize;
        for row in &rows {
            if budget < cols {
                break;
            }
            budget -= cols;
            shown += 1;
            html.push_str("<tr>");
            for c in 0..cols {
                let v = row.get(c).map(String::as_str).unwrap_or("");
                let numeric = !v.is_empty() && v.trim().parse::<f64>().is_ok();
                let align = if numeric { " style=\"text-align:right\"" } else { "" };
                html.push_str(&format!("<td{align}>{}</td>", escape_html(v)));
            }
            html.push_str("</tr>");
        }
        html.push_str("</table>");
        if shown < rows.len() {
            html.push_str(&format!("<p><i>({} more rows not shown)</i></p>", rows.len() - shown));
        }
    }
    if names.is_empty() {
        html.push_str("<p><i>(no sheets)</i></p>");
    }
    html.push_str("</body></html>");
    Ok((html, widest >= LANDSCAPE_FROM_COLUMNS))
}

/// `Op::Slides`: HTML page built by the server (one slide per page) as A4 landscape PDF.
pub fn slides(input: &str, output: &str) -> Result<ConvertReport, OfficeError> {
    let meta = std::fs::metadata(input).map_err(|_| OfficeError("The file could not be read.".into()))?;
    if meta.len() > MAX_INPUT {
        return Err(OfficeError("The file is too large to convert.".into()));
    }
    let bytes = std::fs::read(input).map_err(|_| OfficeError("The file could not be read.".into()))?;
    let html = wordcraft_formats::html::decode(&bytes);
    let doc = html_document(&html, &a4(true, Some(NARROW_MARGIN)));
    let pdf = export(&doc)?;
    if !pdf.starts_with(b"%PDF") {
        return Err(OfficeError("The converter produced no PDF.".into()));
    }
    std::fs::write(output, &pdf).map_err(|_| OfficeError("The result could not be written.".into()))?;
    Ok(ConvertReport { format: "slides".into(), size_before: meta.len(), size_after: pdf.len() as u64 })
}

/// Shorten reader error texts (no long raw data in logs/responses).
fn clean(s: &str) -> String {
    let one_line: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    one_line.chars().take(200).collect()
}

/// Document as PDF.
pub fn export(doc: &wordcraft_doc::Document) -> Result<Vec<u8>, OfficeError> {
    let opts = wordcraft_pdf::PdfOptions::default();
    wordcraft_pdf::export(doc, &opts).map_err(|e| OfficeError(format!("The PDF could not be written: {}", clean(&e.to_string()))))
}

/// Convert `input` (format `format`) to `output` (PDF).
pub fn convert(input: &str, output: &str, format: &str) -> Result<ConvertReport, OfficeError> {
    let format = format.trim().trim_start_matches('.').to_ascii_lowercase();
    if !office_format_known(&format) {
        return Err(OfficeError(format!("The format \"{format}\" cannot be converted.")));
    }
    let meta = std::fs::metadata(input).map_err(|_| OfficeError("The file could not be read.".into()))?;
    if meta.len() > MAX_INPUT {
        return Err(OfficeError("The file is too large to convert.".into()));
    }
    let bytes = std::fs::read(input).map_err(|_| OfficeError("The file could not be read.".into()))?;
    let doc = import(&format, &bytes)?;
    drop(bytes);
    let pdf = export(&doc)?;
    if !pdf.starts_with(b"%PDF") {
        return Err(OfficeError("The converter produced no PDF.".into()));
    }
    std::fs::write(output, &pdf).map_err(|_| OfficeError("The result could not be written.".into()))?;
    Ok(ConvertReport { format, size_before: meta.len(), size_after: pdf.len() as u64 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(r: &[&[&str]]) -> Vec<Vec<String>> {
        r.iter().map(|row| row.iter().map(|c| (*c).to_string()).collect()).collect()
    }

    #[test]
    fn sheet_html_tables_per_sheet() {
        let xls = wordcraft_formats::sheet::xls_fixture(&[
            ("Kunden", rows(&[&["Name", "Betrag"], &["Müller <AG>", "12.5"], &["Datum", "date:45292"]])),
            ("Leer", Vec::new()),
        ])
        .unwrap();
        let (html, landscape) = sheet_html(&xls).unwrap();
        assert!(!landscape);
        assert!(html.contains("<h2>Kunden</h2>"), "{html}");
        assert!(html.contains("Müller &lt;AG&gt;"), "{html}");
        assert!(html.contains("<td style=\"text-align:right\">12.5</td>"), "{html}");
        assert!(html.contains("2024-01-01"), "{html}");
        assert!(html.contains("page-break-before:always\">Leer</h2><p><i>(empty)</i></p>"), "{html}");
        // whole chain up to the PDF
        let pdf = export(&import("xls", &xls).unwrap()).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[test]
    fn wide_sheet_is_landscape() {
        let wide: Vec<&str> = vec!["x"; 9];
        let xls = wordcraft_formats::sheet::xls_fixture(&[("Breit", rows(&[&wide]))]).unwrap();
        assert!(sheet_html(&xls).unwrap().1);
    }

    #[test]
    fn escape() {
        assert_eq!(escape_html("a&b\n<\"c\">\u{1}"), "a&amp;b<br>&lt;&quot;c&quot;&gt;");
    }

    #[test]
    fn slides_landscape_pages() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("s.html");
        std::fs::write(&src, "<h2>Eins</h2><p>a</p><h2 style=\"page-break-before:always\">Zwei</h2><p>b</p>").unwrap();
        let out = t.path().join("s.pdf");
        let r = slides(&src.to_string_lossy(), &out.to_string_lossy()).unwrap();
        assert_eq!(r.format, "slides");
        let d = lopdf::Document::load(&out).unwrap();
        let pages = d.get_pages();
        assert_eq!(pages.len(), 2);
        let first = *pages.values().next().unwrap();
        let mb = d.get_dictionary(first).unwrap().get(b"MediaBox").unwrap().as_array().unwrap().clone();
        let w = mb[2].as_float().unwrap();
        let h = mb[3].as_float().unwrap();
        assert!(w > h, "landscape expected: {w}x{h}");
    }

    #[test]
    fn doc_garbage_is_an_error() {
        assert!(import("doc", b"not a word file").is_err());
        assert!(import("xlsx", b"PK nope").is_err());
    }
}
