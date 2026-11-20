//! RTF → text (replacement for Tika RTFParser). Target destinations like `fonttbl`, `pict`, `*\…`
//! are skipped, `\'hh` decoded via the code page (`\ansicpg`), `\uN` with `\ucN`.

use encoding_rs::Encoding;

use crate::limits::TextSink;
use crate::{Deadline, Limits, ParseError, Result};

use super::OfficeContent;

const SKIP_DESTS: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "pict",
    "object",
    "themedata",
    "colorschememapping",
    "datastore",
    "latentstyles",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "generator",
    "xmlnstbl",
    "mmathPr",
    "pgdsctbl",
    "filetbl",
    "revtbl",
    "fldinst",
    "bkmkstart",
    "bkmkend",
    "nonshppict",
    "header",
    "headerl",
    "headerr",
    "headerf",
    "footer",
    "footerl",
    "footerr",
    "footerf",
    "listtext",
    "pntext",
    "pntxta",
    "pntxtb",
    "private",
    "xe",
    "tc",
    "objdata",
    "blipuid",
    "datafield",
    "fontemb",
    "panose",
    "falt",
    "wgrffmtfilter",
    "defchp",
    "defpap",
    "stylesheet",
    "operator",
    "ftnsep",
    "ftnsepc",
    "aftnsep",
    "aftnsepc",
];

const INFO_FIELDS: &[(&str, &str)] = &[
    ("title", "dc:title"),
    ("subject", "dc:subject"),
    ("author", "dc:creator"),
    ("keywords", "meta:keyword"),
    ("doccomm", "dc:description"),
    ("operator", "meta:last-author"),
    ("company", "extended-properties:Company"),
    ("manager", "extended-properties:Manager"),
    ("category", "cp:category"),
];

#[derive(Clone)]
struct Group {
    skip: bool,
    uc: usize,
    /// current \info field (Tika key)
    info: Option<&'static str>,
    /// \creatim / \revtim
    time: Option<&'static str>,
}

struct Rtf<'a> {
    d: &'a [u8],
    i: usize,
    out: TextSink,
    pending: Vec<u8>,
    enc: &'static Encoding,
    info_vals: Vec<(&'static str, String)>,
    info_cur: Option<(&'static str, String)>,
    times: Vec<(&'static str, [i32; 5])>,
    time_cur: Option<(&'static str, [i32; 5])>,
}

impl Rtf<'_> {
    fn flush(&mut self, g: &Group) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let (s, _, _) = self.enc.decode(&self.pending);
        let s = s.into_owned();
        self.pending.clear();
        self.emit_str(g, &s)
    }
    fn emit_str(&mut self, g: &Group, s: &str) -> Result<()> {
        if let Some((_, v)) = self.info_cur.as_mut()
            && g.info.is_some()
        {
            v.push_str(s);
            return Ok(());
        }
        if g.skip {
            return Ok(());
        }
        self.out.push_str(s)
    }
}

fn codepage(n: i32) -> &'static Encoding {
    let label = match n {
        65001 => "utf-8",
        932 => "shift_jis",
        936 => "gbk",
        949 => "euc-kr",
        950 => "big5",
        874 => "windows-874",
        437 | 850 => "ibm866",
        10000 => "macintosh",
        1250..=1258 => {
            return Encoding::for_label(format!("windows-{n}").as_bytes())
                .unwrap_or(encoding_rs::WINDOWS_1252);
        }
        _ => "windows-1252",
    };
    Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252)
}

pub fn extract(data: &[u8], limits: &Limits, deadline: Deadline) -> Result<OfficeContent> {
    if !data.starts_with(b"{\\rtf") {
        return Err(ParseError::Invalid("not an RTF document".into()));
    }
    let mut r = Rtf {
        d: data,
        i: 0,
        out: TextSink::new(limits.max_text_chars),
        pending: Vec::new(),
        enc: encoding_rs::WINDOWS_1252,
        info_vals: Vec::new(),
        info_cur: None,
        times: Vec::new(),
        time_cur: None,
    };
    let mut stack: Vec<Group> = Vec::new();
    let mut g = Group {
        skip: false,
        uc: 1,
        info: None,
        time: None,
    };
    let mut skip_chars = 0usize;
    let mut steps = 0u64;
    let mut new_group = false;
    while r.i < r.d.len() {
        steps += 1;
        if steps.is_multiple_of(65536) {
            deadline.check()?;
        }
        let c = r.d[r.i];
        match c {
            b'{' => {
                r.flush(&g)?;
                stack.push(g.clone());
                if stack.len() > limits.max_depth {
                    return Err(ParseError::LimitExceeded("RTF nesting too deep".into()));
                }
                new_group = true;
                r.i += 1;
                continue;
            }
            b'}' => {
                r.flush(&g)?;
                if g.info.is_some()
                    && stack.last().is_none_or(|p| p.info != g.info)
                    && let Some((k, v)) = r.info_cur.take()
                {
                    r.info_vals.push((k, v.trim().to_owned()));
                }
                if g.time.is_some()
                    && stack.last().is_none_or(|p| p.time != g.time)
                    && let Some(t) = r.time_cur.take()
                {
                    r.times.push(t);
                }
                g = match stack.pop() {
                    Some(p) => p,
                    None => break,
                };
                new_group = false;
                r.i += 1;
                continue;
            }
            b'\\' => {
                r.i += 1;
                let Some(&n) = r.d.get(r.i) else { break };
                if n.is_ascii_alphabetic() {
                    let ws = r.i;
                    while r.i < r.d.len() && r.d[r.i].is_ascii_alphabetic() && r.i - ws < 32 {
                        r.i += 1;
                    }
                    let word = std::str::from_utf8(&r.d[ws..r.i]).unwrap_or("").to_owned();
                    let mut param: Option<i32> = None;
                    let ps = r.i;
                    if r.i < r.d.len() && (r.d[r.i] == b'-' || r.d[r.i].is_ascii_digit()) {
                        r.i += 1;
                        while r.i < r.d.len() && r.d[r.i].is_ascii_digit() && r.i - ps < 11 {
                            r.i += 1;
                        }
                        param = std::str::from_utf8(&r.d[ps..r.i])
                            .ok()
                            .and_then(|s| s.parse().ok());
                    }
                    if r.i < r.d.len() && r.d[r.i] == b' ' {
                        r.i += 1;
                    }
                    let first = new_group;
                    new_group = false;
                    if skip_chars > 0 && !matches!(word.as_str(), "par" | "line" | "tab") {
                        skip_chars -= 1;
                        continue;
                    }
                    match word.as_str() {
                        "ansicpg" => r.enc = codepage(param.unwrap_or(1252)),
                        "mac" => r.enc = codepage(10000),
                        "pc" | "pca" => r.enc = codepage(437),
                        "uc" => g.uc = param.unwrap_or(1).clamp(0, 16) as usize,
                        "u" => {
                            r.flush(&g)?;
                            let v = param.unwrap_or(0);
                            let v = if v < 0 { v + 65536 } else { v } as u32;
                            let ch = char::from_u32(v).unwrap_or('\u{fffd}');
                            r.emit_str(&g, &ch.to_string())?;
                            skip_chars = g.uc;
                        }
                        "par" | "sect" | "page" | "line" | "row" => {
                            r.flush(&g)?;
                            r.emit_str(&g, "\n")?;
                        }
                        "tab" | "cell" => {
                            r.flush(&g)?;
                            r.emit_str(&g, "\t")?;
                        }
                        "emdash" | "endash" | "bullet" | "lquote" | "rquote" | "ldblquote"
                        | "rdblquote" | "emspace" | "enspace" | "qmspace" => {
                            r.flush(&g)?;
                            let s = match word.as_str() {
                                "emdash" => "\u{2014}",
                                "endash" => "\u{2013}",
                                "bullet" => "\u{2022}",
                                "lquote" => "\u{2018}",
                                "rquote" => "\u{2019}",
                                "ldblquote" => "\u{201c}",
                                "rdblquote" => "\u{201d}",
                                "emspace" => "\u{2003}",
                                "enspace" => "\u{2002}",
                                _ => "\u{2005}",
                            };
                            r.emit_str(&g, s)?;
                        }
                        "bin" => {
                            r.flush(&g)?;
                            let n = param.unwrap_or(0).max(0) as usize;
                            r.i = r.i.saturating_add(n).min(r.d.len());
                        }
                        "info" if first => {
                            r.flush(&g)?;
                            g.skip = true;
                        }
                        "creatim" if first => {
                            g.time = Some("dcterms:created");
                            r.time_cur = Some(("dcterms:created", [0, 1, 1, 0, 0]));
                        }
                        "revtim" if first => {
                            g.time = Some("dcterms:modified");
                            r.time_cur = Some(("dcterms:modified", [0, 1, 1, 0, 0]));
                        }
                        "yr" | "mo" | "dy" | "hr" | "min" if g.time.is_some() => {
                            if let Some((_, t)) = r.time_cur.as_mut() {
                                let idx = match word.as_str() {
                                    "yr" => 0,
                                    "mo" => 1,
                                    "dy" => 2,
                                    "hr" => 3,
                                    _ => 4,
                                };
                                t[idx] = param.unwrap_or(0);
                            }
                        }
                        w if first && INFO_FIELDS.iter().any(|(n, _)| *n == w) => {
                            r.flush(&g)?;
                            let key = INFO_FIELDS
                                .iter()
                                .find(|(n, _)| *n == w)
                                .map(|(_, k)| *k)
                                .unwrap_or("");
                            g.info = Some(key);
                            g.skip = true;
                            r.info_cur = Some((key, String::new()));
                        }
                        w if first && SKIP_DESTS.contains(&w) => {
                            r.flush(&g)?;
                            g.skip = true;
                        }
                        _ => {}
                    }
                    continue;
                }
                // control symbol
                r.i += 1;
                new_group = false;
                match n {
                    b'\'' => {
                        let h =
                            r.d.get(r.i..r.i + 2)
                                .and_then(|x| std::str::from_utf8(x).ok())
                                .and_then(|x| u8::from_str_radix(x, 16).ok());
                        r.i += 2;
                        if skip_chars > 0 {
                            skip_chars -= 1;
                            continue;
                        }
                        if let Some(b) = h
                            && (!g.skip || g.info.is_some())
                        {
                            r.pending.push(b);
                        }
                    }
                    b'*' => {
                        r.flush(&g)?;
                        g.skip = true;
                    }
                    b'~' => {
                        r.flush(&g)?;
                        r.emit_str(&g, "\u{a0}")?;
                    }
                    b'_' => {
                        r.flush(&g)?;
                        r.emit_str(&g, "-")?;
                    }
                    b'-' => {}
                    b'\n' | b'\r' => {
                        r.flush(&g)?;
                        r.emit_str(&g, "\n")?;
                    }
                    b'\\' | b'{' | b'}' => {
                        if skip_chars > 0 {
                            skip_chars -= 1;
                            continue;
                        }
                        r.flush(&g)?;
                        r.emit_str(&g, &(n as char).to_string())?;
                    }
                    _ => {}
                }
                continue;
            }
            b'\r' | b'\n' => {
                r.i += 1;
                continue;
            }
            _ => {
                new_group = false;
                r.i += 1;
                if skip_chars > 0 {
                    skip_chars -= 1;
                    continue;
                }
                if !g.skip || g.info.is_some() {
                    r.pending.push(c);
                }
            }
        }
    }
    r.flush(&g)?;
    let mut md: Vec<(String, String)> = Vec::new();
    for (k, v) in &r.info_vals {
        if !v.is_empty() {
            md.push(((*k).to_owned(), v.clone()));
        }
    }
    let mut created = None;
    for (k, t) in &r.times {
        if t[0] > 0 {
            let v = format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:00",
                t[0], t[1], t[2], t[3], t[4]
            );
            if *k == "dcterms:created" {
                created = Some(v.clone());
            }
            md.push(((*k).to_owned(), v));
        }
    }
    md.push(("Content-Type".into(), "application/rtf".into()));
    Ok(OfficeContent {
        text: r.out.buf,
        metadata: md,
        created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_rtf() {
        let d = b"{\\rtf1\\ansi\\ansicpg1252{\\fonttbl{\\f0 Arial;}}{\\info{\\title RTF Titel}{\\author Erika}{\\creatim\\yr2025\\mo1\\dy6\\hr8\\min0}}\\f0 Erster \\'fc\\'e4\\par\nZweiter\\tab x \\u8364?.\\par{\\pict 89504e}{\\*\\generator X;}Ende\\line Z\\par}";
        let c = extract(
            d,
            &Limits::default(),
            Deadline::from_limits(&Limits::default()),
        )
        .unwrap();
        assert_eq!(c.text, "Erster üä\nZweiter\tx €.\nEnde\nZ\n");
        assert!(
            c.metadata
                .contains(&("dc:title".into(), "RTF Titel".into()))
        );
        assert_eq!(c.created.as_deref(), Some("2025-01-06T08:00:00"));
    }
}
