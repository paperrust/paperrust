//! Detection without a database: area of a file in the media folder, suspicious extensions and
//! file names (ransom notes), content check (magic bytes + Shannon entropy).

use std::io::Read;
use std::path::Path;

/// Where is a file relative to `MEDIA_ROOT`?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// `documents/originals/…`
    Originals,
    /// `documents/archive/…`
    Archive,
    /// `documents/thumbnails/…`
    Thumbnails,
    /// everything else (app logo, `media.lock`, folders of third-party programs)
    Other,
}

/// Area and path relative to the area (with `/`), e.g. `("2024/Rechnung.pdf")`.
pub fn area(rel: &str) -> (Area, &str) {
    let rel = rel.trim_start_matches('/');
    for (prefix, a) in [("documents/originals/", Area::Originals), ("documents/archive/", Area::Archive), ("documents/thumbnails/", Area::Thumbnails)] {
        if let Some(rest) = rel.strip_prefix(prefix) {
            return (a, rest);
        }
    }
    (Area::Other, rel)
}

/// Extensions of known ransomware programs and generic "encrypted" extensions.
pub const SUSPICIOUS_EXTENSIONS: &[&str] = &[
    "locked", "lock", "encrypted", "enc", "crypt", "crypted", "crypto", "cry", "crypz", "cryp1", "locky", "zepto", "odin", "thor", "aesir", "zzzzz",
    "cerber", "cerber2", "cerber3", "wnry", "wncry", "wcry", "wncryt", "ryk", "ryuk", "conti", "lockbit", "akira", "blackcat", "alphv", "royal",
    "basta", "hive", "clop", "phobos", "dharma", "stop", "djvu", "makop", "medusa", "rhysida", "play", "ransom", "pay", "paym", "payms", "kraken",
    "crab", "gdcb", "krab", "micro", "ecc", "ezz", "exx", "xyz", "vvv", "ttt", "abc", "aaa", "zzz", "xxx", "r5a", "rdm", "rrk", "encr", "enciphered",
];

/// Known names of ransom notes (lower case, substring suffices).
const RANSOM_NOTE_MARKERS: &[&str] = &[
    "readme_decrypt", "decrypt_instruction", "how_to_decrypt", "how-to-decrypt", "how_to_restore", "restore_files", "restore-my-files", "recover_files",
    "your_files", "help_decrypt", "_readme.txt", "!readme", "read_me_now", "ransom", "decrypt-files", "files_encrypted", "!!!_read", "#decrypt",
];

/// Allowed extensions per area (lower case). Originals: everything PaperRust can ingest.
const ORIGINAL_EXTENSIONS: &[&str] = &[
    "pdf", "png", "jpg", "jpeg", "jpe", "jfif", "tif", "tiff", "gif", "webp", "bmp", "heic", "heif", "avif", "jxl", "txt", "text", "csv", "md", "eml",
    "msg", "html", "htm", "xml", "json", "doc", "docx", "docm", "dot", "dotx", "odt", "ott", "ods", "ots", "odp", "otp", "odg", "xls", "xlsx", "xlsm",
    "ppt", "pptx", "pptm", "rtf", "wpd", "pages", "numbers", "key", "epub", "zip", "ps", "eps", "svg", "dcm", "pdfa",
];
const ARCHIVE_EXTENSIONS: &[&str] = &["pdf"];
const THUMBNAIL_EXTENSIONS: &[&str] = &["webp", "png", "jpg", "jpeg"];

fn ext_of(name: &str) -> Option<String> {
    let base = name.rsplit('/').next().unwrap_or(name);
    let (stem, ext) = base.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// Reason why a name is suspicious.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameVerdict {
    Ok,
    /// Extension from [`SUSPICIOUS_EXTENSIONS`] or from the additional list.
    SuspiciousExtension(String),
    /// Extension that does not occur in this area (PaperRust never creates it).
    UnknownExtension(String),
    /// Name of a ransom note.
    RansomNote,
}

/// Evaluate the name of a (new or changed) file. `extra` = additional suspicious extensions.
pub fn check_name(a: Area, rel_in_area: &str, extra: &[String]) -> NameVerdict {
    let base = rel_in_area.rsplit('/').next().unwrap_or(rel_in_area).to_ascii_lowercase();
    if RANSOM_NOTE_MARKERS.iter().any(|m| base.contains(m)) {
        return NameVerdict::RansomNote;
    }
    let Some(ext) = ext_of(&base) else {
        return match a {
            Area::Other => NameVerdict::Ok,
            _ => NameVerdict::UnknownExtension(String::new()),
        };
    };
    if SUSPICIOUS_EXTENSIONS.contains(&ext.as_str()) || extra.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
        // Exception: real originals may have any extension from the allow list (e.g. "key").
        if !(a == Area::Originals && ORIGINAL_EXTENSIONS.contains(&ext.as_str()) && !extra.iter().any(|e| e.eq_ignore_ascii_case(&ext))) {
            return NameVerdict::SuspiciousExtension(ext);
        }
    }
    let allowed: &[&str] = match a {
        Area::Originals => ORIGINAL_EXTENSIONS,
        Area::Archive => ARCHIVE_EXTENSIONS,
        Area::Thumbnails => THUMBNAIL_EXTENSIONS,
        Area::Other => return NameVerdict::Ok,
    };
    if allowed.contains(&ext.as_str()) {
        NameVerdict::Ok
    } else {
        NameVerdict::UnknownExtension(ext)
    }
}

/// Shannon entropy in bits per byte (0..=8).
pub fn entropy(buf: &[u8]) -> f64 {
    if buf.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for &b in buf {
        counts[b as usize] += 1;
    }
    let n = buf.len() as f64;
    counts.iter().filter(|&&c| c > 0).map(|&c| {
        let p = c as f64 / n;
        -p * p.log2()
    }).sum()
}

/// Result of the content check.
#[derive(Debug, Clone, PartialEq)]
pub enum ContentVerdict {
    Ok,
    /// Header does not match the extension (e.g. `.pdf` without `%PDF`).
    BadMagic { expected: &'static str, entropy: f64 },
    /// Header does not match and content looks random (encrypted).
    Encrypted { entropy: f64 },
}

impl ContentVerdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, ContentVerdict::Ok)
    }
}

/// From this entropy (bits/byte, over up to 64 KiB) content without a matching header counts as encrypted.
pub const ENTROPY_ENCRYPTED: f64 = 7.5;

fn magic_ok(ext: &str, head: &[u8]) -> Option<(bool, &'static str)> {
    let starts = |m: &[u8]| head.starts_with(m);
    Some(match ext {
        // PDF: `%PDF` must be within the first 1024 bytes according to the specification
        "pdf" | "pdfa" => (head.len() >= 4 && head[..head.len().min(1024)].windows(4).any(|w| w == b"%PDF"), "%PDF"),
        "png" => (starts(b"\x89PNG\r\n\x1a\n"), "PNG"),
        "jpg" | "jpeg" | "jpe" | "jfif" => (starts(&[0xFF, 0xD8, 0xFF]), "JPEG"),
        "gif" => (starts(b"GIF87a") || starts(b"GIF89a"), "GIF"),
        "webp" => (head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP", "WEBP"),
        "tif" | "tiff" => (starts(b"II*\0") || starts(b"MM\0*") || starts(b"II+\0") || starts(b"MM\0+"), "TIFF"),
        "bmp" => (starts(b"BM"), "BMP"),
        "docx" | "docm" | "dotx" | "xlsx" | "xlsm" | "pptx" | "pptm" | "odt" | "ott" | "ods" | "ots" | "odp" | "otp" | "odg" | "epub" | "zip" | "pages"
        | "numbers" | "key" => (starts(b"PK\x03\x04") || starts(b"PK\x05\x06"), "ZIP"),
        "doc" | "xls" | "ppt" | "msg" | "dot" => (starts(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]), "OLE"),
        "rtf" => (starts(b"{\\rtf"), "RTF"),
        _ => return None,
    })
}

/// Text-like extensions: no fixed header, but the entropy of normal texts is well below 6.
fn texty(ext: &str) -> bool {
    matches!(ext, "txt" | "text" | "csv" | "md" | "eml" | "html" | "htm" | "xml" | "json" | "svg" | "ps" | "eps")
}

/// Check the content (start of a file) against the extension. Empty files count as fine.
pub fn check_content(name: &str, head: &[u8]) -> ContentVerdict {
    if head.is_empty() {
        return ContentVerdict::Ok;
    }
    let e = entropy(head);
    let Some(ext) = ext_of(name) else {
        return if e >= ENTROPY_ENCRYPTED && head.len() >= 1024 { ContentVerdict::Encrypted { entropy: e } } else { ContentVerdict::Ok };
    };
    match magic_ok(&ext, head) {
        Some((true, _)) => ContentVerdict::Ok,
        Some((false, expected)) => {
            if e >= ENTROPY_ENCRYPTED {
                ContentVerdict::Encrypted { entropy: e }
            } else {
                ContentVerdict::BadMagic { expected, entropy: e }
            }
        }
        None if texty(&ext) => {
            // UTF-16/UTF-8 text has at most ~5–6 bits/byte; encrypted data ~8.
            if e >= ENTROPY_ENCRYPTED && head.len() >= 1024 { ContentVerdict::Encrypted { entropy: e } } else { ContentVerdict::Ok }
        }
        None => {
            if e >= 7.9 && head.len() >= 4096 { ContentVerdict::Encrypted { entropy: e } } else { ContentVerdict::Ok }
        }
    }
}

/// Read the first 64 KiB of a file.
pub fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(64 * 1024);
    f.take(64 * 1024).read_to_end(&mut buf)?;
    Ok(buf)
}

/// System files that operating systems and NAS programs create (never suspicious).
pub fn is_system_file(rel: &str) -> bool {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    let lower = base.to_ascii_lowercase();
    matches!(lower.as_str(), "thumbs.db" | "desktop.ini" | ".ds_store" | "media.lock" | ".directory")
        || lower.starts_with("._")
        || rel.contains("@eaDir/")
        || rel.starts_with("@eaDir")
        || rel.contains("#recycle/")
        || rel.contains("/.snapshot")
        || lower.ends_with(".paperrust-tmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas() {
        assert_eq!(area("documents/originals/2024/a.pdf"), (Area::Originals, "2024/a.pdf"));
        assert_eq!(area("/documents/archive/a.pdf"), (Area::Archive, "a.pdf"));
        assert_eq!(area("documents/thumbnails/0000001.webp"), (Area::Thumbnails, "0000001.webp"));
        assert_eq!(area("logo/x.png").0, Area::Other);
    }

    #[test]
    fn names() {
        assert_eq!(check_name(Area::Originals, "a.pdf", &[]), NameVerdict::Ok);
        assert_eq!(check_name(Area::Originals, "a.pdf.locked", &[]), NameVerdict::SuspiciousExtension("locked".into()));
        assert_eq!(check_name(Area::Archive, "a.PDF.ENCRYPTED", &[]), NameVerdict::SuspiciousExtension("encrypted".into()));
        assert_eq!(check_name(Area::Archive, "a.png", &[]), NameVerdict::UnknownExtension("png".into()));
        assert_eq!(check_name(Area::Originals, "a.qwerty1", &[]), NameVerdict::UnknownExtension("qwerty1".into()));
        assert_eq!(check_name(Area::Originals, "a.qwerty1", &["QWERTY1".into()]), NameVerdict::SuspiciousExtension("qwerty1".into()));
        assert_eq!(check_name(Area::Originals, "2024/README_DECRYPT.txt", &[]), NameVerdict::RansomNote);
        assert_eq!(check_name(Area::Other, "logo/a.svg", &[]), NameVerdict::Ok);
        // Keynote files are real originals
        assert_eq!(check_name(Area::Originals, "talk.key", &[]), NameVerdict::Ok);
        assert_eq!(check_name(Area::Thumbnails, "0000001.webp", &[]), NameVerdict::Ok);
    }

    fn random(n: usize) -> Vec<u8> {
        // xorshift: deterministisch, gleichverteilt
        let mut x: u64 = 0x9E3779B97F4A7C15;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn entropy_bounds() {
        assert_eq!(entropy(&[]), 0.0);
        assert_eq!(entropy(&[7; 100]), 0.0);
        assert!(entropy(&random(65536)) > 7.9);
        assert!(entropy("Sehr geehrte Damen und Herren, anbei die Rechnung.".repeat(200).as_bytes()) < 5.0);
    }

    #[test]
    fn contents() {
        let mut pdf = b"%PDF-1.7\n".to_vec();
        pdf.extend(random(60000));
        assert!(check_content("a.pdf", &pdf).is_ok(), "komprimierte PDF-Ströme sind ok");
        assert!(matches!(check_content("a.pdf", &random(60000)), ContentVerdict::Encrypted { .. }));
        assert!(matches!(check_content("a.pdf", b"hello world, not a pdf"), ContentVerdict::BadMagic { expected: "%PDF", .. }));
        assert!(check_content("a.txt", "Hallo Welt\n".repeat(500).as_bytes()).is_ok());
        assert!(matches!(check_content("a.txt", &random(8000)), ContentVerdict::Encrypted { .. }));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
        webp.extend(random(2000));
        assert!(check_content("0000001.webp", &webp).is_ok());
        assert!(!check_content("0000001.webp", &random(2000)).is_ok());
        assert!(check_content("empty.pdf", b"").is_ok());
    }

    #[test]
    fn system_files() {
        assert!(is_system_file("documents/originals/Thumbs.db"));
        assert!(is_system_file("documents/originals/@eaDir/x.pdf@SynoEAStream"));
        assert!(is_system_file("media.lock"));
        assert!(!is_system_file("documents/originals/a.pdf"));
    }
}
