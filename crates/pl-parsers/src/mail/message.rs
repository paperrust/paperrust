//! MIME tree like CPython 3.12 `email.feedparser` (compat32) and `email.message.Message`.
//!
//! upstream: CPython 3.12 `email/feedparser.py` (FeedParser._parsegen, _parse_headers),
//! `email/_policybase.py` (Compat32.header_source_parse), `email/message.py`
//! (get_content_type, get_filename, get_payload(decode=True), walk).

use super::header::{self, HeaderValue};

#[derive(Debug, Clone)]
pub enum Payload {
    /// Non-multipart: raw bytes of the body.
    Bytes(Vec<u8>),
    /// multipart/* with boundary or message/*: sub-parts.
    Parts(Vec<Message>),
}

#[derive(Debug, Clone)]
pub struct Message {
    /// (name as written, raw value)
    pub headers: Vec<(String, HeaderValue)>,
    pub payload: Payload,
    pub default_type: &'static str,
    /// Raw bytes of the whole message (for the `as_bytes()` size of attached mails).
    pub raw: Vec<u8>,
    pub epilogue: Option<Vec<u8>>,
}

/// Lines incl. line ending (`\r\n`, `\r`, `\n`) like `NLCRE_crack`.
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

/// headerRE = `^(From |[\041-\071\073-\176]*:|[\t ])`
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

fn strip_eol(l: &[u8]) -> &[u8] {
    if let Some(x) = l.strip_suffix(b"\r\n") {
        x
    } else if let Some(x) = l.strip_suffix(b"\n") {
        x
    } else if let Some(x) = l.strip_suffix(b"\r") {
        x
    } else {
        l
    }
}

fn is_blank_line(l: &[u8]) -> bool {
    l.starts_with(b"\r\n") || l.starts_with(b"\n") || l.starts_with(b"\r")
}

struct Input<'a> {
    lines: Vec<&'a [u8]>,
    pos: usize,
    /// active boundaries (`--boundary`) – a matching line ends reading
    eof: Vec<Vec<u8>>,
}

/// boundaryendRE after the separator: `(--)?[ \t]*(\r\n|\r|\n)?$`
fn boundary_match(line: &[u8], sep: &[u8]) -> Option<bool> {
    let rest = line.strip_prefix(sep)?;
    let (end, rest) = match rest.strip_prefix(b"--") {
        Some(r) => (true, r),
        None => (false, rest),
    };
    let mut i = 0;
    while i < rest.len() && (rest[i] == b' ' || rest[i] == b'\t') {
        i += 1;
    }
    let tail = &rest[i..];
    let ok = tail.is_empty() || tail == b"\r\n" || tail == b"\n" || tail == b"\r";
    ok.then_some(end)
}

impl<'a> Input<'a> {
    fn readline(&mut self) -> Option<&'a [u8]> {
        let l = *self.lines.get(self.pos)?;
        for sep in self.eof.iter().rev() {
            if boundary_match(l, sep).is_some() {
                return None;
            }
        }
        self.pos += 1;
        Some(l)
    }
    fn unread(&mut self) {
        self.pos -= 1;
    }
    fn rest(&mut self) -> Vec<&'a [u8]> {
        let mut v = Vec::new();
        while let Some(l) = self.readline() {
            v.push(l);
        }
        v
    }
}

impl Message {
    pub fn parse(data: &[u8], max_depth: usize) -> Message {
        let lines = split_lines(data);
        let mut input = Input {
            lines,
            pos: 0,
            eof: Vec::new(),
        };
        parsegen(&mut input, "text/plain", 0, max_depth)
    }

    pub fn get(&self, name: &str) -> Option<&HeaderValue> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    pub fn get_all(&self, name: &str) -> Vec<&HeaderValue> {
        self.headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
            .collect()
    }

    pub fn get_str(&self, name: &str) -> Option<String> {
        self.get(name).map(|v| v.py_str())
    }

    pub fn get_content_type(&self) -> String {
        match self.get_str("content-type") {
            None => self.default_type.to_owned(),
            Some(v) => {
                let ct = header::splitparam(&v).0.to_lowercase();
                if ct.matches('/').count() != 1 {
                    "text/plain".to_owned()
                } else {
                    ct
                }
            }
        }
    }

    pub fn get_content_maintype(&self) -> String {
        self.get_content_type()
            .split('/')
            .next()
            .unwrap_or("")
            .to_owned()
    }

    pub fn get_content_charset(&self) -> Option<String> {
        header::content_charset(&self.get_str("content-type")?)
    }

    pub fn get_content_disposition(&self) -> Option<String> {
        self.get_str("content-disposition")
            .map(|v| header::splitparam(&v).0.to_lowercase())
    }

    /// `get_filename()` (`None` = missing; `Some("")` possible).
    pub fn get_filename(&self) -> Option<String> {
        let p = self
            .get_str("content-disposition")
            .and_then(|v| header::get_param(&v, "filename"))
            .or_else(|| {
                self.get_str("content-type")
                    .and_then(|v| header::get_param(&v, "name"))
            })?;
        Some(crate::pystr::py_strip(&header::collapse_rfc2231(&p)).to_owned())
    }

    pub fn get_boundary(&self) -> Option<String> {
        let v = header::get_param(&self.get_str("content-type")?, "boundary")?;
        Some(header::collapse_rfc2231(&v).trim_end().to_owned())
    }

    pub fn is_multipart(&self) -> bool {
        matches!(self.payload, Payload::Parts(_))
    }

    /// `walk()`: pre-order, incl. embedded messages.
    pub fn walk(&self) -> Vec<&Message> {
        let mut out = vec![self];
        if let Payload::Parts(ps) = &self.payload {
            for p in ps {
                out.extend(p.walk());
            }
        }
        out
    }

    /// `get_payload(decode=True)`; `None` for multipart.
    pub fn get_payload_decoded(&self) -> Option<Vec<u8>> {
        let b = match &self.payload {
            Payload::Bytes(b) => b,
            Payload::Parts(_) => return None,
        };
        let cte = self
            .get_str("content-transfer-encoding")
            .unwrap_or_default()
            .to_lowercase();
        Some(match cte.as_str() {
            "quoted-printable" => qp_decode(b),
            "base64" => {
                let joined: Vec<u8> = b
                    .iter()
                    .copied()
                    .filter(|&c| c != b'\r' && c != b'\n')
                    .collect();
                header::b64_decode_lenient(&joined)
            }
            "x-uuencode" | "uuencode" | "uue" | "x-uue" => uu_decode(b),
            _ => b.clone(),
        })
    }
}

fn parsegen<'a>(
    input: &mut Input<'a>,
    default_type: &'static str,
    depth: usize,
    max_depth: usize,
) -> Message {
    let start_pos = input.pos;
    // Collect header lines
    let mut hlines: Vec<&[u8]> = Vec::new();
    while let Some(line) = input.readline() {
        if !is_header_line(line) {
            if !is_blank_line(line) {
                input.unread();
            }
            break;
        }
        hlines.push(line);
    }
    let mut headers: Vec<(String, HeaderValue)> = Vec::new();
    let mut last: Vec<&[u8]> = Vec::new();
    let flush = |last: &mut Vec<&[u8]>, headers: &mut Vec<(String, HeaderValue)>| {
        if last.is_empty() {
            return;
        }
        let first = last[0];
        let colon = first.iter().position(|&c| c == b':').unwrap_or(first.len());
        let name = String::from_utf8_lossy(&first[..colon]).into_owned();
        let mut value: Vec<u8> = first.get(colon + 1..).unwrap_or(&[]).to_vec();
        let lead = value
            .iter()
            .take_while(|&&c| c == b' ' || c == b'\t')
            .count();
        value.drain(..lead);
        for l in &last[1..] {
            value.extend_from_slice(l);
        }
        while value.last().is_some_and(|&c| c == b'\r' || c == b'\n') {
            value.pop();
        }
        headers.push((name, HeaderValue(value)));
        last.clear();
    };
    let n = hlines.len();
    for (lineno, line) in hlines.iter().enumerate() {
        if line.first().is_some_and(|&c| c == b' ' || c == b'\t') {
            if last.is_empty() {
                continue;
            }
            last.push(line);
            continue;
        }
        flush(&mut last, &mut headers);
        if line.starts_with(b"From ") {
            if lineno == 0 {
                continue;
            } else if lineno == n - 1 {
                input.unread();
                break;
            } else {
                continue;
            }
        }
        let i = line.iter().position(|&c| c == b':').unwrap_or(0);
        if i == 0 {
            continue;
        }
        last.push(line);
    }
    flush(&mut last, &mut headers);
    let mut msg = Message {
        headers,
        payload: Payload::Bytes(Vec::new()),
        default_type,
        raw: Vec::new(),
        epilogue: None,
    };
    let ct = msg.get_content_type();
    let maintype = msg.get_content_maintype();
    if maintype == "message" && ct != "message/delivery-status" && depth < max_depth {
        let inner = parsegen(input, "text/plain", depth + 1, max_depth);
        msg.payload = Payload::Parts(vec![inner]);
    } else if maintype == "multipart" && depth < max_depth {
        match msg.get_boundary() {
            None => {
                msg.payload = Payload::Bytes(input.rest().concat());
            }
            Some(boundary) => {
                let sep = format!("--{boundary}").into_bytes();
                let child_default = if ct == "multipart/digest" {
                    "message/rfc822"
                } else {
                    "text/plain"
                };
                let mut capturing_preamble = true;
                let mut preamble: Vec<&[u8]> = Vec::new();
                let mut parts: Vec<Message> = Vec::new();
                let mut close_seen = false;
                while let Some(line) = input.readline() {
                    if let Some(is_end) = boundary_match(line, &sep) {
                        if is_end {
                            close_seen = true;
                            break;
                        }
                        if capturing_preamble {
                            capturing_preamble = false;
                            input.unread();
                            // Read the boundary line again and skip it
                            let _ = input.readline();
                        }
                        // consume further directly following boundary lines
                        loop {
                            match input.lines.get(input.pos) {
                                Some(l) if boundary_match(l, &sep) == Some(false) => input.pos += 1,
                                _ => break,
                            }
                        }
                        input.eof.push(sep.clone());
                        let mut part = parsegen(input, child_default, depth + 1, max_depth);
                        input.eof.pop();
                        // The line ending before the boundary belongs to the boundary
                        strip_last_eol(&mut part);
                        parts.push(part);
                        // after the part comes the next boundary (or EOF)
                        continue;
                    }
                    if capturing_preamble {
                        preamble.push(line);
                    }
                }
                if capturing_preamble {
                    // no start boundary: preamble is the body
                    msg.payload = Payload::Bytes(preamble.concat());
                } else {
                    msg.payload = Payload::Parts(parts);
                    if close_seen {
                        let epi = input.rest().concat();
                        msg.epilogue = Some(epi);
                    }
                }
            }
        }
    } else {
        msg.payload = Payload::Bytes(input.rest().concat());
    }
    msg.raw = input.lines[start_pos..input.pos].concat();
    msg
}

fn strip_last_eol(part: &mut Message) {
    // feedparser cleans up `self._last`: for message/* it is the (innermost) embedded message
    if part.get_content_maintype() == "message"
        && let Payload::Parts(inner) = &mut part.payload
        && let Some(last) = inner.last_mut()
    {
        strip_last_eol(last);
        return;
    }
    if part.get_content_maintype() == "multipart" {
        if let Some(e) = &mut part.epilogue {
            if e.is_empty() {
                part.epilogue = None;
            } else {
                let l = strip_eol(e).len();
                e.truncate(l);
            }
        }
        return;
    }
    if let Payload::Bytes(b) = &mut part.payload {
        let l = strip_eol(b).len();
        b.truncate(l);
    }
}

/// `quopri.decodestring` (binascii.a2b_qp, header=False).
pub fn qp_decode(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(d.len());
    let mut i = 0;
    while i < d.len() {
        if d[i] == b'=' {
            i += 1;
            if i >= d.len() {
                break;
            }
            if d[i] == b'\n' || d[i] == b'\r' {
                if d[i] != b'\n' {
                    while i < d.len() && d[i] != b'\n' {
                        i += 1;
                    }
                }
                if i < d.len() {
                    i += 1;
                }
            } else if d[i] == b'=' {
                out.push(b'=');
                i += 1;
            } else if i + 1 < d.len() {
                match (header::hexval(d[i]), header::hexval(d[i + 1])) {
                    (Some(h), Some(l)) => {
                        out.push((h << 4) | l);
                        i += 2;
                    }
                    _ => out.push(b'='),
                }
            } else {
                out.push(b'=');
            }
        } else {
            out.push(d[i]);
            i += 1;
        }
    }
    out
}

fn uu_decode(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut started = false;
    for line in split_lines(d) {
        let l = strip_eol(line);
        if !started {
            if l.starts_with(b"begin ") {
                started = true;
            }
            continue;
        }
        if l == b"end" || l.is_empty() {
            break;
        }
        let n = ((l[0].wrapping_sub(32)) & 63) as usize;
        let mut bytes = Vec::new();
        for chunk in l[1..].chunks(4) {
            let v: Vec<u8> = chunk.iter().map(|c| c.wrapping_sub(32) & 63).collect();
            let g = |k: usize| v.get(k).copied().unwrap_or(0) as u32;
            let x = (g(0) << 18) | (g(1) << 12) | (g(2) << 6) | g(3);
            bytes.extend_from_slice(&[(x >> 16) as u8, (x >> 8) as u8, x as u8]);
        }
        bytes.truncate(n);
        out.extend(bytes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_basic() {
        let m = Message::parse(
            b"Content-Type: multipart/mixed; boundary=B\n\npre\n--B\nContent-Type: text/plain\n\nhallo\n--B\n\nzwei\n--B--\nepi\n",
            32,
        );
        let Payload::Parts(ps) = &m.payload else {
            panic!()
        };
        assert_eq!(ps.len(), 2);
        assert!(matches!(&ps[0].payload, Payload::Bytes(b) if b == b"hallo"));
        assert!(matches!(&ps[1].payload, Payload::Bytes(b) if b == b"zwei"));
        assert_eq!(m.walk().len(), 3);
    }

    #[test]
    fn folding_kept() {
        let m = Message::parse(b"Subject: a\r\n b\r\nX: 1\r\n\r\nbody", 32);
        assert_eq!(m.get("subject").unwrap().0, b"a\r\n b");
        assert!(matches!(&m.payload, Payload::Bytes(b) if b == b"body"));
    }

    #[test]
    fn qp() {
        assert_eq!(qp_decode(b"f=FCr =\nx=3d=zz"), b"f\xfcr x==zz");
        assert_eq!(qp_decode(b"a=FCb=\r\nc"), b"a\xfcbc");
        assert_eq!(qp_decode(b"a=zz"), b"a=zz");
    }
}
