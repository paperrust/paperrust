//! Old Office formats in the OLE2 container (compound file): Word 97–2003 (.doc), Excel 97–2003 (.xls),
//! PowerPoint 97–2003 (.ppt) – text and metadata without Tika.
//!
//! upstream: src/paperless/parsers/tika.py@v3.4.2 (Tika reads these formats with Apache POI:
//! WordExtractor, ExcelExtractor, PowerPointExtractor; metadata from `SummaryInformation`).
//!
//! Implementation per the open specifications:
//! * [MS-DOC] 2.5 FIB, 2.9.38 Clx/Pcdt/PlcPcd, 2.4.1 character positions (compressed = cp1252,
//!   otherwise UTF-16LE); Word 6/95 without piece table: text between `fcMin` and `fcMac`.
//! * [MS-XLS] 2.4 BIFF8 records (BoundSheet8, SST with Continue, LabelSst, Label, Number, RK,
//!   MulRk, Formula + String, BoolErr), BIFF5 (`Book`) with 8-bit texts.
//! * [MS-PPT] 2.3 CurrentUser → UserEditAtom → PersistDirectory (newest version of each object),
//!   DocumentContainer → SlideListWithText (slides and notes), text in TextCharsAtom/TextBytesAtom
//!   also inside the slide drawing (OfficeArt).
//! * [MS-OLEPS] properties `SummaryInformation`/`DocumentSummaryInformation`.
//!
//! All readers are lenient, bound loops and sizes, and never panic on broken files
//! (no indexing without checks).

use std::io::{Cursor, Read};

use crate::limits::TextSink;
use crate::{Deadline, Limits, ParseError, Result};

use super::{OfficeContent, Slide, push_line};

type Cfb<'a> = cfb::CompoundFile<Cursor<&'a [u8]>>;

pub const MIME_DOC: &str = "application/msword";
pub const MIME_XLS: &str = "application/vnd.ms-excel";
pub const MIME_PPT: &str = "application/vnd.ms-powerpoint";

/// Maximum number of records/pieces that are read (against infinite loops in broken files).
const MAX_RECORDS: usize = 5_000_000;

fn invalid(m: impl Into<String>) -> ParseError {
    ParseError::Invalid(m.into())
}

fn open(data: &[u8]) -> Result<Cfb<'_>> {
    cfb::CompoundFile::open(Cursor::new(data)).map_err(|e| invalid(format!("not an OLE2 compound file: {e}")))
}

/// Read a stream (at most `max` bytes); `None` if it does not exist.
fn stream(cf: &mut Cfb<'_>, name: &str, max: u64) -> Result<Option<Vec<u8>>> {
    if !cf.is_stream(name) {
        return Ok(None);
    }
    let mut s = cf.open_stream(name).map_err(|e| invalid(format!("{name}: {e}")))?;
    if s.len() > max {
        return Err(ParseError::LimitExceeded(format!("stream {name} too large")));
    }
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| invalid(format!("{name}: {e}")))?;
    Ok(Some(buf))
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

fn f64_at(b: &[u8], o: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(o..o.checked_add(8)?)?.try_into().ok()?))
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    String::from_utf16_lossy(&units)
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|c| char::from(*c)).collect()
}

fn cp1252(b: &[u8]) -> String {
    encoding_rs::WINDOWS_1252.decode_without_bom_handling(b).0.into_owned()
}

/// Number like Tika/POI without cell format: whole numbers without decimals, otherwise shortest form.
pub fn format_number(v: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    format!("{v}")
}

/// Common entry point: text and metadata.
pub fn extract(data: &[u8], mime_type: &str, limits: &Limits, deadline: Deadline) -> Result<OfficeContent> {
    let mut cf = open(data)?;
    let mut out = TextSink::new(limits.max_text_chars);
    match mime_type {
        MIME_DOC => doc_text(&mut cf, limits, &mut out)?,
        MIME_XLS => xls_text(&mut cf, limits, deadline, &mut out)?,
        _ => {
            for s in ppt_slides_cf(&mut cf, limits, deadline)? {
                for l in s.title.iter().chain(&s.lines).chain(&s.notes) {
                    push_line(&mut out, l)?;
                }
                out.push('\n')?;
            }
        }
    }
    let (metadata, created) = summary_metadata(&mut cf, mime_type, limits);
    Ok(OfficeContent { text: out.buf, metadata, created })
}

// ---------------------------------------------------------------------------------------------
// Word 97–2003
// ---------------------------------------------------------------------------------------------

/// Text of a Word document with control characters (raw text of all text parts in CP order).
fn doc_raw(cf: &mut Cfb<'_>, limits: &Limits) -> Result<String> {
    let wd = stream(cf, "WordDocument", limits.max_entry_size)?.ok_or_else(|| invalid("no WordDocument stream"))?;
    if wd.len() < 0x20 || u16_at(&wd, 0) != Some(0xA5EC) {
        return Err(invalid("not a Word document (bad FIB)"));
    }
    let nfib = u16_at(&wd, 2).unwrap_or(0);
    let flags = u16_at(&wd, 0x0A).unwrap_or(0);
    if flags & 0x0100 != 0 {
        return Err(ParseError::Encrypted);
    }
    if nfib < 0x00C1 {
        // Word 6/95: without a piece table (fComplex = 0) the text lies contiguously between fcMin and fcMac.
        if flags & 0x0004 != 0 {
            return Err(ParseError::Unsupported("fast-saved Word 6/95 document".into()));
        }
        let fc_min = u32_at(&wd, 0x18).unwrap_or(0) as usize;
        let fc_mac = u32_at(&wd, 0x1C).unwrap_or(0) as usize;
        let bytes = wd.get(fc_min..fc_mac.min(wd.len())).unwrap_or(&[]);
        return Ok(cp1252(bytes));
    }
    // FibBase (32) | csw | fibRgW | cslw | fibRgLw | cbRgFcLcb | fibRgFcLcb
    let csw = u16_at(&wd, 32).ok_or_else(|| invalid("truncated FIB"))? as usize;
    let lw = 32 + 2 + csw * 2;
    let cslw = u16_at(&wd, lw).ok_or_else(|| invalid("truncated FIB"))? as usize;
    let fc_lcb = lw + 2 + cslw * 4 + 2;
    let fc_clx = u32_at(&wd, fc_lcb + 33 * 8).ok_or_else(|| invalid("truncated FIB"))? as usize;
    let lcb_clx = u32_at(&wd, fc_lcb + 33 * 8 + 4).ok_or_else(|| invalid("truncated FIB"))? as usize;
    let table_name = if flags & 0x0200 != 0 { "1Table" } else { "0Table" };
    let table = stream(cf, table_name, limits.max_entry_size)?.ok_or_else(|| invalid("no table stream"))?;
    let clx = table.get(fc_clx..fc_clx.saturating_add(lcb_clx)).ok_or_else(|| invalid("Clx outside the table stream"))?;
    // skip Prc entries (0x01), then Pcdt (0x02)
    let mut i = 0usize;
    let plc = loop {
        match clx.get(i) {
            Some(1) => {
                let cb = u16_at(clx, i + 1).ok_or_else(|| invalid("truncated Prc"))? as usize;
                i += 3 + cb;
            }
            Some(2) => {
                let lcb = u32_at(clx, i + 1).ok_or_else(|| invalid("truncated Pcdt"))? as usize;
                break clx.get(i + 5..(i + 5).saturating_add(lcb)).ok_or_else(|| invalid("truncated PlcPcd"))?;
            }
            _ => return Err(invalid("no piece table")),
        }
    };
    if plc.len() < 4 {
        return Err(invalid("empty piece table"));
    }
    let n = (plc.len() - 4) / 12;
    let mut text = String::new();
    for k in 0..n.min(MAX_RECORDS) {
        let cp0 = u32_at(plc, k * 4).unwrap_or(0) as usize;
        let cp1 = u32_at(plc, (k + 1) * 4).unwrap_or(0) as usize;
        let pcd = (n + 1) * 4 + k * 8;
        let fc_raw = u32_at(plc, pcd + 2).unwrap_or(0);
        let chars = cp1.saturating_sub(cp0);
        if chars == 0 {
            continue;
        }
        if text.len().saturating_add(chars) > limits.max_text_chars {
            return Err(ParseError::LimitExceeded("text too long".into()));
        }
        if fc_raw & 0x4000_0000 != 0 {
            let off = ((fc_raw & 0x3FFF_FFFF) / 2) as usize;
            if let Some(b) = wd.get(off..off.saturating_add(chars)) {
                text.push_str(&cp1252(b));
            }
        } else {
            let off = fc_raw as usize;
            if let Some(b) = wd.get(off..off.saturating_add(chars * 2)) {
                text.push_str(&utf16(b));
            }
        }
    }
    Ok(text)
}

/// Resolve Word control characters: paragraph/line break → `\n`, cell → tab, field commands removed
/// (only the field result remains), object/footnote anchors removed.
pub fn clean_word_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    // Field stack: true = in the command part (between 0x13 and 0x14)
    let mut fields: Vec<bool> = Vec::new();
    for c in raw.chars() {
        match c {
            '\u{13}' => {
                fields.push(true);
                continue;
            }
            '\u{14}' => {
                if let Some(f) = fields.last_mut() {
                    *f = false;
                }
                continue;
            }
            '\u{15}' => {
                fields.pop();
                continue;
            }
            _ => {}
        }
        if fields.iter().any(|in_cmd| *in_cmd) {
            continue;
        }
        match c {
            '\r' | '\u{0b}' | '\u{0c}' | '\u{0e}' => out.push('\n'),
            '\u{07}' => out.push('\t'),
            '\u{1e}' => out.push('-'),
            '\u{1f}' | '\u{01}' | '\u{02}' | '\u{03}' | '\u{04}' | '\u{05}' | '\u{08}' => {}
            '\u{a0}' => out.push(' '),
            c if c.is_control() && c != '\t' && c != '\n' => {}
            c => out.push(c),
        }
    }
    // Smooth cell end before row end ("\t\n")
    out.replace("\t\n", "\n")
}

fn doc_text(cf: &mut Cfb<'_>, limits: &Limits, out: &mut TextSink) -> Result<()> {
    let raw = doc_raw(cf, limits)?;
    out.push_str(&clean_word_text(&raw))
}

// ---------------------------------------------------------------------------------------------
// Excel 97–2003
// ---------------------------------------------------------------------------------------------

/// BIFF records (type, data), CONTINUE records stay separate entries.
fn biff_records(wb: &[u8]) -> Vec<(u16, &[u8])> {
    let mut v = Vec::new();
    let mut i = 0usize;
    while i + 4 <= wb.len() && v.len() < MAX_RECORDS {
        let ty = u16_at(wb, i).unwrap_or(0);
        let len = u16_at(wb, i + 2).unwrap_or(0) as usize;
        let end = (i + 4 + len).min(wb.len());
        v.push((ty, &wb[i + 4..end]));
        i = i + 4 + len;
    }
    v
}

/// Reads characters of an XLUnicodeString sequence across CONTINUE boundaries ([MS-XLS] 2.5.293/2.5.294).
struct ContReader<'a> {
    parts: Vec<&'a [u8]>,
    part: usize,
    pos: usize,
}

impl<'a> ContReader<'a> {
    fn cur(&self) -> &'a [u8] {
        self.parts.get(self.part).copied().unwrap_or(&[])
    }
    fn next_part(&mut self) -> bool {
        if self.part + 1 < self.parts.len() {
            self.part += 1;
            self.pos = 0;
            true
        } else {
            false
        }
    }
    fn bytes(&mut self, mut n: usize) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(n.min(1 << 16));
        while n > 0 {
            let c = self.cur();
            if self.pos >= c.len() {
                if !self.next_part() {
                    return None;
                }
                continue;
            }
            let take = n.min(c.len() - self.pos);
            out.extend_from_slice(&c[self.pos..self.pos + take]);
            self.pos += take;
            n -= take;
        }
        Some(out)
    }
    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).and_then(|b| b.first().copied())
    }
    fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.bytes(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    /// `cch` characters; at a CONTINUE boundary there is a new option byte (fHighByte).
    fn chars(&mut self, cch: usize, mut high: bool) -> Option<String> {
        let mut s = String::new();
        let mut left = cch;
        while left > 0 {
            let c = self.cur();
            if self.pos >= c.len() {
                if !self.next_part() {
                    return None;
                }
                high = self.u8()? & 1 != 0;
                continue;
            }
            let width = if high { 2 } else { 1 };
            let avail = (c.len() - self.pos) / width;
            if avail == 0 {
                // half a character at the end of the record: broken file
                return None;
            }
            let take = left.min(avail);
            let b = &c[self.pos..self.pos + take * width];
            s.push_str(&if high { utf16(b) } else { latin1(b) });
            self.pos += take * width;
            left -= take;
        }
        Some(s)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.bytes(n).map(|_| ())
    }
}

/// Shared String Table (SST + CONTINUE).
fn read_sst(parts: Vec<&[u8]>, max_chars: usize) -> Vec<String> {
    let mut r = ContReader { parts, part: 0, pos: 0 };
    let Some(_total) = r.u32() else { return Vec::new() };
    let Some(unique) = r.u32() else { return Vec::new() };
    let mut out = Vec::with_capacity((unique as usize).min(1 << 20));
    let mut used = 0usize;
    for _ in 0..unique.min(MAX_RECORDS as u32) {
        let Some(cch) = r.u16() else { break };
        let Some(flags) = r.u8() else { break };
        let rich = flags & 0x08 != 0;
        let ext = flags & 0x04 != 0;
        let runs = if rich { r.u16().unwrap_or(0) as usize } else { 0 };
        let ext_len = if ext { r.u32().unwrap_or(0) as usize } else { 0 };
        let Some(s) = r.chars(cch as usize, flags & 1 != 0) else { break };
        if r.skip(runs * 4).is_none() || r.skip(ext_len).is_none() {
            out.push(s);
            break;
        }
        used = used.saturating_add(s.len());
        if used > max_chars {
            break;
        }
        out.push(s);
    }
    out
}

/// RK number ([MS-XLS] 2.5.217).
fn rk(v: u32) -> f64 {
    let mut n = if v & 0x02 != 0 { f64::from((v as i32) >> 2) } else { f64::from_bits(u64::from(v & 0xFFFF_FFFC) << 32) };
    if v & 0x01 != 0 {
        n /= 100.0;
    }
    n
}

/// BIFF8 XLUnicodeString (cch u16, option byte, characters) or BIFF5 (cch u16, 8-bit characters).
fn xl_string(b: &[u8], biff8: bool) -> String {
    let Some(cch) = u16_at(b, 0) else { return String::new() };
    let cch = cch as usize;
    if !biff8 {
        return cp1252(b.get(2..(2 + cch).min(b.len())).unwrap_or(&[]));
    }
    let high = b.get(2).is_some_and(|f| f & 1 != 0);
    if high { utf16(b.get(3..(3 + cch * 2).min(b.len())).unwrap_or(&[])) } else { latin1(b.get(3..(3 + cch).min(b.len())).unwrap_or(&[])) }
}

/// A worksheet: name and cells (row, column, text).
pub struct XlsSheet {
    pub name: String,
    pub cells: Vec<(u16, u16, String)>,
}

/// All sheets of an Excel 97-2003 workbook with cell values as text.
fn xls_sheets(cf: &mut Cfb<'_>, limits: &Limits, deadline: Deadline) -> Result<Vec<XlsSheet>> {
    let (wb, biff8) = match stream(cf, "Workbook", limits.max_entry_size)? {
        Some(w) => (w, true),
        None => (stream(cf, "Book", limits.max_entry_size)?.ok_or_else(|| invalid("no Workbook stream"))?, false),
    };
    let recs = biff_records(&wb);
    let mut sst: Vec<String> = Vec::new();
    let mut sheets: Vec<XlsSheet> = Vec::new();
    let mut in_globals = true;
    let mut depth = 0usize;
    let mut cur: Option<XlsSheet> = None;
    let mut names: Vec<String> = Vec::new();
    let mut pending_formula: Option<(u16, u16)> = None;
    let mut i = 0usize;
    let mut text_chars = 0usize;
    // Cell formats (FORMAT, XF, DATEMODE) for the displayed text like POI `DataFormatter`
    let mut formats: Vec<(u32, String)> = Vec::new();
    let mut xf_fmt: Vec<u32> = Vec::new();
    let mut date1904 = false;
    while i < recs.len() {
        if i.is_multiple_of(4096) {
            deadline.check()?;
        }
        let (ty, d) = recs[i];
        match ty {
            0x0809 | 0x0409 | 0x0209 | 0x0009 => {
                depth += 1;
                if !in_globals && depth == 1 {
                    let name = names.get(sheets.len()).cloned().unwrap_or_else(|| format!("Sheet{}", sheets.len() + 1));
                    cur = Some(XlsSheet { name, cells: Vec::new() });
                }
            }
            0x000A => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if in_globals {
                        in_globals = false;
                    } else if let Some(s) = cur.take() {
                        sheets.push(s);
                    }
                }
            }
            0x002F if in_globals => return Err(ParseError::Encrypted),
            0x0085 if in_globals => {
                // BoundSheet8: lbPlyPos u32, hsState u8, dt u8, ShortXLUnicodeString
                let cch = d.get(6).copied().unwrap_or(0) as usize;
                let name = if biff8 {
                    let high = d.get(7).is_some_and(|f| f & 1 != 0);
                    if high {
                        utf16(d.get(8..(8 + cch * 2).min(d.len())).unwrap_or(&[]))
                    } else {
                        latin1(d.get(8..(8 + cch).min(d.len())).unwrap_or(&[]))
                    }
                } else {
                    cp1252(d.get(7..(7 + cch).min(d.len())).unwrap_or(&[]))
                };
                names.push(name);
            }
            0x041E | 0x001E if in_globals => {
                // FORMAT: ifmt u16, then (BIFF8) XLUnicodeString or (BIFF5) cch u8 + characters
                if let Some(id) = u16_at(d, 0) {
                    let code = if biff8 {
                        xl_string(d.get(2..).unwrap_or(&[]), true)
                    } else {
                        let cch = d.get(2).copied().unwrap_or(0) as usize;
                        cp1252(d.get(3..(3 + cch).min(d.len())).unwrap_or(&[]))
                    };
                    formats.push((u32::from(id), code));
                }
            }
            0x00E0 if in_globals => xf_fmt.push(u32::from(u16_at(d, 2).unwrap_or(0))),
            0x0022 if in_globals => date1904 = u16_at(d, 0) == Some(1),
            0x00FC if in_globals => {
                let mut parts = vec![d];
                while let Some((0x003C, c)) = recs.get(i + 1) {
                    parts.push(c);
                    i += 1;
                }
                sst = read_sst(parts, limits.max_text_chars);
            }
            _ => {}
        }
        if let Some(s) = cur.as_mut().filter(|_| depth == 1) {
            let rc = |d: &[u8]| (u16_at(d, 0).unwrap_or(0), u16_at(d, 2).unwrap_or(0));
            // Number → text according to the format of XF index `ixfe`
            let shown = |v: f64, ixfe: Option<u16>| {
                let id = ixfe.and_then(|x| xf_fmt.get(usize::from(x))).copied().unwrap_or(0);
                super::numfmt::format_cell(v, id, &formats, date1904)
            };
            let mut push = |r: u16, c: u16, t: String| {
                if !t.is_empty() {
                    text_chars = text_chars.saturating_add(t.len());
                    s.cells.push((r, c, t));
                }
            };
            match ty {
                0x00FD => {
                    let (r, c) = rc(d);
                    let idx = u32_at(d, 6).unwrap_or(u32::MAX) as usize;
                    push(r, c, sst.get(idx).cloned().unwrap_or_default());
                }
                0x0204 | 0x00D6 => {
                    let (r, c) = rc(d);
                    push(r, c, xl_string(d.get(6..).unwrap_or(&[]), biff8));
                }
                0x0203 => {
                    let (r, c) = rc(d);
                    if let Some(v) = f64_at(d, 6) {
                        push(r, c, shown(v, u16_at(d, 4)));
                    }
                }
                0x027E => {
                    let (r, c) = rc(d);
                    if let Some(v) = u32_at(d, 6) {
                        push(r, c, shown(rk(v), u16_at(d, 4)));
                    }
                }
                0x00BD => {
                    let (r, first) = rc(d);
                    let n = d.len().saturating_sub(6) / 6;
                    for k in 0..n {
                        if let Some(v) = u32_at(d, 4 + k * 6 + 2) {
                            push(r, first.saturating_add(k as u16), shown(rk(v), u16_at(d, 4 + k * 6)));
                        }
                    }
                }
                0x0006 => {
                    let (r, c) = rc(d);
                    let res = d.get(6..14).unwrap_or(&[]);
                    if res.len() == 8 && res[6] == 0xFF && res[7] == 0xFF {
                        match res[0] {
                            0 => pending_formula = Some((r, c)),
                            1 => push(r, c, if res[2] != 0 { "TRUE".into() } else { "FALSE".into() }),
                            _ => {}
                        }
                    } else if let Some(v) = f64_at(d, 6) {
                        push(r, c, shown(v, u16_at(d, 4)));
                    }
                }
                0x0207 => {
                    if let Some((r, c)) = pending_formula.take() {
                        push(r, c, xl_string(d, biff8));
                    }
                }
                0x0205 => {
                    let (r, c) = rc(d);
                    if d.get(7) == Some(&0) {
                        push(r, c, if d.get(6).is_some_and(|v| *v != 0) { "TRUE".into() } else { "FALSE".into() });
                    }
                }
                _ => {}
            }
            if text_chars > limits.max_text_chars {
                return Err(ParseError::LimitExceeded("text too long".into()));
            }
        }
        i += 1;
    }
    if let Some(s) = cur.take() {
        sheets.push(s);
    }
    Ok(sheets)
}

/// Arrange cells into rows (Tika form: sheet name, then per row a tab + cells separated by tab).
fn xls_text(cf: &mut Cfb<'_>, limits: &Limits, deadline: Deadline, out: &mut TextSink) -> Result<()> {
    for mut sheet in xls_sheets(cf, limits, deadline)? {
        push_line(out, &sheet.name)?;
        sheet.cells.sort_by_key(|(r, c, _)| (*r, *c));
        let mut i = 0;
        while i < sheet.cells.len() {
            let row = sheet.cells[i].0;
            let mut vals = Vec::new();
            while i < sheet.cells.len() && sheet.cells[i].0 == row {
                vals.push(sheet.cells[i].2.as_str());
                i += 1;
            }
            out.push('\t')?;
            push_line(out, &vals.join("\t"))?;
        }
        out.push('\n')?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// PowerPoint 97–2003
// ---------------------------------------------------------------------------------------------

const RT_DOCUMENT: u16 = 0x03E8;
const RT_SLIDE: u16 = 0x03EE;
const RT_SLIDE_ATOM: u16 = 0x03EF;
const RT_NOTES: u16 = 0x03F0;
const RT_SLWT: u16 = 0x0FF0;
const RT_SLIDE_PERSIST: u16 = 0x03F3;
const RT_TEXT_HEADER: u16 = 0x0F9F;
const RT_TEXT_CHARS: u16 = 0x0FA0;
const RT_TEXT_BYTES: u16 = 0x0FA8;
const RT_CSTRING: u16 = 0x0FBA;
const RT_USER_EDIT: u16 = 0x0FF5;
const RT_PERSIST_DIR: u16 = 0x1772;
const RT_MAIN_MASTER: u16 = 0x03F8;

/// Record header: (version, instance, type, length).
fn rh(b: &[u8], o: usize) -> Option<(u16, u16, u16, usize)> {
    let vi = u16_at(b, o)?;
    Some((vi & 0x0F, vi >> 4, u16_at(b, o + 2)?, u32_at(b, o + 4)? as usize))
}

/// Children of a container: (instance, type, start of the record, data range).
fn children(b: &[u8], start: usize, end: usize) -> Vec<(u16, u16, usize, usize, usize)> {
    let mut v = Vec::new();
    let mut o = start;
    let end = end.min(b.len());
    while o + 8 <= end && v.len() < MAX_RECORDS {
        let Some((ver, inst, ty, len)) = rh(b, o) else { break };
        let ds = o + 8;
        let de = ds.saturating_add(len).min(end);
        v.push((ver, inst, ty, o, de));
        let _ = inst;
        o = ds.saturating_add(len);
    }
    v.into_iter().map(|(ver, inst, ty, o, de)| (inst | (ver << 12), ty, o, o + 8, de)).collect()
}

fn is_container(tag: u16) -> bool {
    tag >> 12 == 0x0F
}

fn text_atom(ty: u16, d: &[u8]) -> Option<String> {
    let s = match ty {
        RT_TEXT_CHARS => utf16(d),
        RT_TEXT_BYTES => latin1(d),
        _ => return None,
    };
    let s: String = s
        .chars()
        .map(|c| if c == '\r' || c == '\u{0b}' { '\n' } else { c })
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Text (with kind from the preceding TextHeaderAtom: 0/6 = title) of a container, recursive.
fn collect_texts(b: &[u8], start: usize, end: usize, depth: usize, out: &mut Vec<(u32, String)>, budget: &mut usize) {
    if depth > 64 {
        return;
    }
    let mut kind = 1u32;
    for (tag, ty, _o, ds, de) in children(b, start, end) {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        if is_container(tag) {
            if ty != RT_MAIN_MASTER {
                collect_texts(b, ds, de, depth + 1, out, budget);
            }
            continue;
        }
        let d = b.get(ds..de).unwrap_or(&[]);
        match ty {
            RT_TEXT_HEADER => kind = u32_at(d, 0).unwrap_or(1),
            RT_TEXT_CHARS | RT_TEXT_BYTES => {
                if let Some(t) = text_atom(ty, d) {
                    out.push((kind, t));
                }
            }
            _ => {}
        }
    }
}

/// Persist directory (newest version first wins) and `docPersistIdRef`.
fn ppt_persist(cf: &mut Cfb<'_>, pd: &[u8], limits: &Limits) -> Option<(std::collections::HashMap<u32, usize>, u32)> {
    let cu = stream(cf, "Current User", limits.max_entry_size).ok().flatten()?;
    let mut edit = u32_at(&cu, 16)? as usize;
    let mut map = std::collections::HashMap::new();
    let mut doc_ref = None;
    let mut seen = 0usize;
    while seen < 4096 {
        seen += 1;
        let (_, _, ty, _) = rh(pd, edit)?;
        if ty != RT_USER_EDIT {
            return None;
        }
        let d = edit + 8;
        let last = u32_at(pd, d + 8)? as usize;
        let dir = u32_at(pd, d + 12)? as usize;
        if doc_ref.is_none() {
            doc_ref = u32_at(pd, d + 16);
        }
        let (_, _, dty, dlen) = rh(pd, dir)?;
        if dty != RT_PERSIST_DIR {
            return None;
        }
        let mut o = dir + 8;
        let end = (dir + 8).saturating_add(dlen).min(pd.len());
        while o + 4 <= end {
            let e = u32_at(pd, o)?;
            let id = e & 0x000F_FFFF;
            let n = (e >> 20) as usize;
            o += 4;
            for k in 0..n {
                let off = u32_at(pd, o + k * 4)? as usize;
                map.entry(id + k as u32).or_insert(off);
            }
            o += n * 4;
        }
        if last == 0 || last == edit {
            break;
        }
        edit = last;
    }
    Some((map, doc_ref?))
}

/// Slides of a PowerPoint 97-2003 file.
pub fn ppt_slides(data: &[u8], limits: &Limits, deadline: Deadline) -> Result<Vec<Slide>> {
    let mut cf = open(data)?;
    ppt_slides_cf(&mut cf, limits, deadline)
}

fn to_slide(texts: Vec<(u32, String)>, notes: Vec<(u32, String)>) -> Slide {
    let mut title = None;
    let mut lines = Vec::new();
    for (kind, t) in texts {
        if title.is_none() && (kind == 0 || kind == 6) {
            title = Some(t);
        } else {
            lines.extend(t.split('\n').map(str::to_owned).filter(|l| !l.trim().is_empty()));
        }
    }
    let notes = notes
        .into_iter()
        .flat_map(|(_, t)| t.split('\n').map(str::to_owned).collect::<Vec<_>>())
        .filter(|l| !l.trim().is_empty())
        .collect();
    Slide { title, lines, notes }
}

fn ppt_slides_cf(cf: &mut Cfb<'_>, limits: &Limits, deadline: Deadline) -> Result<Vec<Slide>> {
    let pd = stream(cf, "PowerPoint Document", limits.max_entry_size)?.ok_or_else(|| invalid("no PowerPoint Document stream"))?;
    if cf.is_stream("EncryptedSummary") {
        return Err(ParseError::Encrypted);
    }
    let mut budget = MAX_RECORDS;
    let record_at = |off: usize| -> Option<(u16, usize, usize)> {
        let (_, _, ty, len) = rh(&pd, off)?;
        Some((ty, off + 8, (off + 8).saturating_add(len).min(pd.len())))
    };
    if let Some((persist, doc_ref)) = ppt_persist(cf, &pd, limits)
        && let Some((RT_DOCUMENT, ds, de)) = persist.get(&doc_ref).and_then(|o| record_at(*o))
    {
        // Slide order from SlideListWithText (instance 0), notes from instance 2
        let mut slides: Vec<(u32, Vec<(u32, String)>)> = Vec::new();
        let mut notes: std::collections::HashMap<u32, Vec<(u32, String)>> = std::collections::HashMap::new();
        for (tag, ty, _o, cs, ce) in children(&pd, ds, de) {
            if ty != RT_SLWT || !is_container(tag) {
                continue;
            }
            let inst = tag & 0x0FFF;
            let mut kind = 1u32;
            let mut cur_notes: Option<u32> = None;
            for (_t2, ty2, _o2, d2s, d2e) in children(&pd, cs, ce) {
                let d = pd.get(d2s..d2e).unwrap_or(&[]);
                match ty2 {
                    RT_SLIDE_PERSIST => {
                        let id = u32_at(d, 0).unwrap_or(0);
                        if inst == 0 {
                            slides.push((id, Vec::new()));
                        } else if inst == 2 {
                            cur_notes = Some(id);
                        }
                    }
                    RT_TEXT_HEADER => kind = u32_at(d, 0).unwrap_or(1),
                    RT_TEXT_CHARS | RT_TEXT_BYTES => {
                        if let Some(t) = text_atom(ty2, d) {
                            if inst == 0 {
                                if let Some(s) = slides.last_mut() {
                                    s.1.push((kind, t));
                                }
                            } else if inst == 2
                                && let Some(n) = cur_notes
                            {
                                notes.entry(n).or_default().push((kind, t));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        for (id, mut texts) in slides {
            deadline.check()?;
            let mut note_texts = Vec::new();
            if let Some((RT_SLIDE, ss, se)) = persist.get(&id).and_then(|o| record_at(*o)) {
                let mut drawn = Vec::new();
                collect_texts(&pd, ss, se, 0, &mut drawn, &mut budget);
                texts.extend(drawn);
                // SlideAtom.notesIdRef → notes page
                for (_t, ty, _o, ds2, de2) in children(&pd, ss, se) {
                    if ty == RT_SLIDE_ATOM {
                        let d = pd.get(ds2..de2).unwrap_or(&[]);
                        let notes_ref = u32_at(d, 16).unwrap_or(0);
                        if notes_ref != 0 {
                            if let Some(n) = notes.remove(&notes_ref) {
                                note_texts.extend(n);
                            }
                            if let Some((RT_NOTES, ns, ne)) = persist.get(&notes_ref).and_then(|o| record_at(*o)) {
                                collect_texts(&pd, ns, ne, 0, &mut note_texts, &mut budget);
                            }
                        }
                    }
                }
            }
            out.push(to_slide(texts, note_texts));
        }
        return Ok(out);
    }
    // Fallback (no valid persist directory): all slide containers in file order.
    let mut out = Vec::new();
    for (tag, ty, _o, ds, de) in children(&pd, 0, pd.len()) {
        if ty == RT_SLIDE && is_container(tag) {
            let mut t = Vec::new();
            collect_texts(&pd, ds, de, 0, &mut t, &mut budget);
            out.push(to_slide(t, Vec::new()));
        }
    }
    let _ = RT_CSTRING;
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Metadata: SummaryInformation / DocumentSummaryInformation
// ---------------------------------------------------------------------------------------------

/// Value of a property.
#[derive(Debug, Clone, PartialEq)]
enum PropVal {
    Text(String),
    Int(i64),
    Time(String),
}

fn decode_codepage(b: &[u8], cp: u16) -> String {
    let enc = match cp {
        1200 => return utf16(b),
        65001 => return String::from_utf8_lossy(b).into_owned(),
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        874 => encoding_rs::WINDOWS_874,
        932 => encoding_rs::SHIFT_JIS,
        936 => encoding_rs::GBK,
        949 => encoding_rs::EUC_KR,
        950 => encoding_rs::BIG5,
        10000 => encoding_rs::MACINTOSH,
        _ => encoding_rs::WINDOWS_1252,
    };
    enc.decode_without_bom_handling(b).0.into_owned()
}

/// FILETIME (100 ns since 1601) → ISO 8601 UTC like Tika (`2024-01-02T03:04:05Z`).
fn filetime(v: u64) -> Option<String> {
    if v == 0 {
        return None;
    }
    let secs = (v / 10_000_000) as i64 - 11_644_473_600;
    let dt = chrono::DateTime::from_timestamp(secs, 0)?;
    Some(dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// First section of a property stream: (PID, value).
fn property_set(b: &[u8]) -> Vec<(u32, PropVal)> {
    let mut out = Vec::new();
    if u16_at(b, 0) != Some(0xFFFE) {
        return out;
    }
    let Some(sec) = u32_at(b, 44).map(|o| o as usize) else { return out };
    let n = u32_at(b, sec + 4).unwrap_or(0).min(4096) as usize;
    let mut entries = Vec::new();
    for k in 0..n {
        let (Some(pid), Some(off)) = (u32_at(b, sec + 8 + k * 8), u32_at(b, sec + 12 + k * 8)) else { break };
        entries.push((pid, sec + off as usize));
    }
    let codepage = entries.iter().find(|(p, _)| *p == 1).and_then(|(_, o)| u16_at(b, o + 4)).unwrap_or(1252);
    for (pid, o) in entries {
        let Some(vt) = u16_at(b, o) else { continue };
        let v = o + 4;
        let val = match vt {
            2 => u16_at(b, v).map(|x| PropVal::Int(i64::from(x as i16))),
            3 => u32_at(b, v).map(|x| PropVal::Int(i64::from(x as i32))),
            30 => u32_at(b, v).and_then(|len| {
                let len = len as usize;
                let raw = b.get(v + 4..(v + 4).saturating_add(len))?;
                let s = decode_codepage(raw, codepage);
                Some(PropVal::Text(s.trim_end_matches('\0').to_string()))
            }),
            31 => u32_at(b, v).and_then(|len| {
                let raw = b.get(v + 4..(v + 4).saturating_add(len as usize * 2))?;
                Some(PropVal::Text(utf16(raw).trim_end_matches('\0').to_string()))
            }),
            64 => {
                u32_at(b, v).zip(u32_at(b, v + 4)).and_then(|(lo, hi)| filetime(u64::from(lo) | (u64::from(hi) << 32))).map(PropVal::Time)
            }
            _ => None,
        };
        if let Some(val) = val {
            out.push((pid, val));
        }
    }
    out
}

/// Tika metadata from the OLE properties (keys like in the OOXML reader) and `dcterms:created`.
fn summary_metadata(cf: &mut Cfb<'_>, mime_type: &str, limits: &Limits) -> (Vec<(String, String)>, Option<String>) {
    let si = stream(cf, "\u{5}SummaryInformation", limits.max_entry_size.min(16 << 20))
        .ok()
        .flatten()
        .map(|b| property_set(&b))
        .unwrap_or_default();
    let dsi = stream(cf, "\u{5}DocumentSummaryInformation", limits.max_entry_size.min(16 << 20))
        .ok()
        .flatten()
        .map(|b| property_set(&b))
        .unwrap_or_default();
    let text = |v: &PropVal| match v {
        PropVal::Text(s) | PropVal::Time(s) => s.clone(),
        PropVal::Int(i) => i.to_string(),
    };
    let get = |set: &[(u32, PropVal)], pid: u32| set.iter().find(|(p, _)| *p == pid).map(|(_, v)| text(v)).filter(|s| !s.is_empty());
    let mut md = Vec::new();
    let mut created = None;
    let order: &[(bool, u32, &str)] = &[
        (true, 2, "dc:title"),
        (true, 3, "dc:subject"),
        (true, 4, "dc:creator"),
        (true, 5, "meta:keyword"),
        (true, 6, "dc:description"),
        (true, 8, "meta:last-author"),
        (true, 9, "cp:revision"),
        (false, 2, "cp:category"),
        (true, 11, "meta:print-date"),
        (true, 12, "dcterms:created"),
        (true, 13, "dcterms:modified"),
        (true, 18, "extended-properties:Application"),
        (false, 15, "extended-properties:Company"),
        (false, 14, "extended-properties:Manager"),
        (true, 7, "extended-properties:Template"),
        (true, 14, "meta:page-count"),
        (false, 7, "meta:slide-count"),
        (true, 15, "meta:word-count"),
        (true, 16, "meta:character-count"),
    ];
    for (summary, pid, key) in order {
        let v = if *summary { get(&si, *pid) } else { get(&dsi, *pid) };
        let Some(v) = v else { continue };
        if *key == "meta:slide-count" && mime_type != MIME_PPT {
            continue;
        }
        if *key == "dcterms:created" {
            created = Some(v.clone());
        }
        md.push(((*key).to_owned(), v.clone()));
        if *key == "meta:page-count" {
            md.push(("xmpTPg:NPages".into(), v));
        }
    }
    md.push(("Content-Type".into(), mime_type.to_owned()));
    (md, created)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Small OLE2 container with the given streams.
    pub fn compound(streams: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        for (name, data) in streams {
            let mut s = cf.create_stream(name).unwrap();
            s.write_all(data).unwrap();
        }
        cf.flush().unwrap();
        cf.into_inner().into_inner()
    }

    /// Minimal Word 97 document: FIB (csw=14, cslw=22, 93 fc/lcb pairs), text as UTF-16 in
    /// one piece, Clx in the table stream `1Table`.
    pub fn word97(text: &str) -> Vec<u8> {
        let units: Vec<u16> = text.encode_utf16().collect();
        let fib_len = 32 + 2 + 28 + 2 + 88 + 2 + 93 * 8;
        let text_fc = 1024usize;
        let mut wd = vec![0u8; text_fc + units.len() * 2];
        wd[0..2].copy_from_slice(&0xA5ECu16.to_le_bytes());
        wd[2..4].copy_from_slice(&0x00C1u16.to_le_bytes());
        wd[0x0A..0x0C].copy_from_slice(&0x0200u16.to_le_bytes()); // fWhichTblStm → 1Table
        wd[32..34].copy_from_slice(&14u16.to_le_bytes());
        wd[62..64].copy_from_slice(&22u16.to_le_bytes());
        wd[152..154].copy_from_slice(&93u16.to_le_bytes());
        assert!(fib_len < text_fc);
        for (i, u) in units.iter().enumerate() {
            wd[text_fc + i * 2..text_fc + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        // Clx: Pcdt with one piece (CP 0..n, fc = text_fc, not compressed)
        let mut plc = Vec::new();
        plc.extend_from_slice(&0u32.to_le_bytes());
        plc.extend_from_slice(&(units.len() as u32).to_le_bytes());
        plc.extend_from_slice(&0u16.to_le_bytes());
        plc.extend_from_slice(&(text_fc as u32).to_le_bytes());
        plc.extend_from_slice(&0u16.to_le_bytes());
        let mut clx = vec![2u8];
        clx.extend_from_slice(&(plc.len() as u32).to_le_bytes());
        clx.extend_from_slice(&plc);
        let mut table = vec![0u8; 16];
        let fc_clx = table.len();
        table.extend_from_slice(&clx);
        let base = 154 + 33 * 8;
        wd[base..base + 4].copy_from_slice(&(fc_clx as u32).to_le_bytes());
        wd[base + 4..base + 8].copy_from_slice(&(clx.len() as u32).to_le_bytes());
        compound(&[("WordDocument", &wd), ("1Table", &table), ("\u{5}SummaryInformation", &summary_info("Brief an Bank", "Erika"))])
    }

    /// SummaryInformation with title (PID 2), author (4), creation date (12).
    pub fn summary_info(title: &str, author: &str) -> Vec<u8> {
        let mut props: Vec<(u32, Vec<u8>)> = Vec::new();
        let lpstr = |s: &str| {
            let mut v = 30u32.to_le_bytes().to_vec();
            let mut b = s.as_bytes().to_vec();
            b.push(0);
            v.extend_from_slice(&(b.len() as u32).to_le_bytes());
            v.extend_from_slice(&b);
            while !v.len().is_multiple_of(4) {
                v.push(0);
            }
            v
        };
        let mut cp = 2u32.to_le_bytes().to_vec();
        cp.extend_from_slice(&65001u16.to_le_bytes());
        cp.extend_from_slice(&[0, 0]);
        props.push((1, cp));
        props.push((2, lpstr(title)));
        props.push((4, lpstr(author)));
        // 2024-01-02T03:04:05Z
        let ft: u64 = (1_704_164_645u64 + 11_644_473_600) * 10_000_000;
        let mut t = 64u32.to_le_bytes().to_vec();
        t.extend_from_slice(&ft.to_le_bytes());
        props.push((12, t));
        let mut sec = Vec::new();
        let head = 8 + props.len() * 8;
        let mut offs = Vec::new();
        let mut body = Vec::new();
        for (pid, v) in &props {
            offs.push((*pid, head + body.len()));
            body.extend_from_slice(v);
        }
        sec.extend_from_slice(&((head + body.len()) as u32).to_le_bytes());
        sec.extend_from_slice(&(props.len() as u32).to_le_bytes());
        for (pid, o) in offs {
            sec.extend_from_slice(&pid.to_le_bytes());
            sec.extend_from_slice(&(o as u32).to_le_bytes());
        }
        sec.extend_from_slice(&body);
        let mut b = Vec::new();
        b.extend_from_slice(&0xFFFEu16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&[0; 16]);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&[0xE0, 0x85, 0x9F, 0xF2, 0xF9, 0x4F, 0x68, 0x10, 0xAB, 0x91, 0x08, 0x00, 0x2B, 0x27, 0xB3, 0xD9]);
        b.extend_from_slice(&48u32.to_le_bytes());
        b.extend_from_slice(&sec);
        b
    }

    fn rec(ty: u16, data: &[u8]) -> Vec<u8> {
        let mut v = ty.to_le_bytes().to_vec();
        v.extend_from_slice(&(data.len() as u16).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    /// Minimal BIFF8 workbook: one sheet "Umsatz", SST across a CONTINUE boundary.
    pub fn biff8() -> Vec<u8> {
        let mut wb = Vec::new();
        wb.extend(rec(0x0809, &[0, 6, 5, 0, 0, 0, 0, 0]));
        let mut bs = vec![0u8; 6];
        bs.extend_from_slice(&[6, 0]);
        bs.extend_from_slice(b"Umsatz");
        wb.extend(rec(0x0085, &bs));
        // SST: 2 strings, the second ("Bäckerei") continues in CONTINUE (UTF-16 there)
        let mut sst = Vec::new();
        sst.extend_from_slice(&2u32.to_le_bytes());
        sst.extend_from_slice(&2u32.to_le_bytes());
        sst.extend_from_slice(&5u16.to_le_bytes());
        sst.push(0);
        sst.extend_from_slice(b"Kunde");
        sst.extend_from_slice(&8u16.to_le_bytes());
        sst.push(0);
        sst.extend_from_slice(b"B");
        wb.extend(rec(0x00FC, &sst));
        let mut cont = vec![1u8];
        for u in "äckerei".encode_utf16() {
            cont.extend_from_slice(&u.to_le_bytes());
        }
        wb.extend(rec(0x003C, &cont));
        wb.extend(rec(0x000A, &[]));
        wb.extend(rec(0x0809, &[0, 6, 0x10, 0, 0, 0, 0, 0]));
        let cell = |r: u16, c: u16| {
            let mut v = r.to_le_bytes().to_vec();
            v.extend_from_slice(&c.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
            v
        };
        let mut l = cell(0, 0);
        l.extend_from_slice(&0u32.to_le_bytes());
        wb.extend(rec(0x00FD, &l));
        let mut l = cell(1, 0);
        l.extend_from_slice(&1u32.to_le_bytes());
        wb.extend(rec(0x00FD, &l));
        let mut n = cell(1, 1);
        n.extend_from_slice(&1234.5f64.to_le_bytes());
        wb.extend(rec(0x0203, &n));
        let mut r = cell(1, 2);
        r.extend_from_slice(&((42u32 << 2) | 2).to_le_bytes());
        wb.extend(rec(0x027E, &r));
        wb.extend(rec(0x000A, &[]));
        compound(&[("Workbook", &wb)])
    }

    fn prec(ver: u16, inst: u16, ty: u16, data: &[u8]) -> Vec<u8> {
        let mut v = (ver | (inst << 4)).to_le_bytes().to_vec();
        v.extend_from_slice(&ty.to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    /// Minimal presentation: two slides (title + text in SlideListWithText, second slide with text
    /// in the drawing), persist directory and Current User.
    pub fn ppt() -> Vec<u8> {
        let utf16le = |s: &str| s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>();
        let persist = |id: u32| {
            let mut v = id.to_le_bytes().to_vec();
            v.extend_from_slice(&[0; 16]);
            prec(0, 0, RT_SLIDE_PERSIST, &v)
        };
        let mut slwt = Vec::new();
        slwt.extend(persist(2));
        slwt.extend(prec(0, 0, RT_TEXT_HEADER, &0u32.to_le_bytes()));
        slwt.extend(prec(0, 0, RT_TEXT_CHARS, &utf16le("Quartalsbericht")));
        slwt.extend(prec(0, 0, RT_TEXT_HEADER, &1u32.to_le_bytes()));
        slwt.extend(prec(0, 0, RT_TEXT_BYTES, b"Umsatz gestiegen\rKosten gesunken"));
        slwt.extend(persist(3));
        let doc = prec(0xF, 0, RT_DOCUMENT, &prec(0xF, 0, RT_SLWT, &slwt));
        let slide1 = prec(0xF, 0, RT_SLIDE, &prec(0, 0, RT_SLIDE_ATOM, &[0; 24]));
        let mut inner = prec(0, 0, RT_SLIDE_ATOM, &[0; 24]);
        inner.extend(prec(0xF, 0, 0xF00D, &prec(0, 0, RT_TEXT_CHARS, &utf16le("Ausblick 2025"))));
        let slide2 = prec(0xF, 0, RT_SLIDE, &inner);
        let mut pd = Vec::new();
        let off_doc = pd.len();
        pd.extend(doc);
        let off_s1 = pd.len();
        pd.extend(slide1);
        let off_s2 = pd.len();
        pd.extend(slide2);
        let off_dir = pd.len();
        let mut dir = (1u32 | (3 << 20)).to_le_bytes().to_vec();
        for o in [off_doc, off_s1, off_s2] {
            dir.extend_from_slice(&(o as u32).to_le_bytes());
        }
        pd.extend(prec(0, 0, RT_PERSIST_DIR, &dir));
        let off_edit = pd.len();
        let mut ue = Vec::new();
        ue.extend_from_slice(&0u32.to_le_bytes());
        ue.extend_from_slice(&[0; 4]);
        ue.extend_from_slice(&0u32.to_le_bytes()); // offsetLastEdit
        ue.extend_from_slice(&(off_dir as u32).to_le_bytes());
        ue.extend_from_slice(&1u32.to_le_bytes()); // docPersistIdRef
        ue.extend_from_slice(&[0; 8]);
        pd.extend(prec(0, 0, RT_USER_EDIT, &ue));
        let mut cu = vec![0u8; 16];
        cu.extend_from_slice(&(off_edit as u32).to_le_bytes());
        cu.extend_from_slice(&[0; 8]);
        compound(&[("PowerPoint Document", &pd), ("Current User", &cu)])
    }

    fn ctx() -> (Limits, Deadline) {
        let l = Limits::default();
        let d = Deadline::from_limits(&l);
        (l, d)
    }

    #[test]
    fn word97_text_and_metadata() {
        let data = word97("Sehr geehrte Damen\rIhr Konto \u{13} MERGEFIELD x \u{14}4711\u{15} ist gedeckt.\r");
        let (l, d) = ctx();
        let c = extract(&data, MIME_DOC, &l, d).unwrap();
        assert_eq!(c.text, "Sehr geehrte Damen\nIhr Konto 4711 ist gedeckt.\n");
        assert!(c.metadata.contains(&("dc:title".into(), "Brief an Bank".into())), "{:?}", c.metadata);
        assert!(c.metadata.contains(&("dc:creator".into(), "Erika".into())));
        assert_eq!(c.created.as_deref(), Some("2024-01-02T03:04:05Z"));
        assert_eq!(c.metadata.last(), Some(&("Content-Type".into(), MIME_DOC.into())));
    }

    #[test]
    fn word_encrypted_flag() {
        let mut data = word97("x");
        // set fEncrypted in the FIB: rebuild the stream
        let mut cf = open(&data).unwrap();
        let mut wd = stream(&mut cf, "WordDocument", u64::MAX).unwrap().unwrap();
        let table = stream(&mut cf, "1Table", u64::MAX).unwrap().unwrap();
        wd[0x0A] |= 0x00;
        wd[0x0B] |= 0x01;
        data = compound(&[("WordDocument", &wd), ("1Table", &table)]);
        let (l, d) = ctx();
        assert!(matches!(extract(&data, MIME_DOC, &l, d), Err(ParseError::Encrypted)));
    }

    /// upstream test_tika_liva.py::test_tika_parse_doc (real Word 97 file from upstream).
    #[test]
    fn upstream_sample_doc() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../upstream/src/paperless/tests/samples/tika/sample.doc");
        let Ok(data) = std::fs::read(&p) else {
            eprintln!("upstream sample missing, skipped");
            return;
        };
        let (l, d) = ctx();
        let c = extract(&data, MIME_DOC, &l, d).unwrap();
        assert!(c.text.contains("This is a test document, saved in the older .doc format"), "{}", c.text);
        assert!(c.metadata.iter().any(|(k, _)| k == "dcterms:created"), "{:?}", c.metadata);
    }

    #[test]
    fn biff8_cells_and_continue() {
        let (l, d) = ctx();
        let c = extract(&biff8(), MIME_XLS, &l, d).unwrap();
        assert_eq!(c.text, "Umsatz\n\tKunde\n\tBäckerei\t1234.5\t42\n\n");
    }

    #[test]
    fn ppt_slides_with_persist_directory() {
        let (l, d) = ctx();
        let s = ppt_slides(&ppt(), &l, d).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].title.as_deref(), Some("Quartalsbericht"));
        assert_eq!(s[0].lines, vec!["Umsatz gestiegen", "Kosten gesunken"]);
        assert_eq!(s[1].lines, vec!["Ausblick 2025"]);
        let c = extract(&ppt(), MIME_PPT, &l, d).unwrap();
        assert!(c.text.contains("Quartalsbericht\nUmsatz gestiegen\nKosten gesunken\n\nAusblick 2025\n"), "{}", c.text);
    }

    /// Gap E13: numbers according to cell format (FORMAT/XF/DATEMODE) like POI `DataFormatter`.
    #[test]
    fn biff8_cell_formats() {
        let mut wb = Vec::new();
        wb.extend(rec(0x0809, &[0, 6, 5, 0, 0, 0, 0, 0]));
        // FORMAT 164 = "dd.mm.yyyy" (XLUnicodeString, 8 Bit)
        let mut f = 164u16.to_le_bytes().to_vec();
        f.extend_from_slice(&10u16.to_le_bytes());
        f.push(0);
        f.extend_from_slice(b"dd.mm.yyyy");
        wb.extend(rec(0x041E, &f));
        // XF 0: General, XF 1: 164, XF 2: 10 (0.00%), XF 3: 14 (m/d/yy)
        for ifmt in [0u16, 164, 10, 14] {
            let mut xf = vec![0u8; 20];
            xf[2..4].copy_from_slice(&ifmt.to_le_bytes());
            wb.extend(rec(0x00E0, &xf));
        }
        let mut bs = vec![0u8; 6];
        bs.extend_from_slice(&[1, 0]);
        bs.push(b'T');
        wb.extend(rec(0x0085, &bs));
        wb.extend(rec(0x000A, &[]));
        wb.extend(rec(0x0809, &[0, 6, 0x10, 0, 0, 0, 0, 0]));
        let num = |c: u16, xf: u16, v: f64| {
            let mut n = 0u16.to_le_bytes().to_vec();
            n.extend_from_slice(&c.to_le_bytes());
            n.extend_from_slice(&xf.to_le_bytes());
            n.extend_from_slice(&v.to_le_bytes());
            rec(0x0203, &n)
        };
        wb.extend(num(0, 1, 45366.0));
        wb.extend(num(1, 2, 0.256));
        wb.extend(num(2, 0, 7.5));
        // MULRK with two values: XF 3 (date) and XF 0
        let mut m = 0u16.to_le_bytes().to_vec();
        m.extend_from_slice(&3u16.to_le_bytes());
        m.extend_from_slice(&3u16.to_le_bytes());
        m.extend_from_slice(&((45366u32 << 2) | 2).to_le_bytes());
        m.extend_from_slice(&0u16.to_le_bytes());
        m.extend_from_slice(&((9u32 << 2) | 2).to_le_bytes());
        m.extend_from_slice(&4u16.to_le_bytes());
        wb.extend(rec(0x00BD, &m));
        wb.extend(rec(0x000A, &[]));
        let (l, d) = ctx();
        let c = extract(&compound(&[("Workbook", &wb)]), MIME_XLS, &l, d).unwrap();
        assert_eq!(c.text, "T\n\t15.03.2024\t25.60%\t7.5\t3/15/24\t9\n\n");
    }

    #[test]
    fn numbers_and_rk() {
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(0.1), "0.1");
        assert_eq!(rk((5u32 << 2) | 2), 5.0);
        assert_eq!(rk((1234u32 << 2) | 3), 12.34);
        assert_eq!(rk(((1.5f64.to_bits() >> 32) as u32) & 0xFFFF_FFFC), 1.5);
    }

    #[test]
    fn garbage_never_panics() {
        let (l, d) = ctx();
        for m in [MIME_DOC, MIME_XLS, MIME_PPT] {
            assert!(extract(b"not ole", m, &l, d).is_err());
            assert!(extract(&compound(&[("WordDocument", &[0xEC, 0xA5, 1])]), m, &l, d).is_err());
        }
        assert_eq!(clean_word_text("a\u{13}b\u{13}c\u{14}d\u{15}e\u{14}f\u{15}g"), "afg");
    }
}
