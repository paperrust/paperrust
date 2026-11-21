//! Minimal, read-only ZIP access for paperless-ngx exports (`document_exporter --zip`).
//!
//! Supported: stored (0) and deflated (8), ZIP64 (sizes/offsets, EOCD64). Other methods
//! (bzip2/lzma/zstd, upstream `--zip-compression`) → error with a hint. Nothing is unpacked:
//! entries are streamed directly from the file (checksums, manifest.json).

use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD64_LOC_SIG: u32 = 0x0706_4b50;
const EOCD64_SIG: u32 = 0x0606_4b50;
const CD_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
/// Upper limit for the central directory in memory.
const MAX_CENTRAL_DIR: u64 = 256 << 20;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub compressed: u64,
    /// unpacked size
    pub size: u64,
    header_offset: u64,
}

#[derive(Debug)]
pub struct ZipIndex {
    path: PathBuf,
    entries: Vec<Entry>,
    by_name: HashMap<String, usize>,
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

/// Quick test: does the file begin with a ZIP signature?
pub fn looks_like_zip(path: &Path) -> bool {
    let mut b = [0u8; 4];
    File::open(path).and_then(|mut f| f.read_exact(&mut b)).is_ok() && (u32le(&b, 0) == LOCAL_SIG || u32le(&b, 0) == EOCD_SIG)
}

impl ZipIndex {
    pub fn open(path: &Path) -> Result<ZipIndex> {
        let mut f = File::open(path).with_context(|| format!("{} lässt sich nicht öffnen", path.display()))?;
        let len = f.metadata()?.len();
        if len < 22 {
            bail!("{} ist keine ZIP-Datei (zu klein)", path.display());
        }
        let tail_len = len.min(65_535 + 22);
        f.seek(SeekFrom::Start(len - tail_len))?;
        let mut tail = vec![0u8; tail_len as usize];
        f.read_exact(&mut tail)?;
        let pos = (0..=tail.len() - 22)
            .rev()
            .find(|&i| u32le(&tail, i) == EOCD_SIG)
            .ok_or_else(|| anyhow!("{}: kein ZIP-Endeintrag gefunden", path.display()))?;
        let eocd_abs = len - tail_len + pos as u64;
        let mut count = u64::from(u16le(&tail, pos + 10));
        let mut cd_size = u64::from(u32le(&tail, pos + 12));
        let mut cd_off = u64::from(u32le(&tail, pos + 16));
        if (count == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_off == 0xFFFF_FFFF) && eocd_abs >= 20 {
            // ZIP64: locator sits directly before the EOCD
            let mut loc = [0u8; 20];
            f.seek(SeekFrom::Start(eocd_abs - 20))?;
            f.read_exact(&mut loc)?;
            if u32le(&loc, 0) == EOCD64_LOC_SIG {
                let mut e = [0u8; 56];
                f.seek(SeekFrom::Start(u64le(&loc, 8)))?;
                f.read_exact(&mut e)?;
                if u32le(&e, 0) != EOCD64_SIG {
                    bail!("{}: ZIP64-Endeintrag beschädigt", path.display());
                }
                count = u64le(&e, 32);
                cd_size = u64le(&e, 40);
                cd_off = u64le(&e, 48);
            }
        }
        if cd_size > MAX_CENTRAL_DIR || cd_off.saturating_add(cd_size) > len {
            bail!("{}: Zentralverzeichnis ungültig oder zu groß", path.display());
        }
        f.seek(SeekFrom::Start(cd_off))?;
        let mut cd = vec![0u8; cd_size as usize];
        f.read_exact(&mut cd)?;
        let mut entries = Vec::with_capacity(count.min(1 << 20) as usize);
        let mut i = 0usize;
        while i + 46 <= cd.len() && u32le(&cd, i) == CD_SIG {
            let flags = u16le(&cd, i + 8);
            let method = u16le(&cd, i + 10);
            let mut compressed = u64::from(u32le(&cd, i + 20));
            let mut size = u64::from(u32le(&cd, i + 24));
            let nlen = u16le(&cd, i + 28) as usize;
            let xlen = u16le(&cd, i + 30) as usize;
            let clen = u16le(&cd, i + 32) as usize;
            let mut header_offset = u64::from(u32le(&cd, i + 42));
            let end = i + 46 + nlen + xlen + clen;
            if end > cd.len() {
                bail!("{}: Zentralverzeichnis abgeschnitten", path.display());
            }
            let raw_name = &cd[i + 46..i + 46 + nlen];
            // Bit 11 = UTF-8; otherwise CP437 – exports from Python write ASCII or UTF-8
            let name = if flags & 0x800 != 0 {
                String::from_utf8_lossy(raw_name).into_owned()
            } else {
                raw_name.iter().map(|&b| b as char).collect()
            };
            // ZIP64 extra field (0x0001): only the values that are 0xFFFFFFFF in the fixed part
            let mut x = i + 46 + nlen;
            let xend = x + xlen;
            while x + 4 <= xend {
                let id = u16le(&cd, x);
                let sz = u16le(&cd, x + 2) as usize;
                let mut p = x + 4;
                if id == 1 {
                    let take = |p: &mut usize| -> Option<u64> {
                        (*p + 8 <= x + 4 + sz).then(|| {
                            let v = u64le(&cd, *p);
                            *p += 8;
                            v
                        })
                    };
                    if size == 0xFFFF_FFFF {
                        size = take(&mut p).unwrap_or(size);
                    }
                    if compressed == 0xFFFF_FFFF {
                        compressed = take(&mut p).unwrap_or(compressed);
                    }
                    if header_offset == 0xFFFF_FFFF {
                        header_offset = take(&mut p).unwrap_or(header_offset);
                    }
                }
                x += 4 + sz;
            }
            entries.push(Entry { name, method, compressed, size, header_offset });
            i = end;
        }
        let by_name = entries.iter().enumerate().map(|(i, e)| (normalize(&e.name), i)).collect();
        Ok(ZipIndex { path: path.to_path_buf(), entries, by_name })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.by_name.get(&normalize(name)).map(|&i| &self.entries[i])
    }

    /// Entry as a data stream (unpacked).
    pub fn open_entry(&self, name: &str) -> Result<Box<dyn Read>> {
        let e = self.get(name).ok_or_else(|| anyhow!("{name} fehlt im ZIP"))?;
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(e.header_offset))?;
        let mut h = [0u8; 30];
        f.read_exact(&mut h)?;
        if u32le(&h, 0) != LOCAL_SIG {
            bail!("{name}: lokaler ZIP-Kopf beschädigt");
        }
        let skip = u64::from(u16le(&h, 26)) + u64::from(u16le(&h, 28));
        f.seek(SeekFrom::Current(skip as i64))?;
        let raw = BufReader::with_capacity(1 << 16, f).take(e.compressed);
        Ok(match e.method {
            0 => Box::new(raw),
            // never unpacks more than stated (protection against zip bombs)
            8 => Box::new(flate2::read::DeflateDecoder::new(raw).take(e.size)),
            m => bail!(
                "{name}: ZIP-Verfahren {m} wird nicht unterstützt (nur stored/deflated) – Export ohne --zip-compression erstellen oder entpacken"
            ),
        })
    }
}

/// Normalize path separators, drop leading `./`.
pub fn normalize(name: &str) -> String {
    let n = name.replace('\\', "/");
    n.strip_prefix("./").unwrap_or(&n).to_string()
}

#[cfg(test)]
pub mod testzip {
    //! Small ZIP writer for tests only (stored + deflated).
    use std::io::Write;

    pub fn build(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, data, deflate) in files {
            let crc = crc32(data);
            let body = if *deflate {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(data).unwrap();
                e.finish().unwrap()
            } else {
                data.to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let off = out.len() as u32;
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0x800u16.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&body);
            cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            cd.extend_from_slice(&20u16.to_le_bytes());
            cd.extend_from_slice(&20u16.to_le_bytes());
            cd.extend_from_slice(&0x800u16.to_le_bytes());
            cd.extend_from_slice(&method.to_le_bytes());
            cd.extend_from_slice(&[0, 0, 0, 0]);
            cd.extend_from_slice(&crc.to_le_bytes());
            cd.extend_from_slice(&(body.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0; 12]);
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&cd);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn crc32(d: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in d {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
        }
        !c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_stored_and_deflated() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.zip");
        let big = vec![b'a'; 100_000];
        std::fs::write(&p, testzip::build(&[("manifest.json", b"[]", false), ("originals/a.pdf", &big, true)])).unwrap();
        assert!(looks_like_zip(&p));
        let z = ZipIndex::open(&p).unwrap();
        assert_eq!(z.entries().len(), 2);
        let mut s = String::new();
        z.open_entry("manifest.json").unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "[]");
        let mut v = Vec::new();
        z.open_entry("./originals\\a.pdf").unwrap().read_to_end(&mut v).unwrap();
        assert_eq!(v, big);
        assert!(z.open_entry("nope").is_err());
        let not = dir.path().join("n.zip");
        std::fs::write(&not, b"hello world, not a zip at all....").unwrap();
        assert!(!looks_like_zip(&not));
        assert!(ZipIndex::open(&not).is_err());
    }
}
