//! Read/write ZIP for `document_exporter --zip` / `document_importer <zip>`.
//!
//! upstream: CPython 3.12 Lib/zipfile/__init__.py (`ZipFile.write`, `writestr`, `mkdir`,
//! `_write_end_record`, `ZipInfo.FileHeader`, `extractall` + `_sanitize_windows_name`), as used by
//! src/documents/export/sinks.py (`ZipExportSink`) and document_importer.py.
//! Supported: stored, deflated, bzip2 (12), lzma (14, raw LZMA1 with 4-byte header and end marker like
//! `zipfile.LZMACompressor`), zstd (93, like Python 3.14 `ZIP_ZSTANDARD`), ZIP64 (B6).

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{Datelike, Timelike};

const ZIP64_LIMIT: u64 = (1 << 31) - 1;
const ZIP_FILECOUNT_LIMIT: usize = (1 << 16) - 1;
/// Largest central directory block that is read into memory (review M6).
const MAX_CENTRAL_DIR: u64 = 256 << 20;
/// Smallest entry in the central directory (fixed part).
const CD_ENTRY_MIN: u64 = 46;
/// Unzip limit without `PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES`: 1 GiB + 100 × archive size.
const UNZIP_BASE: u64 = 1 << 30;
const UNZIP_RATIO: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Stored,
    Deflated,
    Bzip2,
    Lzma,
    Zstd,
}

/// `zipfile.ZIP_BZIP2`, `ZIP_LZMA`, `ZIP_ZSTANDARD`.
pub const ZIP_BZIP2: u16 = 12;
pub const ZIP_LZMA: u16 = 14;
pub const ZIP_ZSTANDARD: u16 = 93;
/// Methods that can be read and written.
pub const READABLE_METHODS: &[u16] = &[0, 8, ZIP_BZIP2, ZIP_LZMA, ZIP_ZSTANDARD];

impl Method {
    fn id(self) -> u16 {
        match self {
            Method::Stored => 0,
            Method::Deflated => 8,
            Method::Bzip2 => ZIP_BZIP2,
            Method::Lzma => ZIP_LZMA,
            Method::Zstd => ZIP_ZSTANDARD,
        }
    }

    /// `ZipInfo.FileHeader` min_version: `BZIP2_VERSION` 46, `LZMA_VERSION`/`ZSTANDARD_VERSION` 63.
    fn version(self) -> u16 {
        match self {
            Method::Stored | Method::Deflated => 20,
            Method::Bzip2 => 46,
            Method::Lzma | Method::Zstd => 63,
        }
    }
}

/// `zipfile.LZMACompressor._init`: `struct.pack('<BBH', 9, 4, len(props))` (LZMA-SDK 9.04).
const LZMA_ZIP_HEADER: [u8; 4] = [9, 4, 5, 0];

/// Writes everything except the bytes `[from, to)` of the data stream (removes the 8-byte size from the
/// 13-byte header of the ".lzma" format; what remains is 5 bytes of properties + raw LZMA1).
struct SkipRange<W: Write> {
    inner: W,
    pos: u64,
    from: u64,
    to: u64,
}

impl<W: Write> Write for SkipRange<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let start = self.pos;
        let end = start + buf.len() as u64;
        if end <= self.from || start >= self.to {
            self.inner.write_all(buf)?;
        } else {
            let a = self.from.saturating_sub(start).min(buf.len() as u64) as usize;
            let b = self.to.saturating_sub(start).min(buf.len() as u64) as usize;
            self.inner.write_all(&buf[..a])?;
            self.inner.write_all(&buf[b..])?;
        }
        self.pos = end;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn lzma_encoder_stream() -> io::Result<xz2::stream::Stream> {
    // Python: lzma._encode_filter_properties({'id': FILTER_LZMA1}) = preset 6
    let opts = xz2::stream::LzmaOptions::new_preset(6).map_err(io::Error::other)?;
    xz2::stream::Stream::new_lzma_encoder(&opts).map_err(io::Error::other)
}

/// Data from `src` to `w`, counting CRC and length along the way.
fn pump(src: &mut dyn Read, w: &mut dyn Write, crc: &mut flate2::Crc) -> io::Result<u64> {
    let mut buf = vec![0u8; 1 << 16];
    let mut total: u64 = 0;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        crc.update(&buf[..n]);
        total += n as u64;
        w.write_all(&buf[..n])?;
    }
}

struct Entry {
    name: Vec<u8>,
    flags: u16,
    method: u16,
    dos_time: u16,
    dos_date: u16,
    crc: u32,
    compress_size: u64,
    file_size: u64,
    header_offset: u64,
    external_attr: u32,
    extract_version: u16,
}

pub struct ZipWriter {
    out: BufWriter<File>,
    entries: Vec<Entry>,
    method: Method,
    level: Option<i32>,
    tz: chrono_tz::Tz,
}

fn dos_datetime(t: chrono::DateTime<chrono_tz::Tz>) -> (u16, u16) {
    let year = t.year().clamp(1980, 2107);
    let date = (((year - 1980) as u16) << 9) | ((t.month() as u16) << 5) | t.day() as u16;
    let time = ((t.hour() as u16) << 11) | ((t.minute() as u16) << 5) | (t.second() as u16 / 2);
    (time, date)
}

const CREATE_SYSTEM: u16 = if cfg!(windows) { 0 } else { 3 };

impl ZipWriter {
    /// `level`: deflated 0–9 (default 6), bzip2 1–9 (default 9), zstd −22–22 (default 3); lzma
    /// without a level (preset 6).
    pub fn create(path: &Path, method: Method, level: Option<i32>, tz: chrono_tz::Tz) -> io::Result<ZipWriter> {
        let f = File::create(path)?;
        Ok(ZipWriter { out: BufWriter::new(f), entries: Vec::new(), method, level, tz })
    }

    fn pos(&mut self) -> io::Result<u64> {
        self.out.stream_position()
    }

    fn local_header(&mut self, e: &Entry, zip64: bool) -> io::Result<()> {
        let w = &mut self.out;
        w.write_all(&0x04034b50u32.to_le_bytes())?;
        w.write_all(&e.extract_version.to_le_bytes())?;
        w.write_all(&e.flags.to_le_bytes())?;
        w.write_all(&e.method.to_le_bytes())?;
        w.write_all(&e.dos_time.to_le_bytes())?;
        w.write_all(&e.dos_date.to_le_bytes())?;
        w.write_all(&e.crc.to_le_bytes())?;
        if zip64 {
            w.write_all(&u32::MAX.to_le_bytes())?;
            w.write_all(&u32::MAX.to_le_bytes())?;
        } else {
            w.write_all(&(e.compress_size as u32).to_le_bytes())?;
            w.write_all(&(e.file_size as u32).to_le_bytes())?;
        }
        w.write_all(&(e.name.len() as u16).to_le_bytes())?;
        w.write_all(&(if zip64 { 20u16 } else { 0u16 }).to_le_bytes())?;
        w.write_all(&e.name)?;
        if zip64 {
            w.write_all(&1u16.to_le_bytes())?;
            w.write_all(&16u16.to_le_bytes())?;
            w.write_all(&e.file_size.to_le_bytes())?;
            w.write_all(&e.compress_size.to_le_bytes())?;
        }
        Ok(())
    }

    fn new_entry(&self, arcname: &str, mtime: chrono::DateTime<chrono_tz::Tz>, external_attr: u32, method: u16) -> Entry {
        let (dos_time, dos_date) = dos_datetime(mtime);
        let mut flags = if arcname.is_ascii() { 0 } else { 0x800 };
        // zipfile: LZMA → _MASK_COMPRESS_OPTION_1 (end marker present)
        if method == ZIP_LZMA {
            flags |= 0x02;
        }
        let extract_version = match method {
            ZIP_BZIP2 => Method::Bzip2.version(),
            ZIP_LZMA | ZIP_ZSTANDARD => Method::Lzma.version(),
            _ => 20,
        };
        Entry {
            name: arcname.as_bytes().to_vec(),
            flags,
            method,
            dos_time,
            dos_date,
            crc: 0,
            compress_size: 0,
            file_size: 0,
            header_offset: 0,
            external_attr,
            extract_version,
        }
    }

    /// Write the data stream: header (sizes 0) → data → overwrite header with sizes.
    fn write_stream(&mut self, mut e: Entry, src: &mut dyn Read, size_hint: u64) -> io::Result<()> {
        let zip64 = size_hint as f64 * 1.05 > ZIP64_LIMIT as f64;
        if zip64 {
            e.extract_version = e.extract_version.max(45);
        }
        e.header_offset = self.pos()?;
        self.local_header(&e, zip64)?;
        let data_start = self.pos()?;
        let mut crc = flate2::Crc::new();
        let level = self.level;
        let total = match e.method {
            8 => {
                let lv = flate2::Compression::new(level.unwrap_or(6).clamp(0, 9) as u32);
                let mut enc = flate2::write::DeflateEncoder::new(&mut self.out, lv);
                let t = pump(src, &mut enc, &mut crc)?;
                enc.finish()?;
                t
            }
            ZIP_BZIP2 => {
                let lv = bzip2::Compression::new(level.unwrap_or(9).clamp(1, 9) as u32);
                let mut enc = bzip2::write::BzEncoder::new(&mut self.out, lv);
                let t = pump(src, &mut enc, &mut crc)?;
                enc.finish()?;
                t
            }
            ZIP_LZMA => {
                self.out.write_all(&LZMA_ZIP_HEADER)?;
                let sink = SkipRange { inner: &mut self.out, pos: 0, from: 5, to: 13 };
                let mut enc = xz2::write::XzEncoder::new_stream(sink, lzma_encoder_stream()?);
                let t = pump(src, &mut enc, &mut crc)?;
                enc.finish()?;
                t
            }
            ZIP_ZSTANDARD => {
                let mut enc = zstd::stream::write::Encoder::new(&mut self.out, level.unwrap_or(3).clamp(-22, 22))?;
                let t = pump(src, &mut enc, &mut crc)?;
                enc.finish()?;
                t
            }
            _ => pump(src, &mut self.out, &mut crc)?,
        };
        let end = self.pos()?;
        e.crc = crc.sum();
        e.file_size = total;
        e.compress_size = end - data_start;
        if !zip64 && (e.file_size > ZIP64_LIMIT || e.compress_size > ZIP64_LIMIT) {
            return Err(io::Error::other("File size too large, try using force_zip64"));
        }
        self.out.seek(SeekFrom::Start(e.header_offset))?;
        self.local_header(&e, zip64)?;
        self.out.seek(SeekFrom::Start(end))?;
        self.entries.push(e);
        Ok(())
    }

    /// `ZipFile.write(source, arcname)`.
    pub fn write_file(&mut self, src: &Path, arcname: &str) -> io::Result<()> {
        let meta = std::fs::metadata(src)?;
        let mtime: chrono::DateTime<chrono::Utc> = meta.modified().map(Into::into).unwrap_or_else(|_| chrono::Utc::now());
        let mode: u32 = file_mode(&meta);
        let e = self.new_entry(arcname, mtime.with_timezone(&self.tz), (mode & 0xFFFF) << 16, self.method.id());
        let mut f = BufReader::new(File::open(src)?);
        self.write_stream(e, &mut f, meta.len())
    }

    /// `ZipFile.writestr(arcname, data)` (time = now, mode 0o600).
    pub fn write_bytes(&mut self, arcname: &str, data: &[u8]) -> io::Result<()> {
        let now = chrono::Utc::now().with_timezone(&self.tz);
        let e = self.new_entry(arcname, now, 0o600 << 16, self.method.id());
        let mut r = data;
        self.write_stream(e, &mut r, data.len() as u64)
    }

    /// `ZipFile.mkdir(dir_arc)` (`dir/`, mode 0o40777, MS-DOS directory bit).
    pub fn mkdir(&mut self, dir_arc: &str) -> io::Result<()> {
        let name = if dir_arc.ends_with('/') { dir_arc.to_string() } else { format!("{dir_arc}/") };
        let now = chrono::Utc::now().with_timezone(&self.tz);
        let mut e = self.new_entry(&name, now, (((0o40000 | 0o777) & 0xFFFF) << 16) | 0x10, 0);
        e.header_offset = self.pos()?;
        self.local_header(&e, false)?;
        self.entries.push(e);
        Ok(())
    }

    /// Central directory + end record (ZIP64 if needed), close the file.
    pub fn finish(mut self) -> io::Result<()> {
        let cd_start = self.pos()?;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let mut extra: Vec<u64> = Vec::new();
            let (fs, cs, off) = (e.file_size, e.compress_size, e.header_offset);
            let fs32 = if fs > ZIP64_LIMIT {
                extra.push(fs);
                u32::MAX
            } else {
                fs as u32
            };
            let cs32 = if cs > ZIP64_LIMIT {
                extra.push(cs);
                u32::MAX
            } else {
                cs as u32
            };
            let off32 = if off > ZIP64_LIMIT {
                extra.push(off);
                u32::MAX
            } else {
                off as u32
            };
            let version = if extra.is_empty() { e.extract_version } else { 45 };
            let w = &mut self.out;
            w.write_all(&0x02014b50u32.to_le_bytes())?;
            w.write_all(&((CREATE_SYSTEM << 8) | version.max(20)).to_le_bytes())?;
            w.write_all(&version.to_le_bytes())?;
            w.write_all(&e.flags.to_le_bytes())?;
            w.write_all(&e.method.to_le_bytes())?;
            w.write_all(&e.dos_time.to_le_bytes())?;
            w.write_all(&e.dos_date.to_le_bytes())?;
            w.write_all(&e.crc.to_le_bytes())?;
            w.write_all(&cs32.to_le_bytes())?;
            w.write_all(&fs32.to_le_bytes())?;
            w.write_all(&(e.name.len() as u16).to_le_bytes())?;
            let extra_len = if extra.is_empty() { 0 } else { 4 + 8 * extra.len() };
            w.write_all(&(extra_len as u16).to_le_bytes())?;
            w.write_all(&0u16.to_le_bytes())?; // comment
            w.write_all(&0u16.to_le_bytes())?; // disk
            w.write_all(&0u16.to_le_bytes())?; // internal attr
            w.write_all(&e.external_attr.to_le_bytes())?;
            w.write_all(&off32.to_le_bytes())?;
            w.write_all(&e.name)?;
            if !extra.is_empty() {
                w.write_all(&1u16.to_le_bytes())?;
                w.write_all(&((8 * extra.len()) as u16).to_le_bytes())?;
                for x in &extra {
                    w.write_all(&x.to_le_bytes())?;
                }
            }
        }
        let cd_end = self.pos()?;
        let (count, cd_size) = (entries.len(), cd_end - cd_start);
        let w = &mut self.out;
        if count > ZIP_FILECOUNT_LIMIT || cd_start > ZIP64_LIMIT || cd_size > ZIP64_LIMIT {
            w.write_all(&0x06064b50u32.to_le_bytes())?;
            w.write_all(&44u64.to_le_bytes())?;
            w.write_all(&45u16.to_le_bytes())?;
            w.write_all(&45u16.to_le_bytes())?;
            w.write_all(&0u32.to_le_bytes())?;
            w.write_all(&0u32.to_le_bytes())?;
            w.write_all(&(count as u64).to_le_bytes())?;
            w.write_all(&(count as u64).to_le_bytes())?;
            w.write_all(&cd_size.to_le_bytes())?;
            w.write_all(&cd_start.to_le_bytes())?;
            w.write_all(&0x07064b50u32.to_le_bytes())?;
            w.write_all(&0u32.to_le_bytes())?;
            w.write_all(&cd_end.to_le_bytes())?;
            w.write_all(&1u32.to_le_bytes())?;
        }
        let c16 = count.min(0xFFFF) as u16;
        w.write_all(&0x06054b50u32.to_le_bytes())?;
        w.write_all(&0u16.to_le_bytes())?;
        w.write_all(&0u16.to_le_bytes())?;
        w.write_all(&c16.to_le_bytes())?;
        w.write_all(&c16.to_le_bytes())?;
        w.write_all(&(cd_size.min(u64::from(u32::MAX)) as u32).to_le_bytes())?;
        w.write_all(&(cd_start.min(u64::from(u32::MAX)) as u32).to_le_bytes())?;
        w.write_all(&0u16.to_le_bytes())?;
        w.flush()?;
        Ok(())
    }
}

#[cfg(unix)]
fn file_mode(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode()
}

#[cfg(not(unix))]
fn file_mode(meta: &std::fs::Metadata) -> u32 {
    // Python os.stat on Windows: 0o100666 or 0o100444 (read-only)
    if meta.permissions().readonly() { 0o100444 } else { 0o100666 }
}

// ---------------------------------------------------------------- Reading

#[derive(Debug, Clone)]
pub struct ReadEntry {
    pub name: String,
    pub flags: u16,
    pub method: u16,
    pub crc: u32,
    pub compress_size: u64,
    pub file_size: u64,
    pub header_offset: u64,
}

pub struct ZipReader {
    file: File,
    pub entries: Vec<ReadEntry>,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

/// `zipfile.is_zipfile(path)`: file with a valid end record.
pub fn is_zipfile(path: &Path) -> bool {
    path.is_file() && ZipReader::open(path).is_ok()
}

impl ZipReader {
    pub fn open(path: &Path) -> io::Result<ZipReader> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "File is not a zip file");
        if len < 22 {
            return Err(bad());
        }
        let tail_len = len.min(22 + 65535);
        file.seek(SeekFrom::Start(len - tail_len))?;
        let mut tail = vec![0u8; tail_len as usize];
        file.read_exact(&mut tail)?;
        let eocd = (0..tail.len().saturating_sub(21)).rev().find(|&i| u32le(&tail, i) == 0x06054b50).ok_or_else(bad)?;
        let mut count = u64::from(u16le(&tail, eocd + 10));
        let mut cd_size = u64::from(u32le(&tail, eocd + 12));
        let mut cd_off = u64::from(u32le(&tail, eocd + 16));
        // ZIP64 locator directly before the end record
        let eocd_abs = len - tail_len + eocd as u64;
        if eocd_abs >= 20 {
            let mut loc = [0u8; 20];
            file.seek(SeekFrom::Start(eocd_abs - 20))?;
            file.read_exact(&mut loc)?;
            if u32le(&loc, 0) == 0x07064b50 {
                let rec_off = u64le(&loc, 8);
                if rec_off.checked_add(56).is_none_or(|end| end > eocd_abs - 20) {
                    return Err(bad());
                }
                let mut rec = [0u8; 56];
                file.seek(SeekFrom::Start(rec_off))?;
                file.read_exact(&mut rec)?;
                if u32le(&rec, 0) != 0x06064b50 {
                    return Err(bad());
                }
                count = u64le(&rec, 32);
                cd_size = u64le(&rec, 40);
                cd_off = u64le(&rec, 48);
            }
        }
        // checked: a manipulated ZIP64 end record must neither overflow nor request huge buffers
        // (review M6)
        if cd_off.checked_add(cd_size).is_none_or(|end| end > len) || cd_size > MAX_CENTRAL_DIR {
            return Err(bad());
        }
        if count > cd_size / CD_ENTRY_MIN {
            return Err(bad());
        }
        file.seek(SeekFrom::Start(cd_off))?;
        let mut cd = vec![0u8; usize::try_from(cd_size).map_err(|_| bad())?];
        file.read_exact(&mut cd)?;
        // count ≤ cd_size / 46 ≤ 5.8 million – still do not pre-allocate from input data
        let mut entries = Vec::new();
        let mut p = 0usize;
        while p + 46 <= cd.len() && u32le(&cd, p) == 0x02014b50 {
            let flags = u16le(&cd, p + 8);
            let method = u16le(&cd, p + 10);
            let crc = u32le(&cd, p + 16);
            let mut cs = u64::from(u32le(&cd, p + 20));
            let mut fs = u64::from(u32le(&cd, p + 24));
            let nl = usize::from(u16le(&cd, p + 28));
            let el = usize::from(u16le(&cd, p + 30));
            let kl = usize::from(u16le(&cd, p + 32));
            let mut off = u64::from(u32le(&cd, p + 42));
            let name_b = cd.get(p + 46..p + 46 + nl).ok_or_else(bad)?;
            let name = if flags & 0x800 != 0 {
                String::from_utf8_lossy(name_b).into_owned()
            } else {
                // cp437 → identical for ASCII; otherwise decode like Python (cp437)
                name_b.iter().map(|&b| cp437(b)).collect()
            };
            let extra = cd.get(p + 46 + nl..p + 46 + nl + el).ok_or_else(bad)?;
            let mut q = 0;
            while q + 4 <= extra.len() {
                let tag = u16le(extra, q);
                let sz = usize::from(u16le(extra, q + 2));
                if tag == 1 {
                    let mut r = q + 4;
                    let end = (q + 4 + sz).min(extra.len());
                    if fs == u64::from(u32::MAX) && r + 8 <= end {
                        fs = u64le(extra, r);
                        r += 8;
                    }
                    if cs == u64::from(u32::MAX) && r + 8 <= end {
                        cs = u64le(extra, r);
                        r += 8;
                    }
                    if off == u64::from(u32::MAX) && r + 8 <= end {
                        off = u64le(extra, r);
                    }
                }
                q += 4 + sz;
            }
            if off.checked_add(30).and_then(|v| v.checked_add(cs)).is_none_or(|end| end > len) {
                return Err(bad());
            }
            entries.push(ReadEntry { name, flags, method, crc, compress_size: cs, file_size: fs, header_offset: off });
            p += 46 + nl + el + kl;
        }
        Ok(ZipReader { file, entries })
    }

    /// Method IDs that cannot be read (everything except [`READABLE_METHODS`]).
    pub fn unsupported_methods(&self) -> std::collections::BTreeSet<u16> {
        self.entries.iter().map(|e| e.method).filter(|m| !READABLE_METHODS.contains(m)).collect()
    }

    /// Unzip limit: `PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES` or 1 GiB + 100 × archive size.
    pub fn default_unzip_limit(&self) -> u64 {
        if let Some(v) = pl_core::config::var("PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES").and_then(|v| v.trim().parse::<u64>().ok()) {
            return v;
        }
        let len = self.file.metadata().map(|m| m.len()).unwrap_or(0);
        UNZIP_BASE.saturating_add(len.saturating_mul(UNZIP_RATIO))
    }

    /// `ZipFile.extractall(dir)` (paths sanitized like `_extract_member`), with unzip limit
    /// [`ZipReader::default_unzip_limit`].
    pub fn extract_all(&mut self, dir: &Path) -> io::Result<()> {
        let limit = self.default_unzip_limit();
        self.extract_all_limited(dir, limit)
    }

    /// Like [`ZipReader::extract_all`], sum of the unpacked sizes ≤ `max_total` (zip bombs,
    /// review M6). Each entry must yield exactly its stated size.
    pub fn extract_all_limited(&mut self, dir: &Path, max_total: u64) -> io::Result<()> {
        let too_big = || io::Error::new(io::ErrorKind::InvalidData, format!("ZIP archive expands to more than {max_total} bytes (limit, see PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES)"));
        let mut total: u64 = 0;
        for e in &self.entries {
            total = total.checked_add(e.file_size).ok_or_else(too_big)?;
        }
        if total > max_total {
            return Err(too_big());
        }
        let entries = self.entries.clone();
        for e in entries {
            let Some(target) = sanitize_member(dir, &e.name) else { continue };
            if e.name.ends_with('/') {
                std::fs::create_dir_all(&target)?;
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut lh = [0u8; 30];
            self.file.seek(SeekFrom::Start(e.header_offset))?;
            self.file.read_exact(&mut lh)?;
            if u32le(&lh, 0) != 0x04034b50 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Bad magic number for file header"));
            }
            let skip = u64::from(u16le(&lh, 26)) + u64::from(u16le(&lh, 28));
            self.file.seek(SeekFrom::Current(skip as i64))?;
            let limited = (&mut self.file).take(e.compress_size);
            let mut reader = decoder(limited, &e)?;
            let mut out = BufWriter::new(File::create(&target)?);
            let mut crc = flate2::Crc::new();
            let mut buf = vec![0u8; 1 << 16];
            let mut written: u64 = 0;
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                written = written.saturating_add(n as u64);
                if written > e.file_size {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!("File '{}' is larger than declared", e.name)));
                }
                crc.update(&buf[..n]);
                out.write_all(&buf[..n])?;
            }
            out.flush()?;
            if written != e.file_size {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("File '{}' is truncated", e.name)));
            }
            if crc.sum() != e.crc {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("Bad CRC-32 for file '{}'", e.name)));
            }
        }
        Ok(())
    }
}

/// Decompressor for an entry (`zipfile._get_decompressor`). LZMA: read the 4-byte header + properties
/// and give the ".lzma" decoder a 13-byte header (size unknown if the bit
/// "end marker present" is set, otherwise the stated size).
fn decoder<'a, R: Read + 'a>(mut r: R, e: &ReadEntry) -> io::Result<Box<dyn Read + 'a>> {
    Ok(match e.method {
        0 => Box::new(r),
        8 => Box::new(flate2::read::DeflateDecoder::new(r)),
        ZIP_BZIP2 => Box::new(bzip2::read::BzDecoder::new(r)),
        ZIP_LZMA => {
            let mut head = [0u8; 4];
            r.read_exact(&mut head)?;
            let psize = usize::from(u16::from_le_bytes([head[2], head[3]]));
            if psize != 5 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("Bad LZMA properties size for file '{}'", e.name)));
            }
            let mut alone = [0u8; 13];
            r.read_exact(&mut alone[..5])?;
            let size = if e.flags & 0x02 != 0 { u64::MAX } else { e.file_size };
            alone[5..].copy_from_slice(&size.to_le_bytes());
            let stream = xz2::stream::Stream::new_lzma_decoder(u64::MAX).map_err(io::Error::other)?;
            Box::new(xz2::read::XzDecoder::new_stream(io::Cursor::new(alone).chain(r), stream))
        }
        ZIP_ZSTANDARD => Box::new(zstd::stream::read::Decoder::new(r)?),
        m => return Err(io::Error::other(format!("compression method {m} not supported"))),
    })
}

/// `ZipFile._extract_member`: drive/absolute parts, remove `.`/`..`; on Windows
/// replace invalid characters with `_`.
fn sanitize_member(dir: &Path, name: &str) -> Option<PathBuf> {
    let norm = name.replace('\\', "/");
    let mut out = dir.to_path_buf();
    let mut any = false;
    for part in norm.split('/') {
        let part = if cfg!(windows) && part.len() >= 2 && part.as_bytes()[1] == b':' { &part[2..] } else { part };
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        let clean: String = if cfg!(windows) {
            let s: String = part.chars().map(|c| if ":<>|\"?*".contains(c) { '_' } else { c }).collect();
            s.trim_end_matches('.').to_string()
        } else {
            part.to_string()
        };
        if clean.is_empty() {
            continue;
        }
        out.push(clean);
        any = true;
    }
    any.then_some(out)
}

fn cp437(b: u8) -> char {
    const HI: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";
    if b < 0x80 { b as char } else { HI.chars().nth(usize::from(b - 0x80)).unwrap_or('?') }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_methods_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("big.txt");
        let data: Vec<u8> = (0..200_000u32).flat_map(|i| format!("line {i} lorem ipsum\n").into_bytes()).collect();
        std::fs::write(&src, &data).unwrap();
        for (m, level, id) in [
            (Method::Stored, None, 0u16),
            (Method::Deflated, Some(9), 8),
            (Method::Bzip2, None, ZIP_BZIP2),
            (Method::Bzip2, Some(1), ZIP_BZIP2),
            (Method::Lzma, None, ZIP_LZMA),
            (Method::Zstd, None, ZIP_ZSTANDARD),
            (Method::Zstd, Some(-5), ZIP_ZSTANDARD),
        ] {
            let zpath = dir.path().join(format!("t{id}-{level:?}.zip"));
            let mut w = ZipWriter::create(&zpath, m, level, chrono_tz::UTC).unwrap();
            w.write_file(&src, "dir/big.txt").unwrap();
            w.write_bytes("manifest.json", b"[]").unwrap();
            w.mkdir("empty").unwrap();
            w.finish().unwrap();
            let mut r = ZipReader::open(&zpath).unwrap();
            assert!(r.unsupported_methods().is_empty(), "{m:?}");
            assert_eq!(r.entries[0].method, id);
            if m == Method::Lzma {
                assert_eq!(r.entries[0].flags & 0x02, 0x02, "EOS flag like zipfile");
            }
            if m != Method::Stored {
                assert!(r.entries[0].compress_size < data.len() as u64 / 3, "{m:?} compresses");
            }
            let out = dir.path().join(format!("out{id}-{level:?}"));
            r.extract_all(&out).unwrap();
            assert_eq!(std::fs::read(out.join("dir").join("big.txt")).unwrap(), data, "{m:?}");
            assert_eq!(std::fs::read(out.join("manifest.json")).unwrap(), b"[]");
        }
    }

    /// Byte layout of an LZMA entry like CPython (header 09 04 05 00, properties 5D 00 00 80 00).
    #[test]
    fn lzma_entry_layout_like_cpython() {
        let dir = tempfile::tempdir().unwrap();
        let zpath = dir.path().join("l.zip");
        let mut w = ZipWriter::create(&zpath, Method::Lzma, None, chrono_tz::UTC).unwrap();
        w.write_bytes("a", b"hello hello hello").unwrap();
        w.finish().unwrap();
        let raw = std::fs::read(&zpath).unwrap();
        // local header: 30 bytes + name (1)
        assert_eq!(u16le(&raw, 4), 63, "LZMA_VERSION");
        assert_eq!(u16le(&raw, 8), ZIP_LZMA);
        assert_eq!(&raw[31..35], &[9, 4, 5, 0]);
        assert_eq!(&raw[35..40], &[0x5D, 0x00, 0x00, 0x80, 0x00]);
    }

    #[test]
    fn write_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"hello hello hello hello").unwrap();
        let zpath = dir.path().join("x.zip");
        let mut w = ZipWriter::create(&zpath, Method::Deflated, None, chrono_tz::UTC).unwrap();
        w.mkdir("sub").unwrap();
        w.write_file(&src, "sub/ä.txt").unwrap();
        w.write_bytes("manifest.json", b"[]").unwrap();
        w.finish().unwrap();
        assert!(is_zipfile(&zpath));
        assert!(!is_zipfile(&src));
        let mut r = ZipReader::open(&zpath).unwrap();
        let names: Vec<&str> = r.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["sub/", "sub/ä.txt", "manifest.json"]);
        let out = dir.path().join("out");
        r.extract_all(&out).unwrap();
        assert_eq!(std::fs::read(out.join("sub").join("ä.txt")).unwrap(), b"hello hello hello hello");
        assert_eq!(std::fs::read(out.join("manifest.json")).unwrap(), b"[]");
        assert!(sanitize_member(&out, "../../etc/passwd").unwrap().starts_with(&out));
    }

    /// ZIP64 end record + locator + EOCD without entries, values freely selectable.
    fn zip64_stub(count: u64, cd_size: u64, cd_off: u64) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0x06064b50u32.to_le_bytes());
        b.extend_from_slice(&44u64.to_le_bytes());
        b.extend_from_slice(&[45, 0, 45, 0]);
        b.extend_from_slice(&[0u8; 8]);
        b.extend_from_slice(&count.to_le_bytes());
        b.extend_from_slice(&count.to_le_bytes());
        b.extend_from_slice(&cd_size.to_le_bytes());
        b.extend_from_slice(&cd_off.to_le_bytes());
        b.extend_from_slice(&0x07064b50u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&0x06054b50u32.to_le_bytes());
        b.extend_from_slice(&[0u8; 4]);
        b.extend_from_slice(&[0xff; 4]);
        b.extend_from_slice(&[0xff; 8]);
        b.extend_from_slice(&[0u8; 2]);
        b
    }

    #[test]
    fn zip64_hostile_counts_rejected_without_panic() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("evil.zip");
        // entry count u64::MAX (previously capacity-overflow panic already in is_zipfile)
        std::fs::write(&p, zip64_stub(u64::MAX, 0, 0)).unwrap();
        assert!(ZipReader::open(&p).is_err());
        assert!(!is_zipfile(&p));
        // cd_off + cd_size overflows
        std::fs::write(&p, zip64_stub(1, 64, u64::MAX - 10)).unwrap();
        assert!(ZipReader::open(&p).is_err());
        // huge central directory beyond the file
        std::fs::write(&p, zip64_stub(1, 1 << 40, 0)).unwrap();
        assert!(ZipReader::open(&p).is_err());
        // locator points past the end of the file
        let mut b = zip64_stub(0, 0, 0);
        let loc = 56 + 8;
        b[loc..loc + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        std::fs::write(&p, b).unwrap();
        assert!(ZipReader::open(&p).is_err());
        // too short
        std::fs::write(&p, b"PK").unwrap();
        assert!(!is_zipfile(&p));
    }

    #[test]
    fn unzip_limit_and_declared_size_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let zpath = dir.path().join("bomb.zip");
        let mut w = ZipWriter::create(&zpath, Method::Deflated, None, chrono_tz::UTC).unwrap();
        w.write_bytes("manifest.json", &vec![b'0'; 1 << 20]).unwrap();
        w.finish().unwrap();
        assert!(std::fs::metadata(&zpath).unwrap().len() < 10_000);
        let mut r = ZipReader::open(&zpath).unwrap();
        let e = r.extract_all_limited(&dir.path().join("o1"), 1000).unwrap_err();
        assert!(e.to_string().contains("expands to more than"), "{e}");
        assert!(!dir.path().join("o1").join("manifest.json").exists());
        r.extract_all_limited(&dir.path().join("o2"), 1 << 20).unwrap();
        // stated size smaller than the content (central directory forged) → abort
        r.entries[0].file_size = 10;
        let e = r.extract_all_limited(&dir.path().join("o3"), 1 << 30).unwrap_err();
        assert!(e.to_string().contains("larger than declared"), "{e}");
        let mut r = ZipReader::open(&zpath).unwrap();
        r.entries[0].file_size = 2 << 20;
        let e = r.extract_all_limited(&dir.path().join("o4"), 1 << 30).unwrap_err();
        assert!(e.to_string().contains("truncated"), "{e}");
    }
}
