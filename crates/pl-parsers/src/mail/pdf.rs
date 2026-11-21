//! E-mail → archive PDF (gap E12).
//!
//! upstream: src/paperless/parsers/mail.py@v3.4.2 (MailDocumentParser.generate_pdf, mail_to_html,
//! generate_pdf_from_mail, generate_pdf_from_html), src/paperless_mail/templates/
//! email_msg_template.html, src/paperless_mail/models.py (MailRule.PdfLayout),
//! src/paperless/settings/__init__.py (EMAIL_PARSE_DEFAULT_LAYOUT, default 1).
//!
//! Upstream renders two HTML pages with Gotenberg/Chromium (header + text of the mail from a
//! template, and the HTML part of the mail with the attachments as `cid:` resources) and merges
//! the PDFs depending on the layout. Here the plugged-in [`PdfConverter`] (WordCraft in a child
//! process) sets both pages: the header page as HTML (table instead of Tailwind grid, same
//! fields and labels), the HTML part as an MHT web archive (the `cid:` images are included as
//! parts with `Content-ID`). Merging is done with lopdf.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono_tz::Tz;
use lopdf::{Dictionary, Document, Object, ObjectId};
use regex::Regex;

use super::{EmailAddress, MailAttachment, MailMessage, aware_date, naturalsize_binary};
use crate::office::PdfConverter;
use crate::{ParseError, Result};

/// `MailRule.PdfLayout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfLayout {
    /// 0 – "System default" (for a rule like upstream: falls into the `_` branch = text, then HTML).
    Default,
    /// 1 – text, then HTML.
    TextHtml,
    /// 2 – HTML, then text.
    HtmlText,
    /// 3 – HTML only.
    HtmlOnly,
    /// 4 – text only.
    TextOnly,
}

impl PdfLayout {
    /// Integer from the rule or `PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT`; unknown → `Default`
    /// (upstream: `ValueError`; here like the `_` branch).
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::TextHtml,
            2 => Self::HtmlText,
            3 => Self::HtmlOnly,
            4 => Self::TextOnly,
            _ => Self::Default,
        }
    }
}

/// Escape HTML (Django `escape`).
fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#x27;"),
            c => o.push(c),
        }
    }
    o
}

/// `clean_html`: escape, linkify links and e-mail addresses (turbohtml `linkify` with
/// `parse_email=True`), line breaks → `<br>`.
pub fn clean_html(text: &str) -> String {
    static LINK: OnceLock<Regex> = OnceLock::new();
    let re = LINK.get_or_init(|| {
        Regex::new(r"(?i)\b(?:https?://|www\.)[^\s<>&]+(?:&amp;[^\s<>&]+)*|\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b")
            .expect("static regex")
    });
    let esc = escape(text);
    let linked = re.replace_all(&esc, |c: &regex::Captures<'_>| {
        let m = c.get(0).map(|m| m.as_str()).unwrap_or("");
        let m = m.trim_end_matches(['.', ',', ')', ';', ':']);
        let rest = &c.get(0).map(|m| m.as_str()).unwrap_or("")[m.len()..];
        let href = if m.contains('@') && !m.contains("://") {
            format!("mailto:{m}")
        } else if m.to_ascii_lowercase().starts_with("www.") {
            format!("http://{m}")
        } else {
            m.to_string()
        };
        format!("<a href=\"{href}\" rel=\"nofollow\">{m}</a>{rest}")
    });
    linked.replace('\n', "<br>")
}

fn join_full(v: &[EmailAddress]) -> String {
    v.iter().map(EmailAddress::full).collect::<Vec<_>>().join(", ")
}

/// `mail_to_html`: header page (date, from, subject, to, CC, BCC, attachments, separator line, text).
pub fn mail_to_html(mail: &MailMessage, tz: Tz) -> String {
    let subject = clean_html(&mail.subject());
    let from = clean_html(&mail.from_values().map(|a| a.full()).unwrap_or_default());
    let to = clean_html(&join_full(&mail.to_values()));
    let cc = clean_html(&join_full(&mail.cc_values()));
    let bcc = clean_html(&join_full(&mail.bcc_values()));
    let att: Vec<String> = mail.attachments().iter().map(|a| format!("{} ({})", a.filename, naturalsize_binary(a.size() as u64))).collect();
    let attachments = clean_html(&att.join(", "));
    let (date, _) = aware_date(mail.date(), tz);
    // timezone.localtime(mail.date): convert to the configured time zone
    let date = clean_html(&date.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string());
    let content = clean_html(crate::pystr::py_strip(&mail.text()));

    let label = "style=\"color:#94a3b8;text-align:right;vertical-align:top;width:15%\"";
    let mut h = String::from("<!doctype html><html><head><meta charset=\"UTF-8\"></head><body>");
    h.push_str("<table style=\"width:100%;background-color:#e2e8f0;border-collapse:collapse\" cellpadding=\"4\">");
    let from_label = if from.is_empty() { "" } else { "From" };
    h.push_str(&format!(
        "<tr><td {label}>{from_label}</td><td style=\"width:55%\">{from}</td><td style=\"text-align:right;width:30%\">{date}</td></tr>"
    ));
    for (lab, val, style) in [
        ("Subject", &subject, "font-weight:bold"),
        ("To", &to, "font-size:9pt"),
        ("CC", &cc, "font-size:9pt"),
        ("BCC", &bcc, "font-size:9pt"),
        ("Attachments", &attachments, ""),
    ] {
        if val.is_empty() {
            continue;
        }
        h.push_str(&format!("<tr><td {label}>{lab}</td><td colspan=\"2\" style=\"{style}\">{val}</td></tr>"));
    }
    h.push_str("</table>");
    h.push_str(
        "<p style=\"border-top:1px solid #000000;border-bottom:1px solid #000000;background-color:#e2e8f0;font-size:1pt\">&nbsp;</p>",
    );
    h.push_str(&format!("<div>{content}</div>"));
    h.push_str("</body></html>");
    h
}

/// `clean_html_script`: `<script` → `<div hidden `, `</script` → `</div` (case-insensitive).
pub fn clean_html_script(text: &str) -> String {
    static OPEN: OnceLock<Regex> = OnceLock::new();
    static CLOSE: OnceLock<Regex> = OnceLock::new();
    let open = OPEN.get_or_init(|| Regex::new(r"(?i)<script").expect("static regex"));
    let close = CLOSE.get_or_init(|| Regex::new(r"(?i)</script").expect("static regex"));
    let t = open.replace_all(text, "<div hidden ");
    close.replace_all(&t, "</div").into_owned()
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4 + data.len() / 57 * 2);
    for (i, ch) in data.chunks(3).enumerate() {
        if i > 0 && i % 19 == 0 {
            out.push_str("\r\n");
        }
        let b = [ch[0], *ch.get(1).unwrap_or(&0), *ch.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(char::from(T[(n >> 18) as usize & 63]));
        out.push(char::from(T[(n >> 12) as usize & 63]));
        out.push(if ch.len() > 1 { char::from(T[(n >> 6) as usize & 63]) } else { '=' });
        out.push(if ch.len() > 2 { char::from(T[n as usize & 63]) } else { '=' });
    }
    out
}

/// Header value without line breaks/control characters (against header injection).
fn header_safe(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// `generate_pdf_from_html` (input): MHT web archive with the cleaned HTML part as root and
/// all attachments as parts with `Content-ID` (for `cid:` references).
pub fn build_mht(html: &str, attachments: &[MailAttachment]) -> Vec<u8> {
    let boundary = "----=_PaperRust_MHT_7f3c2a1e9b";
    let mut out = String::new();
    out.push_str("MIME-Version: 1.0\r\n");
    out.push_str(&format!("Content-Type: multipart/related; boundary=\"{boundary}\"; type=\"text/html\"\r\n\r\n"));
    out.push_str(&format!("--{boundary}\r\nContent-Type: text/html; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\nContent-Location: index.html\r\n\r\n"));
    out.push_str(&base64(clean_html_script(html).as_bytes()));
    out.push_str("\r\n");
    for a in attachments {
        if a.content_id.is_empty() {
            continue;
        }
        let ct = if a.content_type.is_empty() { "application/octet-stream".to_string() } else { header_safe(&a.content_type) };
        out.push_str(&format!(
            "--{boundary}\r\nContent-Type: {ct}\r\nContent-Transfer-Encoding: base64\r\nContent-ID: <{}>\r\n\r\n",
            header_safe(&a.content_id)
        ));
        out.push_str(&base64(&a.payload));
        out.push_str("\r\n");
    }
    out.push_str(&format!("--{boundary}--\r\n"));
    out.into_bytes()
}

/// Copy inheritable page properties from the parent `Pages` into the page (before re-parenting).
fn inherit(doc: &Document, page: ObjectId) -> Dictionary {
    let mut d = doc.get_dictionary(page).cloned().unwrap_or_default();
    let mut parent = d.get(b"Parent").and_then(Object::as_reference).ok();
    let mut guard = 0;
    while let Some(pid) = parent {
        guard += 1;
        if guard > 64 {
            break;
        }
        let Ok(p) = doc.get_dictionary(pid) else { break };
        for key in [&b"MediaBox"[..], b"CropBox", b"Resources", b"Rotate"] {
            if !d.has(key)
                && let Ok(v) = p.get(key)
            {
                d.set(key, v.clone());
            }
        }
        parent = p.get(b"Parent").and_then(Object::as_reference).ok();
    }
    d
}

/// Merge PDFs one after another into one PDF (Gotenberg `merge`).
pub fn merge_pdfs(inputs: &[Vec<u8>]) -> Result<Vec<u8>> {
    let bad = |e: lopdf::Error| ParseError::Invalid(format!("PDF merge failed: {e}"));
    if let [one] = inputs {
        return Ok(one.clone());
    }
    let mut out = Document::with_version("1.7");
    let mut next_id = 1u32;
    let mut kids: Vec<ObjectId> = Vec::new();
    let mut pages_dicts: Vec<(ObjectId, Dictionary)> = Vec::new();
    for data in inputs {
        let mut doc = Document::load_mem(data).map_err(bad)?;
        doc.renumber_objects_with(next_id);
        next_id = doc.max_id + 1;
        for (_, pid) in doc.get_pages() {
            pages_dicts.push((pid, inherit(&doc, pid)));
            kids.push(pid);
        }
        for (id, obj) in doc.objects {
            // old catalogs and page trees are replaced
            let skip = obj
                .as_dict()
                .ok()
                .and_then(|d| d.get(b"Type").ok())
                .and_then(|t| t.as_name().ok())
                .is_some_and(|n| n == b"Catalog" || n == b"Pages");
            if !skip {
                out.objects.insert(id, obj);
            }
        }
    }
    let pages_id = (next_id, 0);
    let catalog_id = (next_id + 1, 0);
    out.max_id = next_id + 1;
    for (pid, mut d) in pages_dicts {
        d.set("Parent", Object::Reference(pages_id));
        out.objects.insert(pid, Object::Dictionary(d));
    }
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Count", kids.len() as i64);
    pages.set("Kids", kids.into_iter().map(Object::Reference).collect::<Vec<_>>());
    out.objects.insert(pages_id, Object::Dictionary(pages));
    let mut cat = Dictionary::new();
    cat.set("Type", Object::Name(b"Catalog".to_vec()));
    cat.set("Pages", Object::Reference(pages_id));
    out.objects.insert(catalog_id, Object::Dictionary(cat));
    out.trailer.set("Root", Object::Reference(catalog_id));
    let mut buf = Vec::new();
    out.save_to(&mut buf).map_err(|e| ParseError::Invalid(format!("PDF merge failed: {e}")))?;
    Ok(buf)
}

/// `generate_pdf`: produce the header page (always) and – if the mail has an HTML part – the HTML
/// part, and merge them depending on the layout. Result `dir/merged.pdf`.
pub fn generate_pdf(mail: &MailMessage, layout: PdfLayout, converter: &dyn PdfConverter, dir: &Path, tz: Tz) -> Result<PathBuf> {
    let archive = dir.join("merged.pdf");
    let html_file = dir.join("email_as_html.html");
    std::fs::write(&html_file, mail_to_html(mail, tz))?;
    let mail_pdf = converter
        .convert(&html_file, "text/html", dir)
        .map_err(|e| ParseError::Upstream(format!("Error while converting email to PDF: {e}")))?;
    let mail_bytes = std::fs::read(&mail_pdf)?;
    let html = mail.html();
    if html.is_empty() {
        std::fs::write(&archive, mail_bytes)?;
        return Ok(archive);
    }
    let mht = dir.join("index.mht");
    std::fs::write(&mht, build_mht(&html, &mail.attachments()))?;
    let html_pdf = converter
        .convert(&mht, "multipart/related", dir)
        .map_err(|e| ParseError::Upstream(format!("Error while converting email HTML to PDF: {e}")))?;
    let html_bytes = std::fs::read(&html_pdf)?;
    let parts = match layout {
        PdfLayout::HtmlText => vec![html_bytes, mail_bytes],
        PdfLayout::HtmlOnly => vec![html_bytes],
        PdfLayout::TextOnly => vec![mail_bytes],
        PdfLayout::TextHtml | PdfLayout::Default => vec![mail_bytes, html_bytes],
    };
    let merged = merge_pdfs(&parts).map_err(|e| ParseError::Upstream(format!("Error while merging email HTML into PDF: {e}")))?;
    std::fs::write(&archive, merged)?;
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        assert_eq!(PdfLayout::from_i64(0), PdfLayout::Default);
        assert_eq!(PdfLayout::from_i64(2), PdfLayout::HtmlText);
        assert_eq!(PdfLayout::from_i64(4), PdfLayout::TextOnly);
        assert_eq!(PdfLayout::from_i64(9), PdfLayout::Default);
    }

    #[test]
    fn clean_and_linkify() {
        assert_eq!(clean_html("a <b> & c\nd"), "a &lt;b&gt; &amp; c<br>d");
        assert_eq!(clean_html("see https://x.org/a?b=1."), "see <a href=\"https://x.org/a?b=1\" rel=\"nofollow\">https://x.org/a?b=1</a>.");
        assert_eq!(clean_html("mail max@example.com"), "mail <a href=\"mailto:max@example.com\" rel=\"nofollow\">max@example.com</a>");
    }

    #[test]
    fn script_is_neutralised() {
        assert_eq!(clean_html_script("<SCRIPT>x</Script><p>"), "<div hidden >x</div><p>");
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn mht_has_cid_parts() {
        let a = MailAttachment {
            filename: "logo.png".into(),
            content_id: "logo@x".into(),
            content_type: "image/png".into(),
            payload: vec![1, 2, 3],
        };
        let m = String::from_utf8(build_mht("<img src=\"cid:logo@x\"><script>a</script>", &[a])).unwrap();
        assert!(m.contains("Content-ID: <logo@x>"));
        assert!(m.contains("Content-Type: image/png"));
        assert!(m.contains(&base64(b"<img src=\"cid:logo@x\"><div hidden >a</div>")));
    }

    /// One-page PDF with a given width (page size only in the page tree: inherited).
    fn one_page(width: i64) -> Vec<u8> {
        let mut d = Document::with_version("1.5");
        let pages_id = d.new_object_id();
        let content = d.add_object(lopdf::Stream::new(Dictionary::new(), b"q Q".to_vec()));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Contents", Object::Reference(content));
        let page_id = d.add_object(page);
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", vec![Object::Reference(page_id)]);
        pages.set("Count", 1);
        pages.set("MediaBox", vec![0.into(), 0.into(), width.into(), 800.into()]);
        d.objects.insert(pages_id, Object::Dictionary(pages));
        let mut cat = Dictionary::new();
        cat.set("Type", Object::Name(b"Catalog".to_vec()));
        cat.set("Pages", Object::Reference(pages_id));
        let cat_id = d.add_object(cat);
        d.trailer.set("Root", Object::Reference(cat_id));
        let mut v = Vec::new();
        d.save_to(&mut v).unwrap();
        v
    }

    #[test]
    fn merge_keeps_order_and_inherited_size() {
        let merged = merge_pdfs(&[one_page(100), one_page(200), one_page(300)]).unwrap();
        let d = Document::load_mem(&merged).unwrap();
        let pages = d.get_pages();
        assert_eq!(pages.len(), 3);
        let widths: Vec<i64> = pages
            .values()
            .map(|id| d.get_dictionary(*id).unwrap().get(b"MediaBox").unwrap().as_array().unwrap()[2].as_i64().unwrap())
            .collect();
        assert_eq!(widths, vec![100, 200, 300]);
    }
}
