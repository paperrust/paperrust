//! Email readers (pl-parsers `mail`, `msg`, `html`): eml structure, headers, text/HTML,
//! attachments, date, metadata; Outlook .msg (OLE) and HTML→text.
#![no_main]
use libfuzzer_sys::fuzz_target;
include!("sancov_coff.rs");
use pl_parsers::mail::{MailDocumentParser, MailMessage};

fuzz_target!(|data: &[u8]| {
    if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        if let Ok(m) = pl_parsers::msg::parse_msg(data, 8 << 20) {
            let _ = pl_parsers::msg::format_msg(&m, &pl_parsers::ParseContext::default());
        }
        return;
    }
    let m = MailMessage::from_bytes(data, 32);
    let _ = m.subject();
    let _ = m.from_values();
    let _ = m.to_values();
    let _ = m.cc_values();
    let _ = m.bcc_values();
    let _ = m.date_str();
    let _ = m.date();
    let _ = m.text();
    let html = m.html();
    let _ = pl_parsers::html::html_to_text(&html, 100_000);
    let _ = m.headers();
    for a in m.attachments() {
        let _ = a.size();
    }
    let _ = MailDocumentParser::metadata_for(&m, chrono_tz::Europe::Zurich);
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = pl_parsers::mail::header::decode_header_joined(s);
        let _ = pl_parsers::mail::header::parse_email_addresses(s);
        let _ = pl_parsers::mail::header::parse_email_date(s);
    }
});
