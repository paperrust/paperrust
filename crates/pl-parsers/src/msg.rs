//! Outlook MSG (`application/vnd.ms-outlook`) – **extension**: upstream does not support MSG
//! (libmagic returns `application/vnd.ms-outlook`, there is no parser for it → "Unsupported
//! mime type"). Only active with `RegistryConfig::extended_formats`.
//!
//! Text is built in the same format as in the EML parser (Subject/From/To/CC/BCC/
//! Attachments/HTML content/Body), so that search and classifier work the same way.
//! Source: [MS-OXMSG] (compound file with `__substg1.0_*` streams), [MS-OXPROPS].

use std::io::{Cursor, Read};
use std::path::Path;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};

use crate::mail::{naturalsize_binary, strip_text};
use crate::{DocumentParser, MetadataEntry, ParseContext, ParseError, ParsedDocument, Result, read_limited};

type Cfb<'a> = cfb::CompoundFile<Cursor<&'a [u8]>>;

fn read_stream(cf: &mut Cfb<'_>, path: &str, max: u64) -> Option<Vec<u8>> {
    let mut s = cf.open_stream(path).ok()?;
    let mut buf = Vec::new();
    (&mut s).take(max).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// String property (Unicode 001F preferred, otherwise 8-bit 001E).
fn prop_string(cf: &mut Cfb<'_>, base: &str, id: u16, max: u64) -> Option<String> {
    if let Some(b) = read_stream(cf, &format!("{base}/__substg1.0_{id:04X}001F"), max) {
        let u: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let s = String::from_utf16_lossy(&u);
        return Some(s.trim_end_matches('\0').to_owned());
    }
    if let Some(b) = read_stream(cf, &format!("{base}/__substg1.0_{id:04X}001E"), max) {
        let (s, _, _) = encoding_rs::WINDOWS_1252.decode(&b);
        return Some(s.trim_end_matches('\0').to_owned());
    }
    None
}

fn prop_binary(cf: &mut Cfb<'_>, base: &str, id: u16, max: u64) -> Option<Vec<u8>> {
    read_stream(cf, &format!("{base}/__substg1.0_{id:04X}0102"), max)
}

/// Fixed properties from `__properties_version1.0` (header: 32 bytes for message, 8 otherwise).
fn fixed_props(cf: &mut Cfb<'_>, base: &str, header: usize) -> Vec<(u16, u16, [u8; 8])> {
    let Some(b) = read_stream(cf, &format!("{base}/__properties_version1.0"), 1 << 20) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut i = header;
    while i + 16 <= b.len() {
        let ty = u16::from_le_bytes([b[i], b[i + 1]]);
        let id = u16::from_le_bytes([b[i + 2], b[i + 3]]);
        let mut v = [0u8; 8];
        v.copy_from_slice(&b[i + 8..i + 16]);
        out.push((id, ty, v));
        i += 16;
    }
    out
}

fn filetime(v: [u8; 8]) -> Option<DateTime<FixedOffset>> {
    let ft = u64::from_le_bytes(v);
    if ft == 0 {
        return None;
    }
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    Utc.timestamp_opt(secs, 0).single().map(|d| d.fixed_offset())
}

#[derive(Debug, Default, Clone)]
pub struct MsgMessage {
    pub subject: String,
    pub sender: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub attachments: Vec<(String, u64)>,
    pub body: String,
    pub html: String,
    pub date: Option<DateTime<FixedOffset>>,
}

fn full(name: &str, email: &str) -> String {
    if !name.is_empty() && !email.is_empty() && name != email {
        format!("{name} <{email}>")
    } else if !name.is_empty() {
        name.to_owned()
    } else {
        email.to_owned()
    }
}

pub fn parse_msg(data: &[u8], max_stream: u64) -> Result<MsgMessage> {
    let mut cf = cfb::CompoundFile::open(Cursor::new(data)).map_err(|e| ParseError::Invalid(format!("MSG: {e}")))?;
    let mut m = MsgMessage { subject: prop_string(&mut cf, "", 0x0037, max_stream).unwrap_or_default(), ..Default::default() };
    let sname = prop_string(&mut cf, "", 0x0C1A, max_stream).unwrap_or_default();
    let smail = prop_string(&mut cf, "", 0x5D01, max_stream).or_else(|| prop_string(&mut cf, "", 0x0C1F, max_stream)).unwrap_or_default();
    m.sender = full(&sname, &smail);
    m.body = prop_string(&mut cf, "", 0x1000, max_stream).unwrap_or_default();
    if let Some(h) = prop_binary(&mut cf, "", 0x1013, max_stream) {
        m.html = String::from_utf8_lossy(&h).into_owned();
    } else if let Some(h) = prop_string(&mut cf, "", 0x1013, max_stream) {
        m.html = h;
    }
    for (id, ty, v) in fixed_props(&mut cf, "", 32) {
        if ty == 0x0040 && (id == 0x0039 || (id == 0x0E06 && m.date.is_none())) {
            m.date = filetime(v);
        }
    }
    let entries: Vec<String> = cf.read_root_storage().filter(|e| e.is_storage()).map(|e| e.name().to_owned()).collect();
    let mut have_recips = false;
    for name in &entries {
        let base = format!("/{name}");
        if name.starts_with("__recip_version1.0_") {
            have_recips = true;
            let dn = prop_string(&mut cf, &base, 0x3001, max_stream).unwrap_or_default();
            let em = prop_string(&mut cf, &base, 0x39FE, max_stream)
                .or_else(|| prop_string(&mut cf, &base, 0x3003, max_stream))
                .unwrap_or_default();
            let kind = fixed_props(&mut cf, &base, 8)
                .into_iter()
                .find(|(id, _, _)| *id == 0x0C15)
                .map(|(_, _, v)| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
                .unwrap_or(1);
            let f = full(&dn, &em);
            match kind {
                2 => m.cc.push(f),
                3 => m.bcc.push(f),
                _ => m.to.push(f),
            }
        } else if name.starts_with("__attach_version1.0_") {
            let fname = prop_string(&mut cf, &base, 0x3707, max_stream)
                .or_else(|| prop_string(&mut cf, &base, 0x3704, max_stream))
                .or_else(|| prop_string(&mut cf, &base, 0x3001, max_stream))
                .unwrap_or_default();
            let size = cf.entry(format!("{base}/__substg1.0_37010102")).map(|e| e.len()).unwrap_or(0);
            m.attachments.push((fname, size));
        }
    }
    if !have_recips {
        let split = |s: Option<String>| -> Vec<String> {
            s.map(|v| v.split(';').map(|x| x.trim().to_owned()).filter(|x| !x.is_empty()).collect()).unwrap_or_default()
        };
        m.to = split(prop_string(&mut cf, "", 0x0E04, max_stream));
        m.cc = split(prop_string(&mut cf, "", 0x0E03, max_stream));
        m.bcc = split(prop_string(&mut cf, "", 0x0E02, max_stream));
    }
    Ok(m)
}

pub fn format_msg(m: &MsgMessage, ctx: &ParseContext) -> Result<String> {
    let mut t = format!("Subject: {}\n\nFrom: {}\n\nTo: {}\n\n", m.subject, m.sender, m.to.join(", "));
    if !m.cc.is_empty() {
        t.push_str(&format!("CC: {}\n\n", m.cc.join(", ")));
    }
    if !m.bcc.is_empty() {
        t.push_str(&format!("BCC: {}\n\n", m.bcc.join(", ")));
    }
    if !m.attachments.is_empty() {
        let v: Vec<String> = m.attachments.iter().map(|(n, s)| format!("{n} ({})", naturalsize_binary(*s))).collect();
        t.push_str(&format!("Attachments: {}\n\n", v.join(", ")));
    }
    if !m.html.is_empty() {
        let h = crate::html::html_to_text(&m.html, ctx.limits.max_text_chars)?;
        t.push_str("HTML content: ");
        t.push_str(&strip_text(&h));
    }
    t.push_str("\n\n");
    t.push_str(&strip_text(&m.body));
    Ok(t)
}

pub const MSG_MIME_TYPES: &[(&str, &str)] = &[("application/vnd.ms-outlook", ".msg")];

#[derive(Debug, Default)]
pub struct MsgDocumentParser;

impl DocumentParser for MsgDocumentParser {
    fn class_name(&self) -> &'static str {
        "MsgDocumentParser"
    }
    fn name(&self) -> &'static str {
        "PaperRust Outlook MSG Parser"
    }
    fn supported_mime_types(&self) -> &'static [(&'static str, &'static str)] {
        MSG_MIME_TYPES
    }
    fn score(&self, mime_type: &str, _f: &str, _p: Option<&Path>) -> Option<i32> {
        (mime_type == "application/vnd.ms-outlook").then_some(10)
    }
    fn can_produce_archive(&self) -> bool {
        false
    }
    fn requires_pdf_rendition(&self) -> bool {
        false
    }
    fn parse(&self, path: &Path, _m: &str, ctx: &ParseContext) -> Result<ParsedDocument> {
        let data = read_limited(path, &ctx.limits)?;
        let m = parse_msg(&data, ctx.limits.max_entry_size)?;
        Ok(ParsedDocument { text: format_msg(&m, ctx)?, date: m.date, archive_path: None })
    }
    fn page_count(&self, _p: &Path, _m: &str, _c: &ParseContext) -> Option<u32> {
        None
    }
    fn extract_metadata(&self, path: &Path, _m: &str, ctx: &ParseContext) -> Vec<MetadataEntry> {
        let Ok(data) = read_limited(path, &ctx.limits) else {
            return Vec::new();
        };
        let Ok(m) = parse_msg(&data, ctx.limits.max_entry_size) else {
            return Vec::new();
        };
        let mut out = vec![MetadataEntry::new(
            "",
            "",
            "attachments",
            m.attachments.iter().map(|(n, s)| format!("{n}({})", naturalsize_binary(*s))).collect::<Vec<_>>().join(", "),
        )];
        if let Some(d) = m.date {
            out.push(MetadataEntry::new("", "", "date", d.format("%Y-%m-%d %H:%M:%S UTC").to_string()));
        }
        for (k, v) in [("from", &m.sender), ("subject", &m.subject)] {
            if !v.is_empty() {
                out.push(MetadataEntry::new("", "header", k, v.clone()));
            }
        }
        if !m.to.is_empty() {
            out.push(MetadataEntry::new("", "header", "to", m.to.join(", ")));
        }
        out.sort_by(|a, b| (a.prefix.as_str(), a.key.as_str()).cmp(&(b.prefix.as_str(), b.key.as_str())));
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    /// Small MSG file per [MS-OXMSG] (for tests and MIME detection).
    pub(crate) fn build_msg() -> Vec<u8> {
        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        let put = |cf: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, p: &str, d: &[u8]| {
            let mut s = cf.create_stream(p).unwrap();
            s.write_all(d).unwrap();
        };
        put(&mut cf, "/__substg1.0_0037001F", &utf16("MSG Betreff"));
        put(&mut cf, "/__substg1.0_0C1A001F", &utf16("Anna Absender"));
        put(&mut cf, "/__substg1.0_5D01001F", &utf16("anna@example.com"));
        put(&mut cf, "/__substg1.0_1000001F", &utf16("Hallo\r\n\r\n  Welt"));
        // Properties: header 32 bytes + PR_CLIENT_SUBMIT_TIME (0x0039, PT_SYSTIME)
        let mut props = vec![0u8; 32];
        props.extend_from_slice(&0x0040u16.to_le_bytes());
        props.extend_from_slice(&0x0039u16.to_le_bytes());
        props.extend_from_slice(&[0u8; 4]);
        // 2025-01-06T08:00:00Z as FILETIME
        let ft: u64 = (1_736_150_400 + 11_644_473_600) * 10_000_000;
        props.extend_from_slice(&ft.to_le_bytes());
        put(&mut cf, "/__properties_version1.0", &props);
        cf.create_storage("/__recip_version1.0_#00000000").unwrap();
        put(&mut cf, "/__recip_version1.0_#00000000/__substg1.0_3001001F", &utf16("Bert"));
        put(&mut cf, "/__recip_version1.0_#00000000/__substg1.0_39FE001F", &utf16("bert@example.com"));
        let mut rp = vec![0u8; 8];
        rp.extend_from_slice(&0x0003u16.to_le_bytes());
        rp.extend_from_slice(&0x0C15u16.to_le_bytes());
        rp.extend_from_slice(&[0u8; 4]);
        rp.extend_from_slice(&2u64.to_le_bytes());
        put(&mut cf, "/__recip_version1.0_#00000000/__properties_version1.0", &rp);
        cf.create_storage("/__attach_version1.0_#00000000").unwrap();
        put(&mut cf, "/__attach_version1.0_#00000000/__substg1.0_3707001F", &utf16("anhang.pdf"));
        put(&mut cf, "/__attach_version1.0_#00000000/__substg1.0_37010102", &[7u8; 1500]);
        cf.flush().unwrap();
        cf.into_inner().into_inner()
    }

    #[test]
    fn msg_parse_and_format() {
        let data = build_msg();
        assert_eq!(crate::mime::detect_bytes(&data), "application/vnd.ms-outlook");
        let m = parse_msg(&data, 1 << 20).unwrap();
        assert_eq!(m.subject, "MSG Betreff");
        assert_eq!(m.sender, "Anna Absender <anna@example.com>");
        assert_eq!(m.cc, vec!["Bert <bert@example.com>".to_owned()]);
        assert_eq!(m.attachments, vec![("anhang.pdf".to_owned(), 1500)]);
        assert_eq!(m.date.unwrap().to_rfc3339(), "2025-01-06T08:00:00+00:00");
        let t = format_msg(&m, &ParseContext::default()).unwrap();
        assert_eq!(
            t,
            "Subject: MSG Betreff\n\nFrom: Anna Absender <anna@example.com>\n\nTo: \n\nCC: Bert <bert@example.com>\n\nAttachments: anhang.pdf (1.46 KiB)\n\n\n\nHallo Welt"
        );
        assert!(parse_msg(b"kein cfb", 1024).is_err());
    }
}
