//! E-mail parser (`.eml`, `message/rfc822`).
//!
//! upstream: src/paperless/parsers/mail.py@v3.4.2 (MailDocumentParser: parse/build_formatted_text,
//! strip_text, extract_metadata, parse_file_to_message), imap_tools 1.15 `message.py`
//! (MailMessage.subject/from_values/to_values/cc_values/bcc_values/date/text/html/headers/
//! attachments, MailAttachment.filename/content_id/payload/size), humanize 4.15 `naturalsize`.
//!
//! HTML content is converted with [`crate::html::html_to_text`] instead of Tika (replaceable
//! via [`HtmlToText`]). The archive PDF (upstream Gotenberg) is produced in [`pdf`] via a
//! plugged-in [`PdfConverter`](crate::office::PdfConverter) (WordCraft in a child process).

pub mod header;
pub mod message;
pub mod pdf;

use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, Duration, FixedOffset, NaiveDateTime, Offset, TimeZone};
use chrono_tz::{OffsetName, Tz};
use regex::Regex;

use crate::{DocumentParser, MetadataEntry, ParseContext, ParseError, ParsedDocument, Result, read_limited};
use header::{EmailAddress, HeaderValue, MailDate};
use message::{Message, Payload};

/// HTML → text (upstream: Tika server). Signature: (html, max_chars).
pub type HtmlToText = fn(&str, usize) -> Result<String>;

/// imap_tools `MailMessage` view of a parsed MIME tree.
#[derive(Debug, Clone)]
pub struct MailMessage {
    pub msg: Message,
}

#[derive(Debug, Clone)]
pub struct MailAttachment {
    pub filename: String,
    pub content_id: String,
    pub content_type: String,
    pub payload: Vec<u8>,
}

impl MailAttachment {
    pub fn size(&self) -> usize {
        self.payload.len()
    }
}

/// humanize `naturalsize(value, binary=True, format="%.2f")`.
pub fn naturalsize_binary(n: u64) -> String {
    const SUF: [&str; 10] = [" KiB", " MiB", " GiB", " TiB", " PiB", " EiB", " ZiB", " YiB", " RiB", " QiB"];
    if n == 1 {
        return "1 Byte".into();
    }
    if n < 1024 {
        return format!("{n} Bytes");
    }
    let v = n as f64;
    let exp = (v.ln() / 1024f64.ln()).min(SUF.len() as f64) as usize;
    let exp = exp.max(1);
    format!("{:.2}{}", v / 1024f64.powi(exp as i32), SUF[exp - 1])
}

/// imap_tools `replace_html_ct_charset(html, 'utf-8')`.
fn replace_html_ct_charset(html: &str) -> String {
    static META: OnceLock<Regex> = OnceLock::new();
    static CS: OnceLock<Regex> = OnceLock::new();
    let meta_re = META.get_or_init(|| Regex::new(r"(?is)<\s*meta .*?content-type.*?>").expect("static regex"));
    let cs_re = CS.get_or_init(|| Regex::new(r"(?i)charset\s*=\s*[a-zA-Z0-9_:.+-]+").expect("static regex"));
    if let Some(m) = meta_re.find(html) {
        let meta = m.as_str();
        let new_meta = cs_re.replacen(meta, 1, "charset=utf-8").into_owned();
        return html.replace(meta, &new_meta);
    }
    html.to_owned()
}

/// Size of `payload_item.as_bytes()` of an embedded message (approximation: raw bytes with
/// normalized line endings; for non-multipart without the last line ending).
fn approx_as_bytes(m: &Message) -> Vec<u8> {
    let mut out = Vec::with_capacity(m.raw.len());
    let r = &m.raw;
    let mut i = 0;
    while i < r.len() {
        if r[i] == b'\r' {
            out.push(b'\n');
            if r.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
        } else {
            out.push(r[i]);
        }
        i += 1;
    }
    if !m.is_multipart() && out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

impl MailMessage {
    pub fn from_bytes(data: &[u8], max_depth: usize) -> Self {
        Self { msg: Message::parse(data, max_depth) }
    }

    pub fn subject(&self) -> String {
        self.msg.get("subject").map(HeaderValue::decoded).unwrap_or_default()
    }

    pub fn from_values(&self) -> Option<EmailAddress> {
        let raw = self.msg.get("from").map(HeaderValue::for_addresses).unwrap_or_default();
        header::parse_email_addresses(&raw).into_iter().next()
    }

    fn addr_values(&self, name: &str) -> Vec<EmailAddress> {
        self.msg.get_all(name).into_iter().flat_map(|v| header::parse_email_addresses(&v.for_addresses())).collect()
    }

    pub fn to_values(&self) -> Vec<EmailAddress> {
        self.addr_values("to")
    }
    pub fn cc_values(&self) -> Vec<EmailAddress> {
        self.addr_values("cc")
    }
    pub fn bcc_values(&self) -> Vec<EmailAddress> {
        self.addr_values("bcc")
    }

    pub fn date_str(&self) -> String {
        self.msg.get("date").map(HeaderValue::py_str).unwrap_or_default()
    }

    pub fn date(&self) -> MailDate {
        header::parse_email_date(&self.date_str())
    }

    fn skip_for_body(p: &Message) -> bool {
        p.get_content_maintype() == "multipart" || p.get_filename().is_some_and(|f| !f.is_empty())
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        for p in self.msg.walk() {
            if Self::skip_for_body(p) {
                continue;
            }
            let ct = p.get_content_type();
            if (ct == "text/plain" || ct == "text/")
                && let Some(b) = p.get_payload_decoded()
            {
                out.push_str(&crate::pystr::decode_value(&b, p.get_content_charset().as_deref()));
            }
        }
        out
    }

    pub fn html(&self) -> String {
        let mut out = String::new();
        for p in self.msg.walk() {
            if Self::skip_for_body(p) {
                continue;
            }
            if p.get_content_type() == "text/html"
                && let Some(b) = p.get_payload_decoded()
            {
                let h = crate::pystr::decode_value(&b, p.get_content_charset().as_deref());
                out.push_str(&replace_html_ct_charset(&h));
            }
        }
        out
    }

    /// LazyHeaders: (lowercase name, all raw values) in order of first occurrence.
    pub fn headers(&self) -> Vec<(String, Vec<&HeaderValue>)> {
        let mut out: Vec<(String, Vec<&HeaderValue>)> = Vec::new();
        for (n, v) in &self.msg.headers {
            let k = n.to_lowercase();
            match out.iter_mut().find(|(key, _)| *key == k) {
                Some((_, vals)) => vals.push(v),
                None => out.push((k, vec![v])),
            }
        }
        out
    }

    pub fn attachments(&self) -> Vec<MailAttachment> {
        let mut out = Vec::new();
        for p in self.msg.walk() {
            if p.get_content_maintype() == "multipart" {
                continue;
            }
            let ct = p.get_content_type();
            if p.get("content-id").is_none() && p.get_filename().is_none() && ct != "message/rfc822" {
                continue;
            }
            out.push(MailAttachment {
                filename: attachment_filename(p),
                content_id: p
                    .get("content-id")
                    .map(|v| v.decoded().trim_start_matches('<').trim_end_matches('>').to_owned())
                    .unwrap_or_default(),
                content_type: ct,
                payload: attachment_payload(p),
            });
        }
        out
    }
}

fn attachment_filename(p: &Message) -> String {
    let raw = p.get_filename().unwrap_or_default();
    let a1 = header::decode_header_joined(&raw);
    if !a1.contains('\u{fffd}') {
        return a1;
    }
    for (hname, target) in [("content-disposition", "filename"), ("content-type", "name")] {
        let Some(hv) = p.get(hname) else { continue };
        if hv.0.is_empty() || hv.is_ascii() {
            continue;
        }
        let s = String::from_utf8_lossy(&hv.0).into_owned();
        for item in header::parseparam(&s) {
            let (name, val) = match item.split_once('=') {
                Some((n, v)) => (n.trim().to_owned(), v.trim().to_owned()),
                None => (item.trim().to_owned(), String::new()),
            };
            if name == target && !val.is_empty() && !val.contains('\u{fffd}') {
                return val.trim_matches('"').to_owned();
            }
        }
    }
    a1
}

fn attachment_payload(p: &Message) -> Vec<u8> {
    if let Some(b) = p.get_payload_decoded()
        && !b.is_empty()
    {
        return b;
    }
    if let Payload::Parts(items) = &p.payload
        && let Some(item) = items.first()
    {
        let bytes = approx_as_bytes(item);
        let cte = p.get_str("content-transfer-encoding").unwrap_or_default().to_lowercase();
        let cte = cte.trim();
        if cte == "base64" {
            return header::b64_decode_lenient(&bytes);
        } else if ["7bit", "8bit", "quoted-printable", "binary", ""].contains(&cte) {
            return bytes;
        }
    }
    Vec::new()
}

/// `strip_text` from `MailDocumentParser.parse`: whitespace runs → one space, `strip()`.
pub fn strip_text(text: &str) -> String {
    // re.sub(r"\s+", " ") leaves no line breaks, (\n *)+ is ineffective afterwards
    crate::pystr::py_strip(&crate::pystr::collapse_whitespace(text)).to_owned()
}

/// Django `make_aware(value)` with `zoneinfo` (fold=0): ambiguous → earlier time,
/// gap → offset before the transition.
pub fn make_aware(naive: NaiveDateTime, tz: Tz) -> (DateTime<FixedOffset>, String) {
    let pick = |off: <Tz as TimeZone>::Offset| -> (DateTime<FixedOffset>, String) {
        let fixed = off.fix();
        let name = off.abbreviation().map(str::to_owned).unwrap_or_else(|| header::fixed_tzname(fixed.local_minus_utc()));
        let dt = fixed.from_local_datetime(&naive).single().unwrap_or_else(|| DateTime::from_naive_utc_and_offset(naive, fixed));
        (dt, name)
    };
    match tz.offset_from_local_datetime(&naive) {
        chrono::LocalResult::Single(o) => pick(o),
        chrono::LocalResult::Ambiguous(a, _) => pick(a),
        chrono::LocalResult::None => {
            let before = naive - Duration::hours(3);
            match tz.offset_from_local_datetime(&before) {
                chrono::LocalResult::Single(o) | chrono::LocalResult::Ambiguous(o, _) => pick(o),
                chrono::LocalResult::None => pick(tz.offset_from_utc_datetime(&naive)),
            }
        }
    }
}

/// Resolved mail date + `%Z` name.
pub fn aware_date(d: MailDate, tz: Tz) -> (DateTime<FixedOffset>, String) {
    match d {
        MailDate::Fixed(dt) => (dt, header::fixed_tzname(dt.offset().local_minus_utc())),
        MailDate::Naive(n) => make_aware(n, tz),
    }
}

/// upstream `MailDocumentParser`.
pub struct MailDocumentParser {
    pub html_to_text: HtmlToText,
    /// PDF generator for the archive PDF (upstream Gotenberg); without it no archive.
    pub converter: Option<std::sync::Arc<dyn crate::office::PdfConverter>>,
}

impl Default for MailDocumentParser {
    fn default() -> Self {
        Self { html_to_text: crate::html::html_to_text, converter: None }
    }
}

impl std::fmt::Debug for MailDocumentParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MailDocumentParser")
    }
}

pub const MAIL_MIME_TYPES: &[(&str, &str)] = &[("message/rfc822", ".eml")];

impl MailDocumentParser {
    /// `parse_file_to_message`.
    pub fn parse_file_to_message(path: &Path, ctx: &ParseContext) -> Result<MailMessage> {
        let p = path.display();
        let data = read_limited(path, &ctx.limits).map_err(|e| ParseError::Upstream(format!("Could not parse {p}: {e}")))?;
        let m = MailMessage::from_bytes(&data, ctx.limits.max_depth.min(64));
        if m.from_values().is_none() {
            return Err(ParseError::Upstream(format!("Could not parse {p}: Could not parse {p}: Missing 'from'")));
        }
        Ok(m)
    }

    /// `build_formatted_text`.
    pub fn build_formatted_text(&self, mail: &MailMessage, ctx: &ParseContext) -> Result<String> {
        let mut t = format!("Subject: {}\n\n", mail.subject());
        t.push_str(&format!("From: {}\n\n", mail.from_values().map(|a| a.full()).unwrap_or_default()));
        let to: Vec<String> = mail.to_values().iter().map(EmailAddress::full).collect();
        t.push_str(&format!("To: {}\n\n", to.join(", ")));
        let cc = mail.cc_values();
        if !cc.is_empty() {
            let v: Vec<String> = cc.iter().map(EmailAddress::full).collect();
            t.push_str(&format!("CC: {}\n\n", v.join(", ")));
        }
        let bcc = mail.bcc_values();
        if !bcc.is_empty() {
            let v: Vec<String> = bcc.iter().map(EmailAddress::full).collect();
            t.push_str(&format!("BCC: {}\n\n", v.join(", ")));
        }
        let atts = mail.attachments();
        if !atts.is_empty() {
            let v: Vec<String> = atts.iter().map(|a| format!("{} ({})", a.filename, naturalsize_binary(a.size() as u64))).collect();
            t.push_str(&format!("Attachments: {}\n\n", v.join(", ")));
        }
        let html = mail.html();
        if !html.is_empty() {
            let converted = (self.html_to_text)(&html, ctx.limits.max_text_chars)?;
            t.push_str("HTML content: ");
            t.push_str(&strip_text(crate::pystr::py_strip(&converted)));
        }
        t.push_str("\n\n");
        t.push_str(&strip_text(&mail.text()));
        Ok(t)
    }

    pub fn metadata_for(mail: &MailMessage, tz: Tz) -> Vec<MetadataEntry> {
        let mut result = Vec::new();
        for (key, vals) in mail.headers() {
            if vals.iter().any(|v| !v.is_ascii()) {
                continue;
            }
            let value: Vec<String> = vals.iter().map(|v| v.py_str()).collect();
            result.push(MetadataEntry::new("", "header", key, value.join(", ")));
        }
        let atts: Vec<String> =
            mail.attachments().iter().map(|a| format!("{}({})", a.filename, naturalsize_binary(a.size() as u64))).collect();
        result.push(MetadataEntry::new("", "", "attachments", atts.join(", ")));
        let (dt, name) = aware_date(mail.date(), tz);
        result.push(MetadataEntry::new("", "", "date", format!("{} {}", dt.format("%Y-%m-%d %H:%M:%S"), name)));
        result.sort_by(|a, b| (a.prefix.as_str(), a.key.as_str()).cmp(&(b.prefix.as_str(), b.key.as_str())));
        result
    }
}

impl DocumentParser for MailDocumentParser {
    fn class_name(&self) -> &'static str {
        "MailDocumentParser"
    }
    fn name(&self) -> &'static str {
        "PaperRust Mail Parser"
    }
    fn supported_mime_types(&self) -> &'static [(&'static str, &'static str)] {
        MAIL_MIME_TYPES
    }
    fn score(&self, mime_type: &str, _f: &str, _p: Option<&Path>) -> Option<i32> {
        (mime_type == "message/rfc822").then_some(10)
    }
    fn can_produce_archive(&self) -> bool {
        false
    }
    /// upstream: `True` (Gotenberg always produces a PDF). Here only with a plugged-in
    /// PDF generator; without it `false` and no archive (see contract, Known differences).
    fn requires_pdf_rendition(&self) -> bool {
        self.converter.is_some()
    }
    fn parse(&self, path: &Path, _m: &str, ctx: &ParseContext) -> Result<ParsedDocument> {
        let mail = Self::parse_file_to_message(path, ctx)?;
        let text = self.build_formatted_text(&mail, ctx)?;
        let (date, _) = aware_date(mail.date(), ctx.time_zone);
        // upstream: with a rule `MailRule.PdfLayout(rule.pdf_layout)`, otherwise
        // `EMAIL_PARSE_DEFAULT_LAYOUT`; the caller sets the effective value in `mail_pdf_layout`.
        let archive_path = match &self.converter {
            Some(conv) => {
                let layout = pdf::PdfLayout::from_i64(ctx.mail_pdf_layout.unwrap_or(1));
                let dir = ctx.work_dir.as_deref().or_else(|| path.parent()).unwrap_or_else(|| Path::new("."));
                match pdf::generate_pdf(&mail, layout, conv.as_ref(), dir, ctx.time_zone) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        // PaperRust: continue without archive instead of aborting (known difference)
                        tracing::warn!(target: "paperless.parsing.mail", "Creating a PDF from the email failed, continuing without archive PDF: {e}");
                        None
                    }
                }
            }
            None => None,
        };
        Ok(ParsedDocument { text, date: Some(date), archive_path })
    }
    fn page_count(&self, _p: &Path, _m: &str, _c: &ParseContext) -> Option<u32> {
        None
    }
    fn extract_metadata(&self, path: &Path, _m: &str, ctx: &ParseContext) -> Vec<MetadataEntry> {
        match Self::parse_file_to_message(path, ctx) {
            Ok(m) => Self::metadata_for(&m, ctx.time_zone),
            Err(e) => {
                tracing::warn!(target: "paperless.parsing.mail", "Error while fetching document metadata for {}: {}", path.display(), e);
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(naturalsize_binary(0), "0 Bytes");
        assert_eq!(naturalsize_binary(1), "1 Byte");
        assert_eq!(naturalsize_binary(1023), "1023 Bytes");
        assert_eq!(naturalsize_binary(1024), "1.00 KiB");
        assert_eq!(naturalsize_binary(2000), "1.95 KiB");
        assert_eq!(naturalsize_binary(1024 * 1024), "1.00 MiB");
    }

    #[test]
    fn ct_charset() {
        assert_eq!(
            replace_html_ct_charset("<meta http-equiv=\"Content-Type\" content=\"text/html; charset=iso-8859-1\">x"),
            "<meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\">x"
        );
    }

    #[test]
    fn make_aware_zurich() {
        let n = chrono::NaiveDate::from_ymd_opt(1900, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
        let (d, name) = make_aware(n, chrono_tz::Europe::Zurich);
        assert_eq!(d.to_rfc3339(), "1900-01-01T00:00:00+01:00");
        assert_eq!(name, "CET");
    }
}
