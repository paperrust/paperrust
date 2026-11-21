// upstream: CPython 3.12 Lib/zipfile/__init__.py (ZipFile.write, ZipInfo.from_file,
//           FileHeader, _write_end_record) as used by src/documents/bulk_download.py@v3.4.2
//
// Minimal ZIP writer (stored / deflated, ZIP64 when needed) that streams each source file
// into a seekable output with constant memory. Byte layout follows CPython where it is cheap
// to do so (versions, flags, extra fields, Unix create_system); see Known difference 3.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use chrono::{Datelike, TimeZone, Timelike};

const ZIP64_LIMIT: u64 = (1 << 31) - 1;
const ZIP_FILECOUNT_LIMIT: usize = (1 << 16) - 1;
const DEFAULT_VERSION: u16 = 20;
const ZIP64_VERSION: u16 = 45;
const CREATE_SYSTEM_UNIX: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Stored,
    Deflated,
}

struct Entry {
    name: Vec<u8>,
    flags: u16,
    method: Method,
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
    names: HashSet<String>,
    tz: chrono_tz::Tz,
    method: Method,
}

/// `os.path.normpath` (POSIX) followed by stripping leading slashes, as `ZipInfo.from_file` does.
pub fn normalize_arcname(name: &str) -> String {
    let initial_slashes = name.starts_with('/');
    let mut comps: Vec<&str> = Vec::new();
    for c in name.split('/') {
        if c.is_empty() || c == "." {
            continue;
        }
        if c != ".." || (!initial_slashes && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(c);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let mut s = comps.join("/");
    if s.is_empty() && !initial_slashes {
        s = ".".to_string();
    }
    // ZipInfo: truncate at the first NUL
    if let Some(i) = s.find('\0') {
        s.truncate(i);
    }
    s
}

fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn dos_datetime(mtime: std::time::SystemTime, tz: chrono_tz::Tz) -> io::Result<(u16, u16)> {
    let secs = match mtime.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    let utc = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).ok_or_else(|| io::Error::other("invalid mtime"))?;
    let local = tz.from_utc_datetime(&utc.naive_utc());
    if local.year() < 1980 {
        // zipfile (strict_timestamps=True): ValueError -> 500 upstream
        return Err(io::Error::other("ZIP does not support timestamps before 1980"));
    }
    let date = (((local.year() - 1980) as u16) << 9) | ((local.month() as u16) << 5) | local.day() as u16;
    let time = ((local.hour() as u16) << 11) | ((local.minute() as u16) << 5) | (local.second() as u16 / 2);
    Ok((time, date))
}

#[cfg(unix)]
fn file_mode(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0xFFFF
}
#[cfg(not(unix))]
fn file_mode(_meta: &std::fs::Metadata) -> u32 {
    0o100644
}

impl ZipWriter {
    pub fn new(file: File, method: Method, tz: chrono_tz::Tz) -> Self {
        ZipWriter { out: BufWriter::with_capacity(64 * 1024, file), entries: Vec::new(), names: HashSet::new(), tz, method }
    }

    /// `filename in zipf.namelist()`
    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// `zipf.write(src, arcname)`. Missing source files are an error (500 upstream).
    pub fn write_file(&mut self, src: &Path, arcname: &str) -> io::Result<()> {
        let meta = std::fs::metadata(src)?;
        if meta.is_dir() {
            return Err(io::Error::other("source is a directory"));
        }
        let mut input = File::open(src)?;
        let name = normalize_arcname(arcname);
        let (dos_time, dos_date) = dos_datetime(meta.modified()?, self.tz)?;
        let file_size = meta.len();
        let zip64 = (file_size as f64) * 1.05 > ZIP64_LIMIT as f64;
        let (name_bytes, flags) = if name.is_ascii() { (name.as_bytes().to_vec(), 0u16) } else { (name.as_bytes().to_vec(), 0x800u16) };
        let method = self.method;
        let extract_version = if zip64 { ZIP64_VERSION } else { DEFAULT_VERSION };
        let header_offset = self.out.stream_position()?;

        // Local file header (sizes patched afterwards; seekable output -> no data descriptor).
        let mut h = Vec::with_capacity(30 + name_bytes.len() + 20);
        put32(&mut h, 0x0403_4b50);
        put16(&mut h, extract_version);
        put16(&mut h, flags);
        put16(&mut h, if method == Method::Deflated { 8 } else { 0 });
        put16(&mut h, dos_time);
        put16(&mut h, dos_date);
        put32(&mut h, 0); // crc
        if zip64 {
            put32(&mut h, 0xFFFF_FFFF);
            put32(&mut h, 0xFFFF_FFFF);
        } else {
            put32(&mut h, 0);
            put32(&mut h, 0);
        }
        put16(&mut h, name_bytes.len() as u16);
        put16(&mut h, if zip64 { 20 } else { 0 });
        h.extend_from_slice(&name_bytes);
        if zip64 {
            put16(&mut h, 0x0001);
            put16(&mut h, 16);
            put64(&mut h, 0);
            put64(&mut h, 0);
        }
        self.out.write_all(&h)?;
        let data_start = self.out.stream_position()?;

        let mut crc = flate2::Crc::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut total: u64 = 0;
        match method {
            Method::Stored => loop {
                let n = input.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                crc.update(&buf[..n]);
                total += n as u64;
                self.out.write_all(&buf[..n])?;
            },
            Method::Deflated => {
                let mut enc = flate2::write::DeflateEncoder::new(&mut self.out, flate2::Compression::new(6));
                loop {
                    let n = input.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    crc.update(&buf[..n]);
                    total += n as u64;
                    enc.write_all(&buf[..n])?;
                }
                enc.finish()?;
            }
        }
        let data_end = self.out.stream_position()?;
        let compress_size = data_end - data_start;
        if !zip64 && (total > ZIP64_LIMIT || compress_size > ZIP64_LIMIT) {
            // zipfile raises "File size too large, try using force_zip64" here.
            return Err(io::Error::other("file size changed while zipping"));
        }

        // Patch crc and sizes.
        self.out.seek(SeekFrom::Start(header_offset + 14))?;
        let mut p = Vec::with_capacity(12);
        put32(&mut p, crc.sum());
        if !zip64 {
            put32(&mut p, compress_size as u32);
            put32(&mut p, total as u32);
        }
        self.out.write_all(&p)?;
        if zip64 {
            self.out.seek(SeekFrom::Start(header_offset + 30 + name_bytes.len() as u64 + 4))?;
            let mut e = Vec::with_capacity(16);
            put64(&mut e, total);
            put64(&mut e, compress_size);
            self.out.write_all(&e)?;
        }
        self.out.seek(SeekFrom::Start(data_end))?;

        self.names.insert(name);
        self.entries.push(Entry {
            name: name_bytes,
            flags,
            method,
            dos_time,
            dos_date,
            crc: crc.sum(),
            compress_size,
            file_size: total,
            header_offset,
            external_attr: file_mode(&meta) << 16,
            extract_version,
        });
        Ok(())
    }

    /// Central directory + end records (`ZipFile._write_end_record`).
    pub fn finish(mut self) -> io::Result<File> {
        let cd_start = self.out.stream_position()?;
        for e in &self.entries {
            let mut extra = Vec::new();
            let mut fields: Vec<u64> = Vec::new();
            let file_size = if e.file_size > ZIP64_LIMIT {
                fields.push(e.file_size);
                0xFFFF_FFFFu32
            } else {
                e.file_size as u32
            };
            let compress_size = if e.compress_size > ZIP64_LIMIT {
                fields.push(e.compress_size);
                0xFFFF_FFFFu32
            } else {
                e.compress_size as u32
            };
            let header_offset = if e.header_offset > ZIP64_LIMIT {
                fields.push(e.header_offset);
                0xFFFF_FFFFu32
            } else {
                e.header_offset as u32
            };
            let mut extract_version = e.extract_version;
            if !fields.is_empty() {
                put16(&mut extra, 0x0001);
                put16(&mut extra, (8 * fields.len()) as u16);
                for f in &fields {
                    put64(&mut extra, *f);
                }
                extract_version = extract_version.max(ZIP64_VERSION);
            }
            let create_version = extract_version;
            let mut c = Vec::with_capacity(46 + e.name.len() + extra.len());
            put32(&mut c, 0x0201_4b50);
            put16(&mut c, create_version | (CREATE_SYSTEM_UNIX << 8));
            put16(&mut c, extract_version);
            put16(&mut c, e.flags);
            put16(&mut c, if e.method == Method::Deflated { 8 } else { 0 });
            put16(&mut c, e.dos_time);
            put16(&mut c, e.dos_date);
            put32(&mut c, e.crc);
            put32(&mut c, compress_size);
            put32(&mut c, file_size);
            put16(&mut c, e.name.len() as u16);
            put16(&mut c, extra.len() as u16);
            put16(&mut c, 0); // comment
            put16(&mut c, 0); // disk
            put16(&mut c, 0); // internal attr
            put32(&mut c, e.external_attr);
            put32(&mut c, header_offset);
            c.extend_from_slice(&e.name);
            c.extend_from_slice(&extra);
            self.out.write_all(&c)?;
        }
        let cd_end = self.out.stream_position()?;
        let count = self.entries.len();
        let cd_size = cd_end - cd_start;
        let mut r = Vec::new();
        let (mut cnt16, mut size32, mut off32) = (count as u64, cd_size, cd_start);
        if count > ZIP_FILECOUNT_LIMIT || cd_start > ZIP64_LIMIT || cd_size > ZIP64_LIMIT {
            // Zip64 end of central directory record + locator
            put32(&mut r, 0x0606_4b50);
            put64(&mut r, 44);
            put16(&mut r, ZIP64_VERSION);
            put16(&mut r, ZIP64_VERSION);
            put32(&mut r, 0);
            put32(&mut r, 0);
            put64(&mut r, count as u64);
            put64(&mut r, count as u64);
            put64(&mut r, cd_size);
            put64(&mut r, cd_start);
            put32(&mut r, 0x0706_4b50);
            put32(&mut r, 0);
            put64(&mut r, cd_end);
            put32(&mut r, 1);
            cnt16 = cnt16.min(0xFFFF);
            size32 = size32.min(0xFFFF_FFFF);
            off32 = off32.min(0xFFFF_FFFF);
        }
        put32(&mut r, 0x0605_4b50);
        put16(&mut r, 0);
        put16(&mut r, 0);
        put16(&mut r, cnt16 as u16);
        put16(&mut r, cnt16 as u16);
        put32(&mut r, size32 as u32);
        put32(&mut r, off32 as u32);
        put16(&mut r, 0);
        self.out.write_all(&r)?;
        self.out.flush()?;
        self.out.into_inner().map_err(|e| e.into_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normpath() {
        assert_eq!(normalize_arcname("originals/a.pdf"), "originals/a.pdf");
        assert_eq!(normalize_arcname("archive//x/./y.pdf"), "archive/x/y.pdf");
        assert_eq!(normalize_arcname("/abs/x"), "abs/x");
        assert_eq!(normalize_arcname("a/../b"), "b");
        assert_eq!(normalize_arcname("../b"), "../b");
    }

    #[test]
    fn roundtrip_structure() {
        let dir = std::env::temp_dir().join(format!("plfiles-zip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.txt");
        std::fs::write(&src, b"hello hello hello hello").unwrap();
        for method in [Method::Stored, Method::Deflated] {
            let out = dir.join(format!("out-{method:?}.zip"));
            let f = File::create(&out).unwrap();
            let mut z = ZipWriter::new(f, method, chrono_tz::UTC);
            z.write_file(&src, "originals/ä.txt").unwrap();
            assert!(z.contains("originals/ä.txt"));
            z.write_file(&src, "b.txt").unwrap();
            z.finish().unwrap();
            if let Ok(keep) = std::env::var("PLFILES_KEEP_ZIP") {
                // Manual cross-check with Python's zipfile (see docs/contract/pl-files.md).
                let _ = std::fs::copy(&out, format!("{keep}-{method:?}.zip"));
            }
            let bytes = std::fs::read(&out).unwrap();
            assert_eq!(&bytes[0..4], &[0x50, 0x4b, 0x03, 0x04]);
            let eocd = &bytes[bytes.len() - 22..];
            assert_eq!(&eocd[0..4], &[0x50, 0x4b, 0x05, 0x06]);
            assert_eq!(u16::from_le_bytes([eocd[10], eocd[11]]), 2);
            // crc of first entry patched
            let crc = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
            let mut c = flate2::Crc::new();
            c.update(b"hello hello hello hello");
            assert_eq!(crc, c.sum());
            if method == Method::Stored {
                let size = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
                assert_eq!(size, 23);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
