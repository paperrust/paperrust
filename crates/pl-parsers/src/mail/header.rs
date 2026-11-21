//! Header semantics of CPython 3.12 `email` (compat32) and imap_tools 1.15, as upstream uses them
//! via `MailMessage`.
//!
//! upstream: CPython 3.12 `email/header.py` (decode_header), `email/message.py` (_parseparam,
//! _splitparam, get_param, get_filename, get_content_charset), `email/utils.py` (decode_params,
//! collapse_rfc2231_value, unquote, getaddresses strict), `email/_parseaddr.py`
//! (AddrlistClass, _parsedate_tz); imap_tools `utils.py` (decode_value, parse_email_addresses,
//! parse_email_date, EmailAddress.full).

use std::sync::OnceLock;

use chrono::{FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use regex::Regex;

use crate::pystr::{self, Errors, PyCodec};

/// Raw value of a header, as compat32 stores it (folding is preserved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderValue(pub Vec<u8>);

impl HeaderValue {
    pub fn is_ascii(&self) -> bool {
        self.0.is_ascii()
    }
    /// Python `str` of the value: ASCII directly; with 8-bit bytes compat32 yields a
    /// `Header(…, 'unknown-8bit')` whose `str()` replaces every non-ASCII byte with U+FFFD.
    pub fn py_str(&self) -> String {
        self.0.iter().map(|&b| if b < 0x80 { b as char } else { '\u{fffd}' }).collect()
    }
    /// imap_tools: `''.join(decode_value(*p) for p in decode_header(raw))`.
    pub fn decoded(&self) -> String {
        if self.is_ascii() {
            decode_header_joined(&self.py_str())
        } else {
            // Header object: one piece with charset 'unknown-8bit' -> LookupError -> utf-8 ignore
            pystr::decode_utf8_ignore(&self.0)
        }
    }
    /// Value for `parse_email_addresses`: header object → `decode_value(*decode_header(h)[0])`.
    pub fn for_addresses(&self) -> String {
        if self.is_ascii() { self.py_str() } else { pystr::decode_utf8_ignore(&self.0) }
    }
}

fn ecre() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"=\?([^?]*?)\?([qQbB])\?(.*?)\?=").expect("static regex"))
}

/// `email.header.decode_header(str)` → list of (bytes, charset).
pub fn decode_header(header: &str) -> Vec<(Vec<u8>, Option<String>)> {
    if !ecre().is_match(header) {
        return vec![(raw_unicode_escape(header), None)];
    }
    // (encoded_string, encoding, charset)
    let mut words: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    for line in pystr::py_splitlines(header) {
        let mut first = true;
        let mut last = 0;
        let caps: Vec<_> = ecre().captures_iter(line).collect();
        let push_unenc = |s: &str, first: &mut bool, words: &mut Vec<_>| {
            let s = if *first {
                *first = false;
                s.trim_start_matches(pystr::py_isspace)
            } else {
                s
            };
            if !s.is_empty() {
                words.push((s.to_owned(), None, None));
            }
        };
        for c in caps {
            let m = c.get(0).map(|m| (m.start(), m.end())).unwrap_or((0, 0));
            push_unenc(&line[last..m.0], &mut first, &mut words);
            let charset = c.get(1).map(|x| x.as_str().to_lowercase()).unwrap_or_default();
            let encoding = c.get(2).map(|x| x.as_str().to_lowercase()).unwrap_or_default();
            let encoded = c.get(3).map(|x| x.as_str().to_owned()).unwrap_or_default();
            words.push((encoded, Some(encoding), Some(charset)));
            last = m.1;
        }
        push_unenc(&line[last..], &mut first, &mut words);
    }
    // Remove whitespace between two encoded words
    let mut drop = Vec::new();
    for n in 2..words.len() {
        if words[n].1.is_some() && words[n - 2].1.is_some() && !words[n - 1].0.is_empty() && words[n - 1].0.chars().all(pystr::py_isspace) {
            drop.push(n - 1);
        }
    }
    for d in drop.into_iter().rev() {
        words.remove(d);
    }
    let mut decoded: Vec<(Vec<u8>, Option<String>)> = Vec::new();
    for (enc_str, encoding, charset) in words {
        match encoding.as_deref() {
            None => decoded.push((raw_unicode_escape(&enc_str), None)),
            Some("q") => decoded.push((q_decode(&enc_str), charset)),
            Some(_) => decoded.push((b64_decode_lenient(enc_str.as_bytes()), charset)),
        }
    }
    let mut collapsed: Vec<(Vec<u8>, Option<String>)> = Vec::new();
    for (word, charset) in decoded {
        match collapsed.last_mut() {
            Some((lw, lc)) if *lc == charset => {
                if lc.is_none() {
                    lw.push(b' ');
                }
                lw.extend_from_slice(&word);
            }
            _ => collapsed.push((word, charset)),
        }
    }
    collapsed
}

/// `''.join(decode_value(*p) for p in decode_header(s))`; without encoded words: `s` unchanged.
pub fn decode_header_joined(s: &str) -> String {
    if !ecre().is_match(s) {
        return s.to_owned();
    }
    decode_header(s).into_iter().map(|(b, cs)| pystr::decode_value(&b, cs.as_deref())).collect()
}

/// `decode_value(*decode_header(s)[0])` (only the first piece – imap_tools quirk for names).
pub fn decode_header_first(s: &str) -> String {
    if !ecre().is_match(s) {
        return s.to_owned();
    }
    decode_header(s).into_iter().next().map(|(b, cs)| pystr::decode_value(&b, cs.as_deref())).unwrap_or_default()
}

/// `bytes(s, 'raw-unicode-escape')`.
fn raw_unicode_escape(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let u = c as u32;
        if u <= 0xff {
            out.push(u as u8);
        } else if u <= 0xffff {
            out.extend_from_slice(format!("\\u{u:04x}").as_bytes());
        } else {
            out.extend_from_slice(format!("\\U{u:08x}").as_bytes());
        }
    }
    out
}

pub(crate) fn hexval(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// `email.quoprimime.header_decode` + `raw-unicode-escape`.
fn q_decode(s: &str) -> Vec<u8> {
    let s = s.replace('_', " ");
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'='
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hexval(b[i + 1]), hexval(b[i + 2]))
        {
            out.push((h << 4) | l);
            i += 3;
            continue;
        }
        // Characters like raw-unicode-escape (ASCII header → byte)
        let c = s[i..].chars().next().unwrap_or('?');
        out.extend_from_slice(&raw_unicode_escape(&c.to_string()));
        i += c.len_utf8();
    }
    out
}

/// binascii.a2b_base64 (not strict): non-alphabet characters are skipped,
/// missing padding tolerated.
pub fn b64_decode_lenient(input: &[u8]) -> Vec<u8> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut n = 0;
    for &c in input {
        if c == b'=' {
            if n >= 2 {
                break;
            }
            continue;
        }
        if let Some(v) = val(c) {
            acc = (acc << 6) | v as u32;
            n += 1;
            if n == 4 {
                out.push((acc >> 16) as u8);
                out.push((acc >> 8) as u8);
                out.push(acc as u8);
                acc = 0;
                n = 0;
            }
        }
    }
    match n {
        2 => out.push((acc >> 4) as u8),
        3 => {
            out.push((acc >> 10) as u8);
            out.push((acc >> 2) as u8);
        }
        _ => {}
    }
    out
}

// ---------------------------------------------------------------------------
// Parameter (Content-Type / Content-Disposition)
// ---------------------------------------------------------------------------

/// `email.message._splitparam`: (value before `;`, rest).
pub fn splitparam(s: &str) -> (String, Option<String>) {
    match s.split_once(';') {
        Some((a, b)) => (a.trim().to_owned(), Some(b.trim().to_owned())),
        None => (s.trim().to_owned(), None),
    }
}

/// `email.message._parseparam`.
pub fn parseparam(s: &str) -> Vec<String> {
    let mut s = format!(";{s}");
    let mut plist = Vec::new();
    while s.starts_with(';') {
        s.remove(0);
        let count_q = |t: &str, end: usize| -> usize {
            let sub = &t[..end];
            sub.matches('"').count().saturating_sub(sub.matches("\\\"").count())
        };
        let mut end = s.find(';').map(|e| e as isize).unwrap_or(-1);
        while end > 0 && count_q(&s, end as usize) % 2 == 1 {
            end = s[end as usize + 1..].find(';').map(|e| (e + end as usize + 1) as isize).unwrap_or(-1);
        }
        let endu = if end < 0 { s.len() } else { end as usize };
        let mut f = s[..endu].to_owned();
        if let Some(i) = f.find('=') {
            f = format!("{}={}", f[..i].trim().to_lowercase(), f[i + 1..].trim());
        }
        plist.push(f.trim().to_owned());
        s = s[endu..].to_owned();
    }
    plist
}

pub fn unquote(s: &str) -> String {
    if s.chars().count() > 1 {
        if s.starts_with('"') && s.ends_with('"') {
            return s[1..s.len() - 1].replace("\\\\", "\\").replace("\\\"", "\"");
        }
        if s.starts_with('<') && s.ends_with('>') {
            return s[1..s.len() - 1].to_owned();
        }
    }
    s.to_owned()
}

fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamValue {
    Plain(String),
    /// RFC 2231: (charset, language, value)
    Rfc2231(Option<String>, Option<String>, String),
}

/// `latin-1`-`urllib.parse.unquote`.
fn percent_unquote_latin1(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hexval(b[i + 1]), hexval(b[i + 2]))
        {
            out.push(((h << 4) | l) as char);
            i += 3;
            continue;
        }
        let c = s[i..].chars().next().unwrap_or('?');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn rfc2231_name(name: &str) -> Option<(String, Option<u32>)> {
    // ^(?P<name>\w+)\*((?P<num>[0-9]+)\*?)?$
    let star = name.find('*')?;
    let base = &name[..star];
    if base.is_empty() || !base.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let rest = &name[star + 1..];
    if rest.is_empty() {
        return Some((base.to_owned(), None));
    }
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let after = &rest[digits.len()..];
    if !(after.is_empty() || after == "*") {
        return None;
    }
    Some((base.to_owned(), digits.parse().ok()))
}

/// (number, value, encoded) of an RFC 2231 continuation part.
type Rfc2231Part = (Option<u32>, String, bool);

/// `Message._get_params_preserve` (incl. `utils.decode_params`).
pub fn params(value: &str) -> Vec<(String, ParamValue)> {
    let mut raw = Vec::new();
    for p in parseparam(value) {
        match p.split_once('=') {
            Some((n, v)) => raw.push((n.trim().to_owned(), v.trim().to_owned())),
            None => raw.push((p.trim().to_owned(), String::new())),
        }
    }
    if raw.is_empty() {
        return Vec::new();
    }
    let mut out = vec![(raw[0].0.clone(), ParamValue::Plain(raw[0].1.clone()))];
    // name -> [(num, value, encoded)] in insertion order
    let mut rfc: Vec<(String, Vec<Rfc2231Part>)> = Vec::new();
    for (name, value) in raw.into_iter().skip(1) {
        let encoded = name.ends_with('*');
        let value = unquote(&value);
        if let Some((base, num)) = rfc2231_name(&name) {
            match rfc.iter_mut().find(|(n, _)| *n == base) {
                Some((_, v)) => v.push((num, value, encoded)),
                None => rfc.push((base, vec![(num, value, encoded)])),
            }
        } else {
            out.push((name, ParamValue::Plain(format!("\"{}\"", quote(&value)))));
        }
    }
    for (name, mut conts) in rfc {
        // Python sorts tuples; None < int is a TypeError in Py3 – only occurs with mixed
        // forms, then like upstream: we sort None first.
        conts.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut parts = Vec::new();
        let mut extended = false;
        for (_, s, enc) in conts {
            if enc {
                parts.push(percent_unquote_latin1(&s));
                extended = true;
            } else {
                parts.push(s);
            }
        }
        let value = quote(&parts.concat());
        if extended {
            let sp: Vec<&str> = value.splitn(3, '\'').collect();
            if sp.len() <= 2 {
                out.push((name, ParamValue::Rfc2231(None, None, format!("\"{value}\""))));
            } else {
                out.push((name, ParamValue::Rfc2231(Some(sp[0].to_owned()), Some(sp[1].to_owned()), format!("\"{}\"", sp[2]))));
            }
        } else {
            out.push((name, ParamValue::Plain(format!("\"{value}\""))));
        }
    }
    out
}

/// `get_param(param)` with `unquote=True` from a header value.
pub fn get_param(header: &str, param: &str) -> Option<ParamValue> {
    for (k, v) in params(header) {
        if k.to_lowercase() == param.to_lowercase() {
            return Some(match v {
                ParamValue::Plain(s) => ParamValue::Plain(unquote(&s)),
                ParamValue::Rfc2231(c, l, s) => ParamValue::Rfc2231(c, l, unquote(&s)),
            });
        }
    }
    None
}

/// `utils.collapse_rfc2231_value(value)` (errors='replace', fallback 'us-ascii').
pub fn collapse_rfc2231(v: &ParamValue) -> String {
    match v {
        ParamValue::Plain(s) => unquote(s),
        ParamValue::Rfc2231(charset, _, text) => {
            let cs = charset.clone().unwrap_or_else(|| "us-ascii".into());
            let raw = raw_unicode_escape(text);
            match pystr::lookup_codec(&cs) {
                Some(c) => pystr::decode_with(c, &raw, Errors::Replace),
                None => unquote(text),
            }
        }
    }
}

/// `get_content_charset()` from the Content-Type value.
pub fn content_charset(ct: &str) -> Option<String> {
    let v = get_param(ct, "charset")?;
    let cs = match v {
        ParamValue::Plain(s) => s,
        ParamValue::Rfc2231(c, _, s) => {
            let pc = c.unwrap_or_else(|| "us-ascii".into());
            let raw = raw_unicode_escape(&s);
            {
                let codec = pystr::lookup_codec(&pc)?;
                pystr::decode_with(codec, &raw, Errors::Replace)
            }
        }
    };
    if !cs.is_ascii() {
        return None;
    }
    Some(cs.to_lowercase())
}

// ---------------------------------------------------------------------------
// Addresses
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailAddress {
    pub name: String,
    pub email: String,
}

impl EmailAddress {
    /// imap_tools `EmailAddress.full`.
    pub fn full(&self) -> String {
        if !self.name.is_empty() && !self.email.is_empty() {
            format!("{} <{}>", self.name, self.email)
        } else if !self.name.is_empty() {
            self.name.clone()
        } else {
            self.email.clone()
        }
    }
}

struct AddrList {
    f: Vec<char>,
    pos: usize,
    commentlist: Vec<String>,
}

const SPECIALS: &str = "()<>@,:;.\"[]";

impl AddrList {
    fn at(&self) -> char {
        self.f[self.pos]
    }
    fn is_atomend(c: char) -> bool {
        SPECIALS.contains(c) || c == ' ' || c == '\t' || c == '\r' || c == '\n'
    }
    fn is_phraseend(c: char) -> bool {
        c != '.' && Self::is_atomend(c)
    }
    fn gotonext(&mut self) -> String {
        let mut ws = String::new();
        while self.pos < self.f.len() {
            let c = self.at();
            if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
                if c != '\n' && c != '\r' {
                    ws.push(c);
                }
                self.pos += 1;
            } else if c == '(' {
                let cm = self.getcomment();
                self.commentlist.push(cm);
            } else {
                break;
            }
        }
        ws
    }
    fn getaddrlist(&mut self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        while self.pos < self.f.len() {
            let before = self.pos;
            let ad = self.getaddress(0);
            if ad.is_empty() {
                result.push((String::new(), String::new()));
            } else {
                result.extend(ad);
            }
            if self.pos == before {
                self.pos += 1;
            }
        }
        result
    }
    fn getaddress(&mut self, depth: usize) -> Vec<(String, String)> {
        self.commentlist = Vec::new();
        self.gotonext();
        let oldpos = self.pos;
        let oldcl = self.commentlist.clone();
        let plist = self.getphraselist();
        self.gotonext();
        let mut ret = Vec::new();
        if self.pos >= self.f.len() {
            if let Some(p0) = plist.first() {
                ret = vec![(self.commentlist.join(" "), p0.clone())];
            }
        } else if self.at() == '.' || self.at() == '@' {
            self.pos = oldpos;
            self.commentlist = oldcl;
            let a = self.getaddrspec();
            ret = vec![(self.commentlist.join(" "), a)];
        } else if self.at() == ':' {
            self.pos += 1;
            while self.pos < self.f.len() && depth < 32 {
                self.gotonext();
                if self.pos < self.f.len() && self.at() == ';' {
                    self.pos += 1;
                    break;
                }
                let before = self.pos;
                let mut sub = self.getaddress(depth + 1);
                ret.append(&mut sub);
                if self.pos == before {
                    self.pos += 1;
                }
            }
        } else if self.at() == '<' {
            let r = self.getrouteaddr();
            if !self.commentlist.is_empty() {
                ret = vec![(format!("{} ({})", plist.join(" "), self.commentlist.join(" ")), r)];
            } else {
                ret = vec![(plist.join(" "), r)];
            }
        } else if let Some(p0) = plist.first() {
            ret = vec![(self.commentlist.join(" "), p0.clone())];
        } else if SPECIALS.contains(self.at()) {
            self.pos += 1;
        }
        self.gotonext();
        if self.pos < self.f.len() && self.at() == ',' {
            self.pos += 1;
        }
        ret
    }
    fn getrouteaddr(&mut self) -> String {
        if self.at() != '<' {
            return String::new();
        }
        let mut expectroute = false;
        self.pos += 1;
        self.gotonext();
        let mut adlist = String::new();
        while self.pos < self.f.len() {
            if expectroute {
                self.getdomain();
                expectroute = false;
            } else if self.at() == '>' {
                self.pos += 1;
                break;
            } else if self.at() == '@' {
                self.pos += 1;
                expectroute = true;
            } else if self.at() == ':' {
                self.pos += 1;
            } else {
                adlist = self.getaddrspec();
                self.pos += 1;
                break;
            }
            self.gotonext();
        }
        adlist
    }
    fn getaddrspec(&mut self) -> String {
        let mut aslist: Vec<String> = Vec::new();
        self.gotonext();
        while self.pos < self.f.len() {
            let mut preserve_ws = true;
            let c = self.at();
            if c == '.' {
                if aslist.last().is_some_and(|l| l.trim().is_empty()) {
                    aslist.pop();
                }
                aslist.push(".".into());
                self.pos += 1;
                preserve_ws = false;
            } else if c == '"' {
                let q = self.getquote();
                aslist.push(format!("\"{}\"", quote(&q)));
            } else if Self::is_atomend(c) {
                if aslist.last().is_some_and(|l| l.trim().is_empty()) {
                    aslist.pop();
                }
                break;
            } else {
                let a = self.getatom(false);
                aslist.push(a);
            }
            let ws = self.gotonext();
            if preserve_ws && !ws.is_empty() {
                aslist.push(ws);
            }
        }
        if self.pos >= self.f.len() || self.at() != '@' {
            return aslist.concat();
        }
        aslist.push("@".into());
        self.pos += 1;
        self.gotonext();
        let domain = self.getdomain();
        if domain.is_empty() {
            return String::new();
        }
        aslist.concat() + &domain
    }
    fn getdomain(&mut self) -> String {
        let mut sd = String::new();
        while self.pos < self.f.len() {
            let c = self.at();
            if c == ' ' || c == '\t' {
                self.pos += 1;
            } else if c == '(' {
                let cm = self.getcomment();
                self.commentlist.push(cm);
            } else if c == '[' {
                let d = self.getdelimited('[', "]\r", false);
                sd.push_str(&format!("[{d}]"));
            } else if c == '.' {
                self.pos += 1;
                sd.push('.');
            } else if c == '@' {
                return String::new();
            } else if Self::is_atomend(c) {
                break;
            } else {
                let a = self.getatom(false);
                sd.push_str(&a);
            }
        }
        sd
    }
    fn getdelimited(&mut self, begin: char, ends: &str, allow_comments: bool) -> String {
        if self.at() != begin {
            return String::new();
        }
        let mut s = String::new();
        let mut quote = false;
        self.pos += 1;
        while self.pos < self.f.len() {
            let c = self.at();
            if quote {
                s.push(c);
                quote = false;
            } else if ends.contains(c) {
                self.pos += 1;
                break;
            } else if allow_comments && c == '(' {
                let cm = self.getcomment();
                s.push_str(&cm);
                continue;
            } else if c == '\\' {
                quote = true;
            } else {
                s.push(c);
            }
            self.pos += 1;
        }
        s
    }
    fn getquote(&mut self) -> String {
        self.getdelimited('"', "\"\r", false)
    }
    fn getcomment(&mut self) -> String {
        self.getdelimited('(', ")\r", true)
    }
    fn getatom(&mut self, phrase: bool) -> String {
        let mut s = String::new();
        while self.pos < self.f.len() {
            let c = self.at();
            let end = if phrase { Self::is_phraseend(c) } else { Self::is_atomend(c) };
            if end {
                break;
            }
            s.push(c);
            self.pos += 1;
        }
        s
    }
    fn getphraselist(&mut self) -> Vec<String> {
        let mut plist = Vec::new();
        while self.pos < self.f.len() {
            let c = self.at();
            if c == ' ' || c == '\t' || c == '\r' || c == '\n' {
                self.pos += 1;
            } else if c == '"' {
                let q = self.getquote();
                plist.push(q);
            } else if c == '(' {
                let cm = self.getcomment();
                self.commentlist.push(cm);
            } else if Self::is_phraseend(c) {
                break;
            } else {
                let a = self.getatom(true);
                plist.push(a);
            }
        }
        plist
    }
}

fn iter_escaped(addr: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = addr.chars().collect();
    let mut out = Vec::new();
    let mut escape = false;
    let mut pos = 0;
    for (p, &ch) in chars.iter().enumerate() {
        pos = p;
        if escape {
            out.push((p, format!("\\{ch}")));
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else {
            out.push((p, ch.to_string()));
        }
    }
    if escape {
        out.push((pos, "\\".into()));
    }
    out
}

fn strip_quoted_realnames(addr: &str) -> String {
    if !addr.contains('"') {
        return addr.to_owned();
    }
    let chars: Vec<char> = addr.chars().collect();
    let mut start = 0;
    let mut open: Option<usize> = None;
    let mut result = String::new();
    for (pos, ch) in iter_escaped(addr) {
        if ch == "\"" {
            match open {
                None => open = Some(pos),
                Some(o) => {
                    if start != o {
                        result.extend(&chars[start..o]);
                    }
                    start = pos + 1;
                    open = None;
                }
            }
        }
    }
    if start < chars.len() {
        result.extend(&chars[start..]);
    }
    result
}

fn check_parenthesis(addr: &str) -> bool {
    let a = strip_quoted_realnames(addr);
    let mut opens: i64 = 0;
    for (_, ch) in iter_escaped(&a) {
        if ch == "(" {
            opens += 1;
        } else if ch == ")" {
            opens -= 1;
            if opens < 0 {
                return false;
            }
        }
    }
    opens == 0
}

/// `email.utils.getaddresses([value], strict=True)`.
pub fn getaddresses(values: &[String]) -> Vec<(String, String)> {
    let vals: Vec<String> = values.iter().map(|v| if check_parenthesis(v) { v.clone() } else { "('', '')".to_owned() }).collect();
    let addr = vals.join(", ");
    let mut list = if addr.is_empty() {
        Vec::new()
    } else {
        let mut p = AddrList { f: addr.chars().collect(), pos: 0, commentlist: Vec::new() };
        p.getaddrlist()
    };
    for v in list.iter_mut() {
        if v.1.contains('[') {
            *v = (String::new(), String::new());
        }
    }
    let n: usize = vals.iter().map(|v| 1 + strip_quoted_realnames(v).matches(',').count()).sum();
    if list.len() != n {
        return vec![(String::new(), String::new())];
    }
    list
}

/// imap_tools `parse_email_addresses(raw_header)`.
pub fn parse_email_addresses(raw: &str) -> Vec<EmailAddress> {
    let cleaned: String = raw.chars().filter(|&c| pystr::py_isprintable(c)).collect();
    let mut out = Vec::new();
    for (raw_name, email) in getaddresses(&[cleaned]) {
        let name = pystr::py_strip(&decode_header_first(&raw_name)).to_owned();
        let email = pystr::py_strip(&email).to_owned();
        if name.is_empty() && email.is_empty() {
            continue;
        }
        let has_at = email.contains('@');
        out.push(EmailAddress {
            name: if !name.is_empty() {
                name
            } else if !has_at {
                email.clone()
            } else {
                String::new()
            },
            email: if has_at { email } else { String::new() },
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Date
// ---------------------------------------------------------------------------

/// Mail date like imap_tools `parse_email_date`: naive or with fixed offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDate {
    Naive(NaiveDateTime),
    Fixed(chrono::DateTime<FixedOffset>),
}

const MONTHS: [&str; 24] = [
    "jan",
    "feb",
    "mar",
    "apr",
    "may",
    "jun",
    "jul",
    "aug",
    "sep",
    "oct",
    "nov",
    "dec",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Python `int(str)` (sign, whitespace, underscores between digits).
fn py_int(s: &str) -> Option<i64> {
    let t = pystr::py_strip(s);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let clean: String = digits.chars().filter(|&c| c != '_').collect();
    if !clean.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: i64 = clean.parse().ok()?;
    Some(if neg { -v } else { v })
}

/// `email._parseaddr._parsedate_tz`.
#[allow(clippy::type_complexity)]
pub fn parsedate_tz(data: &str) -> Option<(i64, i64, i64, i64, i64, i64, Option<i64>)> {
    let mut data: Vec<String> = data.split_whitespace().map(str::to_owned).collect();
    if data.is_empty() {
        return None;
    }
    if data[0].ends_with(',') || DAYS.contains(&data[0].to_lowercase().as_str()) {
        data.remove(0);
    } else if let Some(i) = data[0].rfind(',') {
        data[0] = data[0][i + 1..].to_owned();
    }
    if data.len() == 3 {
        let stuff: Vec<String> = data[0].split('-').map(str::to_owned).collect();
        if stuff.len() == 3 {
            let mut nd = stuff;
            nd.extend(data[1..].iter().cloned());
            data = nd;
        }
    }
    if data.len() == 4 {
        let s = data[3].clone();
        let i = s.find('+').or_else(|| s.find('-'));
        match i {
            Some(i) if i > 0 => {
                data.truncate(3);
                data.push(s[..i].to_owned());
                data.push(s[i..].to_owned());
            }
            _ => data.push(String::new()),
        }
    }
    if data.len() < 5 {
        return None;
    }
    data.truncate(5);
    let (mut dd, mut mm, mut yy, mut tm, mut tz) = (data[0].clone(), data[1].clone(), data[2].clone(), data[3].clone(), data[4].clone());
    if dd.is_empty() || mm.is_empty() || yy.is_empty() {
        return None;
    }
    mm = mm.to_lowercase();
    if !MONTHS.contains(&mm.as_str()) {
        let old_dd = dd.to_lowercase();
        dd = mm;
        mm = old_dd;
        if !MONTHS.contains(&mm.as_str()) {
            return None;
        }
    }
    let mut mon = MONTHS.iter().position(|m| *m == mm)? as i64 + 1;
    if mon > 12 {
        mon -= 12;
    }
    if dd.ends_with(',') {
        dd.pop();
    }
    if let Some(i) = yy.find(':')
        && i > 0
    {
        std::mem::swap(&mut yy, &mut tm);
    }
    if yy.ends_with(',') {
        yy.pop();
        if yy.is_empty() {
            return None;
        }
    }
    if !yy.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        std::mem::swap(&mut yy, &mut tz);
    }
    if tm.ends_with(',') {
        tm.pop();
    }
    let parts: Vec<&str> = tm.split(':').collect();
    let (thh, tmm, tss): (String, String, String) = match parts.len() {
        2 => (parts[0].into(), parts[1].into(), "0".into()),
        3 => (parts[0].into(), parts[1].into(), parts[2].into()),
        1 if parts[0].contains('.') => {
            let p: Vec<&str> = parts[0].split('.').collect();
            match p.len() {
                2 => (p[0].into(), p[1].into(), "0".into()),
                3 => (p[0].into(), p[1].into(), p[2].into()),
                _ => return None,
            }
        }
        _ => return None,
    };
    let mut yy = py_int(&yy)?;
    let dd = py_int(&dd)?;
    let thh = py_int(&thh)?;
    let tmm = py_int(&tmm)?;
    let tss = py_int(&tss)?;
    if yy < 100 {
        if yy > 68 {
            yy += 1900;
        } else {
            yy += 2000;
        }
    }
    let tzu = tz.to_uppercase();
    let mut tzoffset: Option<i64> = match tzu.as_str() {
        "UT" | "UTC" | "GMT" | "Z" => Some(0),
        "AST" => Some(-400),
        "ADT" => Some(-300),
        "EST" => Some(-500),
        "EDT" => Some(-400),
        "CST" => Some(-600),
        "CDT" => Some(-500),
        "MST" => Some(-700),
        "MDT" => Some(-600),
        "PST" => Some(-800),
        "PDT" => Some(-700),
        _ => {
            let v = py_int(&tzu);
            if v == Some(0) && tzu.starts_with('-') { None } else { v }
        }
    };
    if let Some(o) = tzoffset
        && o != 0
    {
        let (sign, a) = if o < 0 { (-1, -o) } else { (1, o) };
        tzoffset = Some(sign * ((a / 100) * 3600 + (a % 100) * 60));
    }
    Some((yy, mon, dd, thh, tmm, tss, tzoffset))
}

fn mk_naive(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<NaiveDateTime> {
    let y = i32::try_from(y).ok()?;
    if !(1..=9999).contains(&y) {
        return None;
    }
    NaiveDate::from_ymd_opt(y, u32::try_from(mo).ok()?, u32::try_from(d).ok()?)?.and_hms_opt(
        u32::try_from(h).ok()?,
        u32::try_from(mi).ok()?,
        u32::try_from(s).ok()?,
    )
}

/// `email.utils.parsedate_to_datetime` (error → `None`).
pub fn parsedate_to_datetime(data: &str) -> Option<MailDate> {
    let (y, mo, d, h, mi, s, tz) = parsedate_tz(data)?;
    let naive = mk_naive(y, mo, d, h, mi, s)?;
    match tz {
        None => Some(MailDate::Naive(naive)),
        Some(off) => {
            if off.abs() >= 86_400 {
                return None;
            }
            let fo = FixedOffset::east_opt(off as i32)?;
            Some(MailDate::Fixed(fo.from_local_datetime(&naive).single()?))
        }
    }
}

/// imap_tools `parse_email_date` (fallback: regex, then 1900-01-01 naive).
pub fn parse_email_date(value: &str) -> MailDate {
    if let Some(d) = parsedate_to_datetime(value) {
        return d;
    }
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        Regex::new(
            r"(?P<date>\d{1,2}\s+(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{4})\s+(?P<time>\d{1,2}:\d{1,2}(:\d{1,2})?)\s*(?P<zone_sign>[+-])?(?P<zone>\d{4})?",
        )
        .expect("static regex")
    });
    if let Some(c) = re.captures(value) {
        let date: Vec<&str> = c["date"].split_whitespace().collect();
        let time: Vec<&str> = c["time"].split(':').collect();
        let sign = if c.name("zone_sign").map(|m| m.as_str()) == Some("-") { -1 } else { 1 };
        let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let mo = months.iter().position(|m| *m == date[1]).map(|p| p as i64 + 1).unwrap_or(1);
        let parse = |s: &str| s.parse::<i64>().ok();
        if let (Some(y), Some(d), Some(h), Some(mi)) = (parse(date[2]), parse(date[0]), parse(time[0]), parse(time[1])) {
            let s = time.get(2).and_then(|x| parse(x)).unwrap_or(0);
            if let Some(naive) = mk_naive(y, mo, d, h, mi, s) {
                match c.name("zone") {
                    Some(z) => {
                        let z = z.as_str();
                        let hh: i64 = z[..2].parse().unwrap_or(0);
                        let mm: i64 = z[2..].parse().unwrap_or(0);
                        let off = sign * (hh * 3600 + mm * 60);
                        if off.abs() < 86_400
                            && let Some(fo) = FixedOffset::east_opt(off as i32)
                            && let Some(dt) = fo.from_local_datetime(&naive).single()
                        {
                            return MailDate::Fixed(dt);
                        }
                    }
                    None => return MailDate::Naive(naive),
                }
            }
        }
    }
    MailDate::Naive(NaiveDate::from_ymd_opt(1900, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).unwrap_or_default())
}

/// Python `timezone(timedelta(seconds=off)).tzname(None)`.
pub fn fixed_tzname(off_secs: i32) -> String {
    if off_secs == 0 {
        return "UTC".into();
    }
    let sign = if off_secs < 0 { '-' } else { '+' };
    let a = off_secs.unsigned_abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    if s != 0 { format!("UTC{sign}{h:02}:{m:02}:{s:02}") } else { format!("UTC{sign}{h:02}:{m:02}") }
}

/// A `PyCodec` for a charset name (public for `msg`).
pub fn codec(label: &str) -> Option<PyCodec> {
    pystr::lookup_codec(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_header_cases() {
        assert_eq!(decode_header_joined("=?utf-8?B?SWhyZSBCZXN0ZWxsdW5nIMOEIDEyMw==?="), "Ihre Bestellung Ä 123");
        assert_eq!(decode_header_joined("=?iso-8859-1?Q?M=FCller?= Hans"), "Müller Hans");
        assert_eq!(decode_header_first("=?iso-8859-1?Q?M=FCller?= Hans"), "Müller");
        assert_eq!(decode_header_joined("=?utf-8?q?a?= =?utf-8?q?b?="), "ab");
        assert_eq!(decode_header_joined("plain\r\n text"), "plain\r\n text");
    }

    #[test]
    fn params_rfc2231() {
        let v = get_param("attachment; filename*=utf-8''gr%C3%B6sse%20liste.bin", "filename").unwrap();
        assert_eq!(collapse_rfc2231(&v), "grösse liste.bin");
        let v = get_param("text/plain; name=\"a;b.txt\"; charset=UTF-8", "name").unwrap();
        assert_eq!(collapse_rfc2231(&v), "a;b.txt");
        assert_eq!(content_charset("text/plain; charset=\"UTF-8\"").as_deref(), Some("utf-8"));
        let v = get_param("x; filename*0=\"a\"; filename*1=\"b.txt\"", "filename").unwrap();
        assert_eq!(collapse_rfc2231(&v), "ab.txt");
    }

    #[test]
    fn addresses() {
        let a = parse_email_addresses("Bob <bob@example.com>, carol@example.com");
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].full(), "Bob <bob@example.com>");
        assert_eq!(a[1].full(), "carol@example.com");
        let a = parse_email_addresses("\"Shop, Online\" <shop@example.org>");
        assert_eq!(a[0].full(), "Shop, Online <shop@example.org>");
        assert!(parse_email_addresses("").is_empty());
        // Trailing comma -> strict: invalid -> no addresses
        assert!(parse_email_addresses("a@example.com,").is_empty());
        let a = parse_email_addresses("x@y.z (Kommentar)");
        assert_eq!(a[0].name, "Kommentar");
        let a = parse_email_addresses("nur name");
        assert_eq!(a[0].full(), "nur");
    }

    #[test]
    fn dates() {
        assert_eq!(
            parse_email_date("Wed, 8 Jan 2025 10:00:00 +0100 (CET)"),
            MailDate::Fixed(FixedOffset::east_opt(3600).unwrap().with_ymd_and_hms(2025, 1, 8, 10, 0, 0).unwrap())
        );
        assert!(matches!(parse_email_date("Thu, 9 Jan 2025 07:05:00 -0000"), MailDate::Naive(_)));
        assert!(matches!(parse_email_date("1 Jul 2024 14:00 GMT"), MailDate::Fixed(d) if d.offset().local_minus_utc() == 0));
        assert_eq!(parse_email_date(""), MailDate::Naive(NaiveDate::from_ymd_opt(1900, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap()));
        assert_eq!(fixed_tzname(3600), "UTC+01:00");
        assert_eq!(fixed_tzname(-16200), "UTC-04:30");
    }
}
