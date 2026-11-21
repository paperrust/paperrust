//! Minimal, hardened ZIP reader (stored + deflate) for office containers.
//!
//! Protection against zip bombs: limits for entry count, unpacked size per entry and in total,
//! and compression ratio; counting happens during the actual unpacking (not based on the
//! information in the directory, which can be forged).

use std::cell::Cell;
use std::io::Read;

use crate::{Limits, ParseError, Result};

#[derive(Debug, Clone)]
pub struct ZipEntry {
    pub name: String,
    pub method: u16,
    pub flags: u16,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub local_header_offset: u64,
}

pub struct ZipArchive<'a> {
    data: &'a [u8],
    pub entries: Vec<ZipEntry>,
    limits: Limits,
    used: Cell<u64>,
}

fn u16le(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn u32le(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn u64le(d: &[u8], o: usize) -> Option<u64> {
    d.get(o..o + 8).map(|b| {
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        u64::from_le_bytes(a)
    })
}

fn bad(msg: &str) -> ParseError {
    ParseError::Invalid(format!("zip: {msg}"))
}

impl<'a> ZipArchive<'a> {
    pub fn open(data: &'a [u8], limits: &Limits) -> Result<Self> {
        if data.len() < 22 {
            return Err(bad("too short"));
        }
        // End of central directory: search in the last 64 KiB + 22 bytes
        let search_start = data.len().saturating_sub(22 + 65_535);
        let mut eocd = None;
        let mut i = data.len() - 22;
        loop {
            if &data[i..i + 4] == b"PK\x05\x06" {
                eocd = Some(i);
                break;
            }
            if i == search_start {
                break;
            }
            i -= 1;
        }
        let eocd = eocd.ok_or_else(|| bad("no end of central directory"))?;
        let mut count = u16le(data, eocd + 10).ok_or_else(|| bad("eocd"))? as u64;
        let mut cd_size = u32le(data, eocd + 12).ok_or_else(|| bad("eocd"))? as u64;
        let mut cd_off = u32le(data, eocd + 16).ok_or_else(|| bad("eocd"))? as u64;
        if count == 0xffff || cd_size == 0xffff_ffff || cd_off == 0xffff_ffff {
            // ZIP64 locator directly before the EOCD
            if eocd >= 20 && &data[eocd - 20..eocd - 16] == b"PK\x06\x07" {
                let z64 = u64le(data, eocd - 12).ok_or_else(|| bad("zip64 locator"))? as usize;
                if data.get(z64..z64 + 4) != Some(b"PK\x06\x06") {
                    return Err(bad("zip64 eocd"));
                }
                count = u64le(data, z64 + 32).ok_or_else(|| bad("zip64 eocd"))?;
                cd_size = u64le(data, z64 + 40).ok_or_else(|| bad("zip64 eocd"))?;
                cd_off = u64le(data, z64 + 48).ok_or_else(|| bad("zip64 eocd"))?;
            }
        }
        if count > limits.max_zip_entries as u64 {
            return Err(ParseError::LimitExceeded(format!("zip entries {count}")));
        }
        let cd_start = usize::try_from(cd_off).map_err(|_| bad("cd offset"))?;
        let cd_end = cd_start.checked_add(usize::try_from(cd_size).map_err(|_| bad("cd size"))?).ok_or_else(|| bad("cd size"))?;
        if cd_end > data.len() {
            return Err(bad("central directory outside file"));
        }
        let mut entries = Vec::with_capacity(count as usize);
        let mut p = cd_start;
        while p + 46 <= cd_end && entries.len() < count as usize {
            if &data[p..p + 4] != b"PK\x01\x02" {
                return Err(bad("central directory entry"));
            }
            let flags = u16le(data, p + 8).unwrap_or(0);
            let method = u16le(data, p + 10).unwrap_or(0);
            let mut csize = u32le(data, p + 20).unwrap_or(0) as u64;
            let mut usize_ = u32le(data, p + 24).unwrap_or(0) as u64;
            let nlen = u16le(data, p + 28).unwrap_or(0) as usize;
            let elen = u16le(data, p + 30).unwrap_or(0) as usize;
            let clen = u16le(data, p + 32).unwrap_or(0) as usize;
            let mut lho = u32le(data, p + 42).unwrap_or(0) as u64;
            let name_bytes = data.get(p + 46..p + 46 + nlen).ok_or_else(|| bad("name"))?;
            let name = if flags & 0x800 != 0 {
                String::from_utf8_lossy(name_bytes).into_owned()
            } else {
                // CP437 is irrelevant for office names (ASCII); try UTF-8
                String::from_utf8_lossy(name_bytes).into_owned()
            };
            // ZIP64 extra field
            if let Some(extra) = data.get(p + 46 + nlen..p + 46 + nlen + elen) {
                let mut q = 0;
                while q + 4 <= extra.len() {
                    let id = u16le(extra, q).unwrap_or(0);
                    let sz = u16le(extra, q + 2).unwrap_or(0) as usize;
                    if id == 0x0001 {
                        let mut r = q + 4;
                        if usize_ == 0xffff_ffff {
                            usize_ = u64le(extra, r).unwrap_or(usize_);
                            r += 8;
                        }
                        if csize == 0xffff_ffff {
                            csize = u64le(extra, r).unwrap_or(csize);
                            r += 8;
                        }
                        if lho == 0xffff_ffff {
                            lho = u64le(extra, r).unwrap_or(lho);
                        }
                    }
                    q += 4 + sz;
                }
            }
            entries.push(ZipEntry { name, method, flags, compressed_size: csize, uncompressed_size: usize_, local_header_offset: lho });
            p += 46 + nlen + elen + clen;
        }
        Ok(Self { data, entries, limits: limits.clone(), used: Cell::new(0) })
    }

    pub fn by_name(&self, name: &str) -> Option<&ZipEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    pub fn has(&self, name: &str) -> bool {
        self.by_name(name).is_some()
    }

    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let e = self.by_name(name).ok_or_else(|| bad(&format!("missing entry {name}")))?;
        self.read_entry(e)
    }

    pub fn read_opt(&self, name: &str) -> Result<Option<Vec<u8>>> {
        match self.by_name(name) {
            Some(e) => self.read_entry(e).map(Some),
            None => Ok(None),
        }
    }

    pub fn read_entry(&self, e: &ZipEntry) -> Result<Vec<u8>> {
        if e.flags & 1 != 0 {
            return Err(ParseError::Encrypted);
        }
        let lho = usize::try_from(e.local_header_offset).map_err(|_| bad("offset"))?;
        if self.data.get(lho..lho + 4) != Some(b"PK\x03\x04") {
            return Err(bad("local header"));
        }
        let nlen = u16le(self.data, lho + 26).ok_or_else(|| bad("local header"))? as usize;
        let elen = u16le(self.data, lho + 28).ok_or_else(|| bad("local header"))? as usize;
        let start = lho + 30 + nlen + elen;
        let csize = usize::try_from(e.compressed_size).map_err(|_| bad("size"))?;
        let end = start.checked_add(csize).ok_or_else(|| bad("size"))?;
        let comp = self.data.get(start..end.min(self.data.len())).ok_or_else(|| bad("entry data outside file"))?;
        if end > self.data.len() {
            return Err(bad("truncated entry"));
        }
        let remaining_total = self.limits.max_total_uncompressed.saturating_sub(self.used.get());
        let max = self.limits.max_entry_size.min(remaining_total);
        let out = match e.method {
            0 => {
                if comp.len() as u64 > max {
                    return Err(ParseError::LimitExceeded(format!("zip entry {} too large", e.name)));
                }
                comp.to_vec()
            }
            8 => {
                let mut dec = flate2::read::DeflateDecoder::new(comp).take(max + 1);
                let mut out = Vec::with_capacity((e.uncompressed_size.min(max) as usize).min(64 << 20));
                dec.read_to_end(&mut out).map_err(|err| bad(&format!("inflate {}: {err}", e.name)))?;
                if out.len() as u64 > max {
                    return Err(ParseError::LimitExceeded(format!("zip entry {} decompresses beyond limit", e.name)));
                }
                out
            }
            m => {
                return Err(ParseError::Unsupported(format!("zip compression method {m}")));
            }
        };
        let n = out.len() as u64;
        if n >= self.limits.ratio_check_min && n / (comp.len() as u64).max(1) > self.limits.max_compression_ratio {
            return Err(ParseError::LimitExceeded(format!("zip entry {} compression ratio too high", e.name)));
        }
        self.used.set(self.used.get() + n);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored_zip(name: &str, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        let crc = 0u32;
        v.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        v.extend_from_slice(&crc.to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(data);
        let cd = v.len();
        v.extend_from_slice(b"PK\x01\x02\x14\x00\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        v.extend_from_slice(&crc.to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&[0u8; 12]);
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(name.as_bytes());
        let cd_size = v.len() - cd;
        v.extend_from_slice(b"PK\x05\x06\x00\x00\x00\x00\x01\x00\x01\x00");
        v.extend_from_slice(&(cd_size as u32).to_le_bytes());
        v.extend_from_slice(&(cd as u32).to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v
    }

    #[test]
    fn read_stored() {
        let z = stored_zip("a.txt", b"hallo");
        let a = ZipArchive::open(&z, &Limits::default()).unwrap();
        assert_eq!(a.read("a.txt").unwrap(), b"hallo");
        assert!(a.read("b").is_err());
    }

    #[test]
    fn entry_limit() {
        let z = stored_zip("a.txt", b"hallo welt");
        let l = Limits { max_entry_size: 4, ..Limits::default() };
        let a = ZipArchive::open(&z, &l).unwrap();
        assert!(matches!(a.read("a.txt"), Err(ParseError::LimitExceeded(_))));
    }

    #[test]
    fn garbage() {
        assert!(ZipArchive::open(b"not a zip at all, definitely not", &Limits::default()).is_err());
    }
}
