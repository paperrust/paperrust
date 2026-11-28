//! Model files: fixed sources with SHA-256, lookup in the model directories, verified installation.
//!
//! Rules (docs/OCR-NEXT.md §2.2): fixed source, fixed SHA-256, stored in the data folder (kept
//! across updates), never downloaded without a setting or a click, always overridable with
//! `PAPERRUST_OCR_MODEL_DIR` (servers without internet put the files there themselves). The
//! Docker image contains the base package (see `docker/Dockerfile`, same URLs and checksums).
//!
//! The download itself is left to the caller ([`install`] takes a fetch function), so this crate
//! needs no HTTP client: the server uses its `reqwest`, the desktop app its own.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// Group of a model file (decides whether it ships in the base package).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Base package: in the Docker image, desktop "Bessere Texterkennung herunterladen".
    Base,
    /// Only on explicit choice (bigger/slower, or other scripts).
    OnDemand,
}

/// A model file with its fixed source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    /// File name in the model directory.
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    pub group: Group,
    /// Licence of the weights.
    pub license: &'static str,
}

/// PP-OCRv6 small detector (RapidOCR model zoo v3.10.0, Apache-2.0).
pub const DET_SMALL: ModelFile = ModelFile {
    file: "PP-OCRv6_det_small.onnx",
    url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.10.0/onnx/PP-OCRv6/det/PP-OCRv6_det_small.onnx",
    sha256: "090f04abcd9d9a7498bc4ebf677e4cb9bdce1fe4197ddb7e529f1ef44e1ff94f",
    size: 9_929_594,
    group: Group::Base,
    license: "Apache-2.0",
};
/// PP-OCRv6 small recognizer (multilingual incl. Latin, character list in the ONNX metadata).
pub const REC_SMALL: ModelFile = ModelFile {
    file: "PP-OCRv6_rec_small.onnx",
    url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.10.0/onnx/PP-OCRv6/rec/PP-OCRv6_rec_small.onnx",
    sha256: "6f327246b50388f3c176ae304bd95767ea6dc0c9ae92153ef8cbe210b3c14884",
    size: 21_234_383,
    group: Group::Base,
    license: "Apache-2.0",
};
/// PP-OCRv6 tiny recognizer: used by the `fast` variant (small detector + tiny recognizer).
pub const REC_TINY: ModelFile = ModelFile {
    file: "PP-OCRv6_rec_tiny.onnx",
    url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.10.0/onnx/PP-OCRv6/rec/PP-OCRv6_rec_tiny.onnx",
    sha256: "e16e242de5937ad92609223f19bc2aff3727ee40b095f996907c24749bad251b",
    size: 4_489_813,
    group: Group::OnDemand,
    license: "Apache-2.0",
};
pub const DET_MEDIUM: ModelFile = ModelFile {
    file: "PP-OCRv6_det_medium.onnx",
    url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.10.0/onnx/PP-OCRv6/det/PP-OCRv6_det_medium.onnx",
    sha256: "92078b7355007ccfffcd4c8cd441a3afd4538904d06881b29a155e1e679907c2",
    size: 62_119_454,
    group: Group::OnDemand,
    license: "Apache-2.0",
};
pub const REC_MEDIUM: ModelFile = ModelFile {
    file: "PP-OCRv6_rec_medium.onnx",
    url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.10.0/onnx/PP-OCRv6/rec/PP-OCRv6_rec_medium.onnx",
    sha256: "eef444829dbbe18d7fea59a3f6eb75647518d2b3a9568d27c92e42940204894b",
    size: 76_629_984,
    group: Group::OnDemand,
    license: "Apache-2.0",
};
/// Page orientation PP-LCNet_x1_0_doc_ori (PaddleX weights, Apache-2.0; ONNX export published
/// by the oar-ocr project, release v0.3.0).
pub const ORIENT_PPLCNET: ModelFile = ModelFile {
    file: "pp-lcnet_x1_0_doc_ori.onnx",
    url: "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0/pp-lcnet_x1_0_doc_ori.onnx",
    sha256: "bbcd6c2b43ab15d2e605455aef2cd280ab87b570824522d24a69e9298875a1ac",
    size: 6_787_248,
    group: Group::Base,
    license: "Apache-2.0",
};
/// Page orientation docTR/OnnxTR MobileNetV3-small (Apache-2.0); compared in stage 1, not shipped.
pub const ORIENT_DOCTR: ModelFile = ModelFile {
    file: "mobilenet_v3_small_page_orientation-60606ce4.onnx",
    url: "https://github.com/felixdittrich92/OnnxTR/releases/download/v0.6.0/mobilenet_v3_small_page_orientation-60606ce4.onnx",
    sha256: "60606ce4e1796327717ba546ff23106208893a26c90a4c4bfcfebad0f2b1aa67",
    size: 6_097_456,
    group: Group::OnDemand,
    license: "Apache-2.0",
};
/// DE·HTR v2 (German handwriting lines, int8, Apache-2.0); for the later second pass (stage 5).
pub const DEHTR_V2: ModelFile = ModelFile {
    file: "de-htr-web-v2.onnx",
    url: "https://huggingface.co/naeyn/de-htr-web-v2/resolve/6241c1ff0fd42725e0dc46fcd3217469cac851bb/model.onnx",
    sha256: "b576a0a1281b9be46b2574028b75575e041b7d7cb650f063886e733467cc1499",
    size: 10_396_365,
    group: Group::Base,
    license: "Apache-2.0",
};

/// All known files.
pub const ALL: &[ModelFile] = &[DET_SMALL, REC_SMALL, ORIENT_PPLCNET, DEHTR_V2, REC_TINY, DET_MEDIUM, REC_MEDIUM, ORIENT_DOCTR];

/// Files of the base package.
pub fn base_package() -> impl Iterator<Item = &'static ModelFile> {
    ALL.iter().filter(|m| m.group == Group::Base)
}

/// Total download size of the base package in bytes.
pub fn base_package_size() -> u64 {
    base_package().map(|m| m.size).sum()
}

/// Model size variant of the PP-OCRv6 detector/recognizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Variant {
    #[default]
    Small,
    Medium,
    /// Small detector + tiny recognizer: ~3× faster on arm64, print as good, handwriting −11 points
    /// (stage 3, `results/ocr-eval/2026-11-25/stage3-arm64.md`).
    Fast,
}

impl Variant {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "small" => Some(Self::Small),
            "medium" => Some(Self::Medium),
            "fast" | "tiny" => Some(Self::Fast),
            _ => None,
        }
    }
    pub fn files(self) -> (ModelFile, ModelFile) {
        match self {
            Self::Small => (DET_SMALL, REC_SMALL),
            Self::Medium => (DET_MEDIUM, REC_MEDIUM),
            Self::Fast => (DET_SMALL, REC_TINY),
        }
    }
}

/// First directory of `dirs` that contains `m.file` (size checked; the full checksum is checked at
/// installation, not at every start).
pub fn find(m: &ModelFile, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(m.file)).find(|p| std::fs::metadata(p).is_ok_and(|md| md.is_file() && md.len() == m.size))
}

/// SHA-256 of a file (hex).
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex(&h.finalize()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Checks size and checksum of downloaded bytes.
pub fn verify_bytes(m: &ModelFile, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 != m.size {
        return Err(Error::Checksum(format!("{}: size {} instead of {}", m.file, bytes.len(), m.size)));
    }
    let got = hex(&Sha256::digest(bytes));
    if got != m.sha256 {
        return Err(Error::Checksum(format!("{}: SHA-256 {got} instead of {}", m.file, m.sha256)));
    }
    Ok(())
}

/// Installs `m` into `dir` unless a verified copy is there: `fetch(url)` returns the file's
/// bytes; they are checked against the fixed checksum and written atomically (temporary file +
/// rename). Returns the path.
pub fn install(m: &ModelFile, dir: &Path, fetch: &mut dyn FnMut(&str) -> std::result::Result<Vec<u8>, String>) -> Result<PathBuf> {
    let target = dir.join(m.file);
    if target.is_file() && sha256_file(&target).is_ok_and(|s| s == m.sha256) {
        return Ok(target);
    }
    std::fs::create_dir_all(dir)?;
    let bytes = fetch(m.url).map_err(|e| Error::Download(format!("{}: {e}", m.url)))?;
    verify_bytes(m, &bytes)?;
    let tmp = dir.join(format!(".{}.part", m.file));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &target)?;
    Ok(target)
}

/// Installs several files; stops at the first error.
pub fn install_all<'a>(
    files: impl IntoIterator<Item = &'a ModelFile>,
    dir: &Path,
    fetch: &mut dyn FnMut(&str) -> std::result::Result<Vec<u8>, String>,
) -> Result<Vec<PathBuf>> {
    files.into_iter().map(|m| install(m, dir, fetch)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST: ModelFile = ModelFile {
        file: "t.onnx",
        url: "https://example.invalid/t.onnx",
        // sha256("abc")
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        size: 3,
        group: Group::OnDemand,
        license: "test",
    };

    #[test]
    fn registry_is_consistent() {
        for m in ALL {
            assert_eq!(m.sha256.len(), 64, "{}", m.file);
            assert!(m.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{}", m.file);
            assert!(m.url.starts_with("https://"), "{}", m.file);
            assert!(m.size > 0);
        }
        let mut names: Vec<&str> = ALL.iter().map(|m| m.file).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
        // base package stays around 50 MB (+ ONNX Runtime in the image)
        let mb = base_package_size() / 1_000_000;
        assert!((40..=60).contains(&mb), "{mb} MB");
    }

    #[test]
    fn install_verifies_and_skips_existing() {
        let dir = std::env::temp_dir().join(format!("pl-ppocr-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut calls = 0;
        let mut good = |_: &str| {
            calls += 1;
            Ok(b"abc".to_vec())
        };
        let p = install(&TEST, &dir, &mut good).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"abc");
        install(&TEST, &dir, &mut good).unwrap();
        assert_eq!(calls, 1, "verified file must not be fetched again");
        assert_eq!(find(&TEST, std::slice::from_ref(&dir)), Some(p.clone()));
        std::fs::remove_file(&p).unwrap();
        let mut bad = |_: &str| Ok(b"abd".to_vec());
        assert!(matches!(install(&TEST, &dir, &mut bad), Err(Error::Checksum(_))));
        assert!(!p.exists(), "corrupt download must not be installed");
        let mut short = |_: &str| Ok(b"ab".to_vec());
        assert!(matches!(install(&TEST, &dir, &mut short), Err(Error::Checksum(_))));
        let mut fail = |_: &str| Err("offline".to_string());
        assert!(matches!(install(&TEST, &dir, &mut fail), Err(Error::Download(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn variants() {
        assert_eq!(Variant::parse(""), Some(Variant::Small));
        assert_eq!(Variant::parse("Medium"), Some(Variant::Medium));
        assert_eq!(Variant::parse("large"), None);
        assert_eq!(Variant::parse("fast"), Some(Variant::Fast));
        assert_eq!(Variant::Fast.files().1.file, "PP-OCRv6_rec_tiny.onnx");
        assert_eq!(Variant::Medium.files().1.file, "PP-OCRv6_rec_medium.onnx");
    }
}
