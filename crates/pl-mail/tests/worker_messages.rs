//! Parity of the message evaluation against upstream (imap_tools + CPython email), without DB.
//!
//! Reference: tests/parity/pl-mail-worker/expected/ (generated with `upstream_messages.py` from
//! unmodified upstream code). Compared: subject, from_, from_values.name, date,
//! attachments (name, disposition, type, SHA-256, libmagic MIME) and the .eml bytes of `_process_eml`.

use std::path::PathBuf;

use pl_mail::fetch::client::Item;
use pl_mail::fetch::generator;
use pl_mail::fetch::handler::py_isoformat;
use pl_mail::fetch::message::FetchedMessage;
use serde_json::Value;
use sha2::Digest;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/pl-mail-worker")
}

#[test]
fn messages_match_upstream() {
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(dir().join("expected/messages.json")).unwrap()).unwrap();
    let mut diffs = Vec::new();
    let mut n = 0;
    for (name, exp) in expected.as_object().unwrap() {
        let raw = std::fs::read(dir().join("mails").join(name)).unwrap();
        let items = vec![Item::Literal(format!("1 (UID 1 FLAGS () BODY[] {{{}}}", raw.len()).into_bytes(), raw), Item::Line(b")".to_vec())];
        let m = FetchedMessage::from_fetch_items(&items);
        let mut check = |field: &str, got: Value| {
            n += 1;
            if exp[field] != got {
                diffs.push(format!("{name}.{field}: upstream {} rust {}", exp[field], got));
            }
        };
        check("uid", m.uid.clone().into());
        check("subject", m.subject().into());
        check("from_", m.from_().into());
        check("from_name", m.from_name().map(Value::from).unwrap_or(Value::Null));
        check("date", py_isoformat(&m.date_aware(chrono_tz::Europe::Zurich)).into());
        let atts: Vec<Value> = m
            .attachments()
            .into_iter()
            .map(|a| {
                serde_json::json!({
                    "filename": a.filename,
                    "content_disposition": a.content_disposition,
                    "content_type": a.content_type,
                    "sha256": hex(&sha2::Sha256::digest(&a.payload)),
                    "magic": pl_parsers::mime::detect_bytes(&a.payload),
                })
            })
            .collect();
        check("attachments", Value::Array(atts));
        // The reference is v3.4.2 (compat32, LF). Since upstream fix a1d76f3a1 (#14745) the file
        // is written with CRLF: compare the LF variant with the reference and check that the
        // CRLF variant is the same text with every generator line ending as CRLF.
        let eml = generator::eml_bytes_linesep(&m.mail.msg, b"\n").unwrap();
        let crlf = generator::eml_bytes(&m.mail.msg).unwrap();
        n += 1;
        if lf_from_crlf(&crlf) != eml || crlf.windows(2).filter(|w| w == b"\r\n").count() < eml.iter().filter(|b| **b == b'\n').count() {
            diffs.push(format!("{name}.eml: CRLF variant is not the LF variant with CRLF line endings"));
        }
        let sha = hex(&sha2::Sha256::digest(&eml));
        if exp["eml_sha256"] != sha.as_str() {
            let want = std::fs::read(dir().join("expected").join(name)).unwrap_or_default();
            let pos = want.iter().zip(eml.iter()).position(|(a, b)| a != b).unwrap_or(want.len().min(eml.len()));
            let ctx = |b: &[u8]| String::from_utf8_lossy(&b[pos.saturating_sub(40)..(pos + 40).min(b.len())]).into_owned();
            diffs.push(format!(
                "{name}.eml: differs at byte {pos} (len upstream {} rust {}):\n  upstream {:?}\n  rust     {:?}",
                want.len(),
                eml.len(),
                ctx(&want),
                ctx(&eml)
            ));
        }
        n += 1;
    }
    assert!(diffs.is_empty(), "{} of {n} comparisons differ:\n{}", diffs.len(), diffs.join("\n"));
    eprintln!("{n} comparisons, 0 differences");
}

fn lf_from_crlf(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
