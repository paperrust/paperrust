//! Robustness: all parsers over truncated and corrupted sample files – there may only be
//! `Err` or `Ok`, never a crash; time/size limits apply.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pl_parsers::{Limits, ParseContext, ParserRegistry, RegistryConfig, mime};

fn samples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/pl-parsers/samples")
}

/// Simple deterministic randomness (xorshift).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn mutated_samples_never_panic() {
    let reg = ParserRegistry::new(RegistryConfig { tika_enabled: true, extended_formats: true });
    let ctx = ParseContext {
        limits: Limits {
            timeout: Duration::from_secs(10),
            max_text_chars: 4 * 1024 * 1024,
            max_entry_size: 32 * 1024 * 1024,
            ..Limits::default()
        },
        ..ParseContext::default()
    };
    let tmp = tempfile::tempdir().unwrap();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut files: Vec<PathBuf> = std::fs::read_dir(samples()).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();
    let start = Instant::now();
    let mut runs = 0;
    for f in files {
        let data = std::fs::read(&f).unwrap();
        let orig_mime = mime::detect_bytes(&data).to_owned();
        let mut variants: Vec<Vec<u8>> = Vec::new();
        for cut in [1usize, 7, 31, 100, 517] {
            if data.len() > cut {
                variants.push(data[..data.len() * cut / 1000].to_vec());
            }
        }
        for _ in 0..4 {
            let mut v = data.clone();
            if !v.is_empty() {
                for _ in 0..(1 + v.len() / 200) {
                    let i = (rng.next() as usize) % v.len();
                    v[i] = rng.next() as u8;
                }
            }
            variants.push(v);
        }
        for (k, v) in variants.into_iter().enumerate() {
            let p = tmp.path().join(format!("v{k}_{}", f.file_name().unwrap().to_string_lossy()));
            std::fs::write(&p, &v).unwrap();
            let m = mime::detect_file(&p).unwrap();
            // Try parsers for the detected and for the original type
            for mt in [m.as_str(), orig_mime.as_str()] {
                if let Some(parser) = reg.get_parser_for_file(mt, "x", Some(&p), true) {
                    let _ = parser.parse(&p, mt, &ctx);
                    let _ = parser.extract_metadata(&p, mt, &ctx);
                    let _ = parser.page_count(&p, mt, &ctx);
                    runs += 1;
                }
            }
        }
    }
    assert!(runs > 100, "zu wenige Durchläufe: {runs}");
    assert!(start.elapsed() < Duration::from_secs(600));
}

#[test]
fn huge_xml_and_deep_nesting_are_rejected() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    // DOCX with an element nested 2000 levels deep
    let deep = format!(
        "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{}{}</w:body></w:document>",
        "<w:p>".repeat(2000),
        "</w:p>".repeat(2000)
    );
    let path = tmp.path().join("deep.docx");
    {
        let f = std::fs::File::create(&path).unwrap();
        let mut w = std::io::BufWriter::new(f);
        w.write_all(&stored_zip(&[("[Content_Types].xml", b"<Types/>"), ("word/document.xml", deep.as_bytes())])).unwrap();
    }
    let reg = ParserRegistry::default();
    let m = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
    let p = reg.get_parser_for_file(m, "deep.docx", None, true).unwrap();
    let ctx = ParseContext::default();
    assert!(matches!(p.parse(&path, m, &ctx), Err(pl_parsers::ParseError::LimitExceeded(_))));

    // File larger than max_file_size
    let big = tmp.path().join("big.txt");
    std::fs::write(&big, vec![b'a'; 4096]).unwrap();
    let ctx = ParseContext { limits: Limits { max_file_size: 1024, ..Limits::default() }, ..ParseContext::default() };
    let t = reg.get_parser_for_file("text/plain", "big.txt", None, true).unwrap();
    assert!(matches!(t.parse(&big, "text/plain", &ctx), Err(pl_parsers::ParseError::LimitExceeded(_))));

    // Time limit 0 → timeout instead of endless work (large XLSX-like sheet)
    let rows: String = (0..20000).map(|i| format!("<row><c t=\"inlineStr\"><is><t>Zeile {i}</t></is></c></row>")).collect();
    let sheet =
        format!("<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{rows}</sheetData></worksheet>");
    let wb = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
    let xp = tmp.path().join("slow.xlsx");
    std::fs::write(
        &xp,
        stored_zip(&[
            ("[Content_Types].xml", b"<Types/>"),
            ("xl/workbook.xml", wb.as_bytes()),
            ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
            ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ]),
    )
    .unwrap();
    let xm = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
    let ok = p.parse(&xp, xm, &ParseContext::default());
    assert!(ok.as_ref().is_ok_and(|d| d.text.contains("Zeile 19999")), "{ok:?}");
    let ctx0 = ParseContext { limits: Limits { timeout: Duration::from_nanos(1), ..Limits::default() }, ..ParseContext::default() };
    assert!(matches!(p.parse(&xp, xm, &ctx0), Err(pl_parsers::ParseError::Timeout(_))));
}

/// Uncompressed ZIP without CRC (the reader does not check CRCs).
fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut v = Vec::new();
    let mut cd = Vec::new();
    for (name, data) in entries {
        let off = v.len() as u32;
        let mut h = Vec::new();
        h.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        v.extend(h);
        v.extend_from_slice(data);
        cd.extend_from_slice(b"PK\x01\x02\x14\x00\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&[0u8; 12]);
        cd.extend_from_slice(&off.to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
    }
    let cd_off = v.len() as u32;
    let n = entries.len() as u16;
    v.extend_from_slice(&cd);
    v.extend_from_slice(b"PK\x05\x06\x00\x00\x00\x00");
    v.extend_from_slice(&n.to_le_bytes());
    v.extend_from_slice(&n.to_le_bytes());
    v.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    v.extend_from_slice(&cd_off.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v
}
