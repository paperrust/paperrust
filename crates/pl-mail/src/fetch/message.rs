//! Fetched message like imap_tools `MailMessage` (uid, subject, from_, from_values, date,
//! attachments incl. `content_disposition`).
//!
//! upstream: imap_tools 1.15.1 `message.py` (MailMessage._get_message_data_parts, uid, subject,
//! from_values, from_, date, attachments, MailAttachment.content_disposition), `consts.py`
//! (UID_PATTERN, DEFAULT_EMAIL_DATE). MIME semantics from `pl_parsers::mail` (CPython compat32 re-implementation).

use chrono::{DateTime, FixedOffset, TimeZone};
use chrono_tz::Tz;
use pl_parsers::mail::MailMessage;
use pl_parsers::mail::header::MailDate;

use super::client::Item;

/// An attachment (`MailAttachment`).
#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    /// `part.get_content_disposition() or ''`
    pub content_disposition: String,
    pub payload: Vec<u8>,
}

/// A fetched message.
#[derive(Debug, Clone)]
pub struct FetchedMessage {
    pub uid: Option<String>,
    pub mail: MailMessage,
    /// Raw message (`message.obj.as_bytes()`), for GPG decryption (M1).
    pub raw: Vec<u8>,
}

/// `UID_PATTERN = r'(^|\s+|\W)UID\s+(?P<uid>\d+)'`
fn find_uid(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut i = 0;
    while let Some(p) = s[i..].find("UID") {
        let at = i + p;
        // before it: start, whitespace or non-word character
        let ok_before = at == 0 || {
            let c = b[at - 1];
            !(c.is_ascii_alphanumeric() || c == b'_') || c.is_ascii_whitespace()
        };
        let mut j = at + 3;
        let ws_start = j;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let d_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if ok_before && j > ws_start && d_start > ws_start && j > d_start {
            return Some(s[d_start..j].to_string());
        }
        i = at + 3;
    }
    None
}

impl FetchedMessage {
    /// `MailMessage(fetch_item)` from a FETCH pair.
    pub fn from_fetch_items(items: &[Item]) -> Self {
        let mut raw_message: Vec<u8> = Vec::new();
        let mut raw_uid: Vec<u8> = Vec::new();
        let mut flag_items: Vec<Vec<u8>> = Vec::new();
        for it in items {
            match it {
                Item::Line(l) => flag_items.push(l.clone()),
                Item::Literal(h, d) => {
                    raw_uid = h.clone();
                    flag_items.push(h.clone());
                    raw_message = d.clone();
                }
            }
        }
        let mut uid = None;
        for cand in std::iter::once(&raw_uid).chain(flag_items.iter()) {
            if let Some(u) = find_uid(&String::from_utf8_lossy(cand)) {
                uid = Some(u);
                break;
            }
        }
        Self { uid, mail: MailMessage::from_bytes(&raw_message, 1000), raw: raw_message }
    }

    /// `MailMessage([(f"UID {message.uid}".encode(), decrypted_message.as_bytes())])`.
    pub fn with_raw(&self, raw: Vec<u8>) -> Self {
        Self { uid: self.uid.clone(), mail: MailMessage::from_bytes(&raw, 1000), raw }
    }

    pub fn uid_str(&self) -> String {
        self.uid.clone().unwrap_or_else(|| "None".into())
    }

    pub fn subject(&self) -> String {
        self.mail.subject()
    }

    /// `from_` (e-mail address of the first sender).
    pub fn from_(&self) -> String {
        self.mail.from_values().map(|a| a.email).unwrap_or_default()
    }

    /// `from_values.name`
    pub fn from_name(&self) -> Option<String> {
        self.mail.from_values().map(|a| a.name)
    }

    /// `message.date`, with `make_aware` (naive → `PAPERLESS_TIME_ZONE`).
    pub fn date_aware(&self, tz: Tz) -> DateTime<FixedOffset> {
        match self.mail.date() {
            MailDate::Fixed(d) => d,
            MailDate::Naive(n) => match tz.from_local_datetime(&n) {
                chrono::LocalResult::Single(d) => d.fixed_offset(),
                // zoneinfo: gap/overlap → fold=0 (first time) – ambiguous: earliest
                chrono::LocalResult::Ambiguous(a, _) => a.fixed_offset(),
                chrono::LocalResult::None => {
                    // gap: zoneinfo takes the offset before the changeover
                    let shifted = n - chrono::Duration::hours(1);
                    tz.from_local_datetime(&shifted)
                        .earliest()
                        .map(|d| (d + chrono::Duration::hours(1)).fixed_offset())
                        .unwrap_or_else(|| tz.from_utc_datetime(&n).fixed_offset())
                }
            },
        }
    }

    /// `attachments` with `content_disposition` (same order as `MailMessage.attachments`).
    pub fn attachments(&self) -> Vec<Attachment> {
        let atts = self.mail.attachments();
        let mut dispositions = Vec::new();
        for p in self.mail.msg.walk() {
            if p.get_content_maintype() == "multipart" {
                continue;
            }
            if p.get("content-id").is_none() && p.get_filename().is_none() && p.get_content_type() != "message/rfc822" {
                continue;
            }
            dispositions.push(p.get_content_disposition().unwrap_or_default());
        }
        atts.into_iter()
            .zip(dispositions)
            .map(|(a, d)| Attachment { filename: a.filename, content_type: a.content_type, content_disposition: d, payload: a.payload })
            .collect()
    }
}

/// Python `s[:n]` (characters).
pub fn py_truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_pattern() {
        assert_eq!(find_uid("3 (UID 15 FLAGS (\\Seen) BODY[] {10}"), Some("15".into()));
        assert_eq!(find_uid("3 (FLAGS (\\Seen) UID 7)"), Some("7".into()));
        assert_eq!(find_uid("3 (XUID 7)"), None);
        assert_eq!(find_uid("(UID 9)"), Some("9".into()));
    }

    #[test]
    fn parse_fetch_pair() {
        let raw = b"From: Alice <alice@example.org>\r\nSubject: Hi\r\nDate: Mon, 06 Jan 2025 08:00:00 +0100\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nbody\r\n--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"a.pdf\"\r\n\r\n%PDF-1.4\r\n--b--\r\n";
        let items = vec![Item::Literal(b"1 (UID 42 FLAGS () BODY[] {10}".to_vec(), raw.to_vec()), Item::Line(b")".to_vec())];
        let m = FetchedMessage::from_fetch_items(&items);
        assert_eq!(m.uid.as_deref(), Some("42"));
        assert_eq!(m.subject(), "Hi");
        assert_eq!(m.from_(), "alice@example.org");
        assert_eq!(m.from_name().as_deref(), Some("Alice"));
        let a = m.attachments();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].filename, "a.pdf");
        assert_eq!(a[0].content_disposition, "attachment");
        assert_eq!(m.date_aware(chrono_tz::Europe::Zurich).to_rfc3339(), "2025-01-06T08:00:00+01:00");
    }
}
