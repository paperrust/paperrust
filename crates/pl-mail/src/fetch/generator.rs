//! `message.obj.as_bytes()` like CPython 3.12 `email.generator.BytesGenerator` with policy
//! `compat32` (`linesep="\n"`, `max_line_length=78`, `mangle_from_=False`; the `.eml` file with
//! `linesep="\r\n"` since upstream fix a1d76f3a1), plus the
//! "From header to the front" rearrangement from `MailAccountHandler._process_eml`.
//!
//! upstream: src/paperless_mail/mail.py@v3.4.2 (_process_eml), CPython 3.12
//! `email/generator.py` (Generator._write/_dispatch/_write_lines/_handle_text/_handle_multipart/
//! _handle_multipart_signed/_handle_message, BytesGenerator), `email/_policybase.py`
//! (Compat32._fold/fold_binary), `email/header.py` (Header.encode, _ValueFormatter, _Accumulator),
//! `email/feedparser.py` (preamble: the last line ending belongs to the boundary).
//!
//! The MIME tree comes from `pl_parsers::mail::message::Message` (same feedparser re-implementation as
//! the mail parser). It does not store the preamble – it is read here from the raw lines.

use pl_parsers::mail::message::{Message, Payload};

const NL: &[u8] = b"\n";
const CRLF: &[u8] = b"\r\n";
const MAX_LINE_LENGTH: usize = 78;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratorError(pub String);

/// `MailAccountHandler._process_eml`: move the last `From` header line (exact name) to the start
/// (only if its index is not 0), then `as_bytes()` with **CRLF**.
///
/// upstream fix a1d76f3a1 (#14745, paperless-ngx `dev` after v3.4.2): RFC 5322 requires CRLF;
/// with the compat32 default (`\n`) Outlook reads the soft line breaks of quoted-printable as a
/// literal `=`. Hence `as_bytes(policy=policy.clone(linesep="\r\n"))`.
pub fn eml_bytes(msg: &Message) -> Result<Vec<u8>, GeneratorError> {
    eml_bytes_linesep(msg, CRLF)
}

/// [`eml_bytes`] with selectable line ending (`b"\n"` = behaviour of v3.4.2, for the
/// parity check against the v3.4.2 reference).
pub fn eml_bytes_linesep(msg: &Message, nl: &[u8]) -> Result<Vec<u8>, GeneratorError> {
    let mut m = msg.clone();
    let from = m.headers.iter().rposition(|(n, _)| n == "From");
    if let Some(i) = from
        && i > 0
    {
        let h = m.headers.remove(i);
        m.headers.insert(0, h);
    }
    as_bytes_linesep(&m, nl)
}

/// `Message.as_bytes()` (compat32, `linesep="\n"`).
pub fn as_bytes(msg: &Message) -> Result<Vec<u8>, GeneratorError> {
    as_bytes_linesep(msg, NL)
}

/// `Message.as_bytes(policy=msg.policy.clone(linesep=…))`: like [`as_bytes`], but every line ending
/// written by the generator (head, folded header lines, boundaries, normalised
/// text lines) is `nl`. Raw data written unchanged (multipart without parts, 8-bit header)
/// stays untouched like upstream.
pub fn as_bytes_linesep(msg: &Message, nl: &[u8]) -> Result<Vec<u8>, GeneratorError> {
    let mut out = Vec::new();
    flatten(msg, &mut out, MAX_LINE_LENGTH, nl)?;
    Ok(out)
}

fn nl_str(nl: &[u8]) -> &str {
    std::str::from_utf8(nl).unwrap_or("\n")
}

fn flatten(msg: &Message, out: &mut Vec<u8>, maxlen: usize, nl: &[u8]) -> Result<(), GeneratorError> {
    // body first into a buffer (Generator._write), then head + body
    let mut body = Vec::new();
    dispatch(msg, &mut body, maxlen, nl)?;
    write_headers(msg, out, maxlen, nl)?;
    out.extend_from_slice(&body);
    Ok(())
}

fn write_headers(msg: &Message, out: &mut Vec<u8>, maxlen: usize, nl: &[u8]) -> Result<(), GeneratorError> {
    for (name, value) in &msg.headers {
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b": ");
        if value.0.is_ascii() {
            // Header(value, header_name=name).encode(linesep="\n", maxlinelen=78)
            let s = String::from_utf8_lossy(&value.0);
            let folded = header_encode_linesep(&s, name.len() + 2, if maxlen == 0 { 1_000_000 } else { maxlen }, nl_str(nl));
            if embedded_header(&folded) {
                return Err(GeneratorError(format!("header value appears to contain an embedded header: {folded:?}")));
            }
            out.extend_from_slice(folded.as_bytes());
        } else {
            // 8-bit raw data (surrogates): unchanged, without line break
            out.extend_from_slice(&value.0);
        }
        out.extend_from_slice(nl);
    }
    out.extend_from_slice(nl);
    Ok(())
}

/// `_embedded_header = re.compile(r'\n[^ \t]+:')`
fn embedded_header(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\n' {
            // `[^ \t]+` (also `:` and `\n`) followed by `:` → a `:` from i+2 in the run without whitespace
            let mut j = i + 1;
            while j < b.len() && b[j] != b' ' && b[j] != b'\t' {
                if b[j] == b':' && j > i + 1 {
                    return true;
                }
                j += 1;
            }
        }
        i += 1;
    }
    false
}

fn content_type_parts(msg: &Message) -> (String, String) {
    let ct = msg.get_content_type();
    let mut it = ct.splitn(2, '/');
    let main = it.next().unwrap_or("").to_string();
    let sub = it.next().unwrap_or("").to_string();
    (main, sub)
}

fn dispatch(msg: &Message, out: &mut Vec<u8>, maxlen: usize, nl: &[u8]) -> Result<(), GeneratorError> {
    let (main, sub) = content_type_parts(msg);
    match (main.as_str(), sub.as_str()) {
        ("multipart", "signed") => handle_multipart(msg, out, 0, nl),
        ("multipart", _) => handle_multipart(msg, out, maxlen, nl),
        ("message", _) => match &msg.payload {
            Payload::Parts(ps) => {
                if let Some(inner) = ps.first() {
                    flatten(inner, out, maxlen, nl)?;
                }
                Ok(())
            }
            // message/delivery-status (or depth limit): text as written (known difference)
            Payload::Bytes(b) => {
                write_lines(b, out, nl);
                Ok(())
            }
        },
        _ => {
            if let Payload::Bytes(b) = &msg.payload {
                write_lines(b, out, nl);
            }
            Ok(())
        }
    }
}

fn handle_multipart(msg: &Message, out: &mut Vec<u8>, maxlen: usize, nl: &[u8]) -> Result<(), GeneratorError> {
    let parts = match &msg.payload {
        // e.g. no start boundary: payload is text → write raw
        Payload::Bytes(b) => {
            out.extend_from_slice(b);
            return Ok(());
        }
        Payload::Parts(p) => p,
    };
    let mut texts = Vec::with_capacity(parts.len());
    for p in parts {
        let mut s = Vec::new();
        flatten(p, &mut s, maxlen, nl)?;
        texts.push(s);
    }
    let boundary = msg.get_boundary().unwrap_or_default();
    if let Some(pre) = preamble(msg, &boundary) {
        write_lines(&pre, out, nl);
        out.extend_from_slice(nl);
    }
    out.extend_from_slice(b"--");
    out.extend_from_slice(boundary.as_bytes());
    out.extend_from_slice(nl);
    let mut it = texts.into_iter();
    if let Some(first) = it.next() {
        out.extend_from_slice(&first);
    }
    for t in it {
        out.extend_from_slice(nl);
        out.extend_from_slice(b"--");
        out.extend_from_slice(boundary.as_bytes());
        out.extend_from_slice(nl);
        out.extend_from_slice(&t);
    }
    out.extend_from_slice(nl);
    out.extend_from_slice(b"--");
    out.extend_from_slice(boundary.as_bytes());
    out.extend_from_slice(b"--");
    out.extend_from_slice(nl);
    if let Some(e) = &msg.epilogue {
        write_lines(e, out, nl);
    }
    Ok(())
}

/// `_write_lines`: line endings (`\r\n|\r|\n`) → `\n`; a final piece without line ending stays as is.
fn write_lines(data: &[u8], out: &mut Vec<u8>, nl: &[u8]) {
    let mut i = 0;
    let mut start = 0;
    while i < data.len() {
        match data[i] {
            b'\r' => {
                out.extend_from_slice(&data[start..i]);
                out.extend_from_slice(nl);
                if data.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            b'\n' => {
                out.extend_from_slice(&data[start..i]);
                out.extend_from_slice(nl);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < data.len() {
        out.extend_from_slice(&data[start..]);
    }
}

// ---------------------------------------------------------------------------------------------
// Preamble (feedparser): lines between head and first boundary, last line ending removed.

fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b'\n' => {
                out.push(&data[start..=i]);
                start = i + 1;
            }
            b'\r' => {
                if data.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                out.push(&data[start..=i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < data.len() {
        out.push(&data[start..]);
    }
    out
}

fn is_header_line(l: &[u8]) -> bool {
    if l.starts_with(b"From ") || l.first().is_some_and(|&c| c == b' ' || c == b'\t') {
        return true;
    }
    for &c in l {
        if c == b':' {
            return true;
        }
        if !((0x21..=0x39).contains(&c) || (0x3b..=0x7e).contains(&c)) {
            return false;
        }
    }
    false
}

fn is_blank_line(l: &[u8]) -> bool {
    l.starts_with(b"\r\n") || l.starts_with(b"\n") || l.starts_with(b"\r")
}

fn boundary_match(line: &[u8], sep: &[u8]) -> bool {
    let Some(rest) = line.strip_prefix(sep) else { return false };
    let rest = rest.strip_prefix(b"--").unwrap_or(rest);
    let mut i = 0;
    while i < rest.len() && (rest[i] == b' ' || rest[i] == b'\t') {
        i += 1;
    }
    let tail = &rest[i..];
    tail.is_empty() || tail == b"\r\n" || tail == b"\n" || tail == b"\r"
}

fn strip_eol(l: &[u8]) -> &[u8] {
    l.strip_suffix(b"\r\n").or_else(|| l.strip_suffix(b"\n")).or_else(|| l.strip_suffix(b"\r")).unwrap_or(l)
}

fn preamble(msg: &Message, boundary: &str) -> Option<Vec<u8>> {
    let lines = split_lines(&msg.raw);
    let mut pos = 0;
    // skip header lines (feedparser._parse_headers lead-in)
    while pos < lines.len() {
        let l = lines[pos];
        if !is_header_line(l) {
            if is_blank_line(l) {
                pos += 1;
            }
            break;
        }
        pos += 1;
    }
    let sep = format!("--{boundary}").into_bytes();
    let mut pre: Vec<&[u8]> = Vec::new();
    while pos < lines.len() {
        if boundary_match(lines[pos], &sep) {
            break;
        }
        pre.push(lines[pos]);
        pos += 1;
    }
    if pre.is_empty() {
        return None;
    }
    let last = pre.len() - 1;
    let mut v: Vec<u8> = pre[..last].concat();
    v.extend_from_slice(strip_eol(pre[last]));
    Some(v)
}

// ---------------------------------------------------------------------------------------------
// email.header.Header.encode for pure ASCII values (charset us-ascii, no encoding)

#[derive(Debug, Clone, Default)]
struct Accumulator {
    initial_size: usize,
    parts: Vec<(String, String)>,
}

impl Accumulator {
    fn new(initial: usize) -> Self {
        Self { initial_size: initial, parts: Vec::new() }
    }
    fn push(&mut self, fws: &str, s: &str) {
        self.parts.push((fws.to_string(), s.to_string()));
    }
    fn pop(&mut self) -> (String, String) {
        self.parts.pop().unwrap_or_default()
    }
    fn len(&self) -> usize {
        self.parts.iter().map(|(f, p)| f.chars().count() + p.chars().count()).sum::<usize>() + self.initial_size
    }
    fn to_str(&self) -> String {
        self.parts.iter().map(|(f, p)| format!("{f}{p}")).collect()
    }
    fn reset(&mut self, start: Vec<(String, String)>) {
        self.parts = start;
        self.initial_size = 0;
    }
    fn is_onlyws(&self) -> bool {
        self.initial_size == 0 && (self.parts.is_empty() || py_isspace_str(&self.to_str()))
    }
}

fn py_isspace_str(s: &str) -> bool {
    !s.is_empty() && s.chars().all(pl_parsers::pystr::py_isspace)
}

struct ValueFormatter {
    maxlen: usize,
    splitchars: &'static str,
    lines: Vec<String>,
    cur: Accumulator,
}

impl ValueFormatter {
    fn str(&mut self, linesep: &str) -> String {
        self.newline();
        self.lines.join(linesep)
    }

    fn newline(&mut self) {
        let end = self.cur.pop();
        if !(end.0 == " " && end.1.is_empty()) {
            self.cur.parts.push(end);
        }
        if self.cur.len() > 0 {
            if self.cur.is_onlyws() && !self.lines.is_empty() {
                let s = self.cur.to_str();
                if let Some(l) = self.lines.last_mut() {
                    l.push_str(&s);
                }
            } else {
                self.lines.push(self.cur.to_str());
            }
        }
        self.cur.reset(Vec::new());
    }

    fn add_transition(&mut self) {
        self.cur.push(" ", "");
    }

    fn ascii_split(&mut self, fws: &str, string: &str) {
        // re.split("([ \t]+)", fws+string)
        let full = format!("{fws}{string}");
        let mut parts: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut in_ws = false;
        for c in full.chars() {
            let ws = c == ' ' || c == '\t';
            if ws != in_ws {
                parts.push(std::mem::take(&mut cur));
                in_ws = ws;
            }
            cur.push(c);
        }
        parts.push(cur);
        // parts alternate: text, WS, text, …  (first element possibly empty)
        if !parts[0].is_empty() {
            parts.insert(0, String::new());
        } else {
            parts.remove(0);
        }
        let mut i = 0;
        while i + 1 < parts.len() {
            let (f, p) = (parts[i].clone(), parts[i + 1].clone());
            self.append_chunk(&f, &p);
            i += 2;
        }
    }

    fn append_chunk(&mut self, fws: &str, string: &str) {
        self.cur.push(fws, string);
        if self.cur.len() <= self.maxlen {
            return;
        }
        let n = self.cur.parts.len();
        let mut split_at: Option<usize> = None;
        'outer: for ch in self.splitchars.chars() {
            for i in (1..n).rev() {
                if ch.is_whitespace() {
                    let f = &self.cur.parts[i].0;
                    if f.starts_with(ch) {
                        split_at = Some(i);
                        break 'outer;
                    }
                }
                let prev = &self.cur.parts[i - 1].1;
                if prev.ends_with(ch) {
                    split_at = Some(i);
                    break 'outer;
                }
            }
        }
        match split_at {
            None => {
                let (mut f, p) = self.cur.pop();
                if self.cur.initial_size > 0 {
                    self.newline();
                    if f.is_empty() {
                        f = " ".into();
                    }
                }
                self.cur.push(&f, &p);
            }
            Some(i) => {
                let remainder = self.cur.parts.split_off(i);
                self.lines.push(self.cur.to_str());
                self.cur.reset(remainder);
            }
        }
    }
}

/// `str.splitlines()` (Python) for ASCII.
fn py_splitlines(s: &str) -> Vec<&str> {
    pl_parsers::pystr::py_splitlines(s)
}

/// `Header(s, header_name=name).encode(linesep="\n", maxlinelen=maxlen)` for ASCII text.
pub fn header_encode(s: &str, headerlen: usize, maxlen: usize) -> String {
    header_encode_linesep(s, headerlen, maxlen, "\n")
}

/// `Header(s, header_name=name).encode(linesep=linesep, maxlinelen=maxlen)` for ASCII text.
pub fn header_encode_linesep(s: &str, headerlen: usize, maxlen: usize, linesep: &str) -> String {
    let mut f = ValueFormatter { maxlen, splitchars: ";, \t", lines: Vec::new(), cur: Accumulator::new(headerlen) };
    let lines = py_splitlines(s);
    match lines.first() {
        Some(l) => f.ascii_split("", l),
        None => f.ascii_split("", ""),
    }
    for line in lines.iter().skip(1) {
        f.newline();
        let sline = line.trim_start_matches(pl_parsers::pystr::py_isspace);
        let fws = &line[..line.len() - sline.len()];
        f.ascii_split(fws, sline);
    }
    if lines.len() > 1 {
        f.newline();
    }
    f.add_transition();
    f.str(linesep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_header_unchanged() {
        assert_eq!(header_encode("Hello world", 9, 78), "Hello world");
    }

    #[test]
    fn long_header_folds_at_space() {
        let s = "a ".repeat(50);
        let out = header_encode(s.trim_end(), 9, 78);
        assert!(out.lines().all(|l| l.len() <= 78), "{out}");
        assert!(out.contains("\n "));
    }

    #[test]
    fn existing_fold_kept_with_lf() {
        assert_eq!(header_encode("part one\r\n\tpart two", 9, 78), "part one\n\tpart two");
    }

    #[test]
    fn write_lines_normalizes() {
        let mut o = Vec::new();
        write_lines(b"a\r\nb\rc\nd", &mut o, NL);
        assert_eq!(o, b"a\nb\nc\nd");
        let mut o = Vec::new();
        write_lines(b"a\r\nb\rc\nd", &mut o, CRLF);
        assert_eq!(o, b"a\r\nb\r\nc\r\nd");
    }

    /// upstream fix a1d76f3a1 (#14745): the `.eml` file has CRLF throughout (head, folded
    /// header lines, boundaries, quoted-printable text), no bare `\n`.
    #[test]
    fn eml_bytes_use_crlf() {
        let raw = format!(
            "Subject: {}\nTo: a@example.com\nFrom: b@example.com\nMIME-Version: 1.0\n\
             Content-Type: multipart/mixed; boundary=\"XX\"\n\n--XX\n\
             Content-Type: text/plain; charset=utf-8\nContent-Transfer-Encoding: quoted-printable\n\n\
             line one with a soft=\nbreak\nline two\n--XX--\n",
            "word ".repeat(30).trim_end()
        );
        let msg = Message::parse(raw.as_bytes(), 32);
        let out = eml_bytes(&msg).unwrap();
        let s = String::from_utf8_lossy(&out).into_owned();
        assert!(s.starts_with("From: b@example.com\r\n"), "{s}");
        for (i, b) in out.iter().enumerate() {
            if *b == b'\n' {
                assert!(i > 0 && out[i - 1] == b'\r', "bare LF at {i}: {s:?}");
            }
        }
        assert!(s.contains("\r\n word"), "folded Subject uses CRLF: {s:?}");
        assert!(s.contains("soft=\r\nbreak\r\n"), "{s:?}");
        assert!(s.contains("\r\n--XX\r\n") && s.ends_with("--XX--\r\n"), "{s:?}");
        // compat32 default (as_bytes) stays LF
        assert!(!as_bytes(&msg).unwrap().contains(&b'\r'));
    }
}
