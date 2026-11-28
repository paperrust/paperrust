//! HTML → text (replacement for Tika `as_text` with `text/html`, as used by the mail parser) and
//! a dedicated HTML parser (extension: upstream does not support `text/html` as a document).
//!
//! upstream: src/paperless/parsers/mail.py@v3.4.2 (MailDocumentParser.tika_parse)
//! Tika: HtmlParser + XHTMLContentHandler + ToTextContentHandler (text without tags; `<script>`
//! and `<style>` without content; block elements end with a line break, table cells with a tab;
//! `<title>` comes first). Deviation class: Tika formats paragraphs/whitespace differently –
//! irrelevant for mails, because `strip_text` afterwards collapses every whitespace run into a
//! single space.

use std::path::Path;

use crate::limits::{TextSink, Truncation};
use crate::{DocumentParser, MetadataEntry, ParseContext, ParseError, ParsedDocument, Result, read_limited};

const BLOCK: &[&str] = &[
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "div",
    "ul",
    "ol",
    "dl",
    "pre",
    "hr",
    "blockquote",
    "address",
    "fieldset",
    "table",
    "form",
    "noscript",
    "li",
    "dt",
    "dd",
    "noframes",
    "br",
    "tr",
    "select",
    "option",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "aside",
    "main",
    "figure",
    "figcaption",
    "caption",
    "thead",
    "tbody",
    "tfoot",
    "body",
    "html",
    "title",
    "center",
    "legend",
    "details",
    "summary",
];

fn is_block(name: &str) -> bool {
    BLOCK.contains(&name)
}

/// Decodes an HTML character reference starting at `s` (after `&`). Returns: (text, bytes consumed).
fn decode_entity(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    if b.first() == Some(&b'#') {
        let (hex, start) = if matches!(b.get(1), Some(b'x') | Some(b'X')) { (true, 2) } else { (false, 1) };
        let mut end = start;
        while end < b.len() && (if hex { b[end].is_ascii_hexdigit() } else { b[end].is_ascii_digit() }) {
            end += 1;
        }
        if end == start {
            return None;
        }
        let num = u32::from_str_radix(&s[start..end.min(start + 8)], if hex { 16 } else { 10 }).unwrap_or(0xfffd);
        let consumed = if b.get(end) == Some(&b';') { end + 1 } else { end };
        // HTML5: 0x80..0x9F as Windows-1252, invalid → U+FFFD
        let c = match num {
            0 => '\u{fffd}',
            0x80..=0x9f => {
                let byte = [num as u8];
                encoding_rs::WINDOWS_1252.decode_without_bom_handling(&byte).0.chars().next().unwrap_or('\u{fffd}')
            }
            n => char::from_u32(n).unwrap_or('\u{fffd}'),
        };
        return Some((c.to_string(), consumed));
    }
    let mut end = 0;
    while end < b.len() && end < 40 && b[end].is_ascii_alphanumeric() {
        end += 1;
    }
    if end == 0 {
        return None;
    }
    if b.get(end) == Some(&b';') {
        if let Some(r) = quick_xml::escape::resolve_html5_entity(&s[..end]) {
            return Some((r.to_owned(), end + 1));
        }
        return None;
    }
    // Legacy references without semicolon (e.g. "&nbsp", "&uuml"): longest known prefix
    for l in (2..=end).rev() {
        if let Some(r) = quick_xml::escape::resolve_html5_entity(&s[..l])
            && l <= 6
        {
            return Some((r.to_owned(), l));
        }
    }
    None
}

/// Text with decoded character references.
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p + 1..];
        match decode_entity(after) {
            Some((t, n)) => {
                out.push_str(&t);
                rest = &after[n..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

struct Out {
    sink: TextSink,
    /// Does it end with a (logical) line break?
    at_line_start: bool,
    pending_space: bool,
}

impl Out {
    fn newline(&mut self) -> Result<()> {
        self.pending_space = false;
        self.sink.push('\n')?;
        self.at_line_start = true;
        Ok(())
    }
    fn text(&mut self, t: &str, pre: bool) -> Result<()> {
        if pre {
            self.sink.push_str(t)?;
            self.at_line_start = t.ends_with('\n');
            return Ok(());
        }
        for (i, word) in t.split(|c: char| c.is_ascii_whitespace()).enumerate() {
            if i > 0 {
                self.pending_space = true;
            }
            if word.is_empty() {
                continue;
            }
            if self.pending_space && !self.at_line_start {
                self.sink.push(' ')?;
            }
            self.pending_space = false;
            self.sink.push_str(word)?;
            self.at_line_start = false;
        }
        Ok(())
    }
}

/// HTML → text. Malformed markup is tolerated (like TagSoup in Tika).
///
/// Fails with [`ParseError::LimitExceeded`] when the text exceeds `max_chars`; see
/// [`html_to_text_report`] for keeping the part read so far.
pub fn html_to_text(html: &str, max_chars: usize) -> Result<String> {
    let (text, truncated) = html_to_text_report(html, max_chars)?;
    match truncated {
        Some(_) => Err(ParseError::LimitExceeded(format!("extracted text > {max_chars} bytes"))),
        None => Ok(text),
    }
}

/// [`html_to_text`] that keeps the text read so far when `max_chars` is reached and reports
/// that as [`Truncation::Text`] (PaperRust: "some text beats no document", never silently).
pub fn html_to_text_report(html: &str, max_chars: usize) -> Result<(String, Option<Truncation>)> {
    let mut out = Out { sink: TextSink::new(max_chars), at_line_start: true, pending_space: false };
    let mut title = String::new();
    let mut body = Out { sink: TextSink::new(max_chars), at_line_start: true, pending_space: false };
    let walked = walk(html, &mut body, &mut title);
    let mut truncated = body.sink.truncation(walked)?;
    let title = crate::pystr::py_strip(&title).to_owned();
    let mut assemble = || -> Result<()> {
        if !title.is_empty() {
            out.sink.push_str(&title)?;
            out.newline()?;
        }
        out.sink.push_str(&body.sink.buf)
    };
    let assembled = assemble();
    if let Some(t) = out.sink.truncation(assembled)? {
        truncated = Some(t);
    }
    Ok((out.sink.buf, truncated))
}

/// Walks the markup: the title into `title`, the visible text into `body`.
fn walk(s: &str, body: &mut Out, title: &mut String) -> Result<()> {
    let mut pre_depth = 0usize;
    let mut in_head = false;
    let mut i = 0usize;
    let b = s.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            let rest = &s[i..];
            if let Some(after) = rest.strip_prefix("<!--") {
                i += after.find("-->").map(|p| p + 7).unwrap_or(rest.len());
                continue;
            }
            if let Some(after) = rest.strip_prefix("<![CDATA[") {
                let end = after.find("]]>").map(|p| p + 9).unwrap_or(rest.len());
                body.text(&rest[9..end], pre_depth > 0)?;
                i += (end + 3).min(rest.len());
                continue;
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                i += rest.find('>').map(|p| p + 1).unwrap_or(rest.len());
                continue;
            }
            let closing = rest.as_bytes().get(1) == Some(&b'/');
            let name_start = if closing { 2 } else { 1 };
            let nb = rest.as_bytes();
            let mut ne = name_start;
            while ne < nb.len() && (nb[ne].is_ascii_alphanumeric() || nb[ne] == b'-' || nb[ne] == b':') {
                ne += 1;
            }
            if ne == name_start {
                // not a tag ("a < b")
                body.text("<", pre_depth > 0)?;
                i += 1;
                continue;
            }
            let name = rest[name_start..ne].to_ascii_lowercase();
            // End of the tag (respect quotation marks)
            let mut j = ne;
            let mut quote: Option<u8> = None;
            while j < nb.len() {
                let c = nb[j];
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == b'"' || c == b'\'' => quote = Some(c),
                    None if c == b'>' => break,
                    None => {}
                }
                j += 1;
            }
            let tag_end = (j + 1).min(nb.len());
            i += tag_end;
            if closing {
                match name.as_str() {
                    "head" => in_head = false,
                    "pre" => pre_depth = pre_depth.saturating_sub(1),
                    _ => {}
                }
                if name == "td" || name == "th" {
                    body.sink.push('\t')?;
                    body.at_line_start = false;
                    body.pending_space = false;
                } else if is_block(&name) && !body.at_line_start {
                    body.newline()?;
                }
                continue;
            }
            match name.as_str() {
                "script" | "style" => {
                    let close = format!("</{name}");
                    let after = &s[i..];
                    i += find_ci(after, &close).unwrap_or(after.len());
                    continue;
                }
                "title" => {
                    let after = &s[i..];
                    let end = find_ci(after, "</title").unwrap_or(after.len());
                    if title.is_empty() {
                        *title = crate::pystr::collapse_whitespace(&decode_entities(&after[..end]));
                    }
                    i += end;
                    continue;
                }
                "head" => in_head = true,
                "body" => in_head = false,
                "pre" => pre_depth += 1,
                "br" => {
                    body.newline()?;
                    continue;
                }
                _ => {}
            }
            if is_block(&name) && !body.at_line_start {
                body.newline()?;
            }
            continue;
        }
        // Text up to the next '<'
        let next = s[i..].find('<').map(|p| p + i).unwrap_or(s.len());
        if !in_head {
            let t = decode_entities(&s[i..next]);
            body.text(&t, pre_depth > 0)?;
        }
        i = next;
    }
    Ok(())
}

/// Extension: `text/html` as a document (upstream: not supported → "Unsupported mime type").
#[derive(Debug, Default)]
pub struct HtmlDocumentParser;

pub const HTML_MIME_TYPES: &[(&str, &str)] = &[("text/html", ".html"), ("application/xhtml+xml", ".xhtml")];

impl DocumentParser for HtmlDocumentParser {
    fn class_name(&self) -> &'static str {
        "HtmlDocumentParser"
    }
    fn name(&self) -> &'static str {
        "PaperRust HTML Parser"
    }
    fn supported_mime_types(&self) -> &'static [(&'static str, &'static str)] {
        HTML_MIME_TYPES
    }
    fn score(&self, mime_type: &str, _f: &str, _p: Option<&Path>) -> Option<i32> {
        HTML_MIME_TYPES.iter().any(|(m, _)| *m == mime_type).then_some(10)
    }
    fn can_produce_archive(&self) -> bool {
        false
    }
    fn requires_pdf_rendition(&self) -> bool {
        false
    }
    fn parse(&self, path: &Path, _m: &str, ctx: &ParseContext) -> Result<ParsedDocument> {
        let raw = read_limited(path, &ctx.limits)?;
        let html = crate::text::decode_text_bytes(&raw);
        let (text, truncated) = html_to_text_report(&html, ctx.limits.max_text_chars)?;
        Ok(ParsedDocument {
            text: crate::pystr::py_strip(&text).to_owned(),
            date: None,
            archive_path: None,
            truncated: truncated.into_iter().collect(),
        })
    }
    fn page_count(&self, _p: &Path, _m: &str, _c: &ParseContext) -> Option<u32> {
        None
    }
    fn extract_metadata(&self, _p: &Path, _m: &str, _c: &ParseContext) -> Vec<MetadataEntry> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_html() {
        let h = "<html><head><title>T</title><style>p{}</style><script>x<1</script></head>\
                 <body><h1>Kopf</h1><p>Erster&nbsp;Absatz &amp; mehr &lt;x&gt; &#252; &#x00E4; &uuml</p>\
                 <div>Zwei<br>Drei</div><table><tr><td>A</td><td>B</td></tr></table><!-- c --></body></html>";
        let t = html_to_text(h, 1 << 20).unwrap();
        assert_eq!(t, "T\nKopf\nErster\u{a0}Absatz & mehr <x> ü ä ü\nZwei\nDrei\nA\tB\t\n");
    }

    #[test]
    fn broken_html() {
        let t = html_to_text("a < b <p class=\"x>y\">c", 1000).unwrap();
        assert_eq!(t, "a < b\nc");
        assert!(html_to_text("<p>xxxxxxxxxx</p>", 4).is_err());
    }

    #[test]
    fn text_size_limit_keeps_prefix_and_reports_truncation() {
        let h = "<title>T</title><p>Hallo Welt \u{e4}\u{f6}\u{fc} und noch viel mehr</p>";
        let (t, tr) = html_to_text_report(h, 1 << 20).unwrap();
        assert_eq!(t, "T\nHallo Welt \u{e4}\u{f6}\u{fc} und noch viel mehr\n");
        assert_eq!(tr, None, "complete text: nothing reported");
        // 15 bytes end inside the 2-byte "\u{f6}": cut at the character boundary before it
        let (t, tr) = html_to_text_report(h, 15).unwrap();
        assert_eq!(t, "T\nHallo Welt \u{e4}");
        assert_eq!(tr, Some(Truncation::Text { limit: 15 }));
        // the strict variant still fails
        assert!(matches!(html_to_text(h, 15), Err(ParseError::LimitExceeded(_))));
    }

    #[test]
    fn html_parser_sets_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.html");
        std::fs::write(&p, "<p>eins zwei drei vier</p>").unwrap();
        let mut ctx = ParseContext::default();
        let full = HtmlDocumentParser.parse(&p, "text/html", &ctx).unwrap();
        assert!(full.truncated.is_empty());
        assert_eq!(full.truncation_message(), None);
        ctx.limits.max_text_chars = 9;
        let d = HtmlDocumentParser.parse(&p, "text/html", &ctx).unwrap();
        assert_eq!(d.text, "eins zwei");
        assert_eq!(d.truncated, vec![Truncation::Text { limit: 9 }]);
        assert!(d.truncation_message().is_some_and(|m| m.contains("text size limit (9 bytes)")));
    }
}
