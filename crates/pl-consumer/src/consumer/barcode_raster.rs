// E17 – barcodes without PDFium: rasterize pages with `pdftoppm` (poppler), like upstream.
//
// upstream: src/documents/barcodes.py@v3.4.2 (`BarcodePlugin.detect`: `convert_from_path(pdf,
//           dpi=barcode_dpi, output_folder=temp_dir)` from pdf2image = call of `pdftoppm`),
//           pdf2image 1.17 (`convert_from_path` → `pdftoppm -r <dpi> -f <n> -l <n> …`).
//
// PDFium remains the first choice (pl-ocr); only if it is missing and `pdftoppm` is on the PATH (Docker image:
// poppler-utils) does this renderer rasterize the pages in grayscale (PGM) for rxing.

use std::path::{Path, PathBuf};
use std::process::Command;

use pl_ocr::OcrError;
use pl_ocr::render::{PdfRenderer, Raster, RenderDoc};

/// `pdftoppm` as [`PdfRenderer`] (only [`RenderDoc::render`] in grayscale/color).
#[derive(Debug, Clone)]
pub struct PdftoppmRenderer {
    pub program: PathBuf,
    pub scratch: PathBuf,
}

impl PdftoppmRenderer {
    /// `pdftoppm` found (`pdftoppm -v` runs)? Otherwise `None`.
    pub fn detect(scratch: &Path) -> Option<Self> {
        let program = PathBuf::from("pdftoppm");
        let ok = Command::new(&program).arg("-v").output().is_ok();
        ok.then(|| Self { program, scratch: scratch.to_path_buf() })
    }
}

struct PdftoppmDoc<'a> {
    r: &'a PdftoppmRenderer,
    dir: PathBuf,
    pdf: PathBuf,
    pages: usize,
}

impl Drop for PdftoppmDoc<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl PdfRenderer for PdftoppmRenderer {
    fn name(&self) -> String {
        "pdftoppm".into()
    }

    fn open<'a>(&'a self, pdf: &'a [u8]) -> pl_ocr::Result<Box<dyn RenderDoc + 'a>> {
        let pages = lopdf::Document::load_mem(pdf).map_err(|e| OcrError::Parse(e.to_string()))?.get_pages().len();
        std::fs::create_dir_all(&self.scratch).map_err(|e| OcrError::Parse(e.to_string()))?;
        let dir = self.scratch.join(format!("paperless-barcode-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).map_err(|e| OcrError::Parse(e.to_string()))?;
        let doc = PdftoppmDoc { r: self, pdf: dir.join("in.pdf"), dir, pages };
        std::fs::write(&doc.pdf, pdf).map_err(|e| OcrError::Parse(e.to_string()))?;
        Ok(Box::new(doc))
    }
}

impl RenderDoc for PdftoppmDoc<'_> {
    fn page_count(&self) -> usize {
        self.pages
    }

    fn page_size_pt(&self, _index: usize) -> pl_ocr::Result<(f64, f64)> {
        Err(OcrError::Unavailable("page size is not provided by the pdftoppm renderer".into()))
    }

    fn render(&self, index: usize, dpi: f64, gray: bool) -> pl_ocr::Result<Raster> {
        let n = (index + 1).to_string();
        let prefix = self.dir.join(format!("page-{n}"));
        let mut cmd = Command::new(&self.r.program);
        cmd.arg("-r").arg(format!("{}", dpi.round().max(1.0) as u32)).args(["-f", &n, "-l", &n, "-singlefile"]);
        if gray {
            cmd.arg("-gray");
        }
        cmd.arg(&self.pdf).arg(&prefix);
        let out = cmd.output().map_err(|e| OcrError::Unavailable(format!("pdftoppm: {e}")))?;
        if !out.status.success() {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(OcrError::Parse(format!("pdftoppm failed: {msg}")));
        }
        let file = prefix.with_extension(if gray { "pgm" } else { "ppm" });
        let bytes = std::fs::read(&file).map_err(|e| OcrError::Parse(e.to_string()))?;
        let _ = std::fs::remove_file(&file);
        parse_pnm(&bytes, dpi).ok_or_else(|| OcrError::Parse("pdftoppm returned an unreadable image".into()))
    }
}

/// Binary PGM (P5) or PPM (P6) with maxval ≤ 255 → [`Raster`].
pub fn parse_pnm(b: &[u8], dpi: f64) -> Option<Raster> {
    let channels: u8 = match b.get(..2)? {
        b"P5" => 1,
        b"P6" => 3,
        _ => return None,
    };
    let mut pos = 2;
    let mut fields = [0u32; 3];
    for f in &mut fields {
        // Skip whitespace and comments
        loop {
            match b.get(pos)? {
                c if c.is_ascii_whitespace() => pos += 1,
                b'#' => {
                    while *b.get(pos)? != b'\n' {
                        pos += 1;
                    }
                }
                _ => break,
            }
        }
        let start = pos;
        while b.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
        }
        *f = std::str::from_utf8(b.get(start..pos)?).ok()?.parse().ok()?;
    }
    let [width, height, maxval] = fields;
    if maxval == 0 || maxval > 255 {
        return None;
    }
    pos += 1; // exactly one space after maxval
    let len = (width as usize).checked_mul(height as usize)?.checked_mul(usize::from(channels))?;
    let data = b.get(pos..pos.checked_add(len)?)?.to_vec();
    Some(Raster { width, height, channels, data, dpi })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pnm_header_with_comment() {
        let mut b = b"P5\n# made by test\n3 2\n255\n".to_vec();
        b.extend_from_slice(&[0, 1, 2, 3, 4, 5]);
        let r = parse_pnm(&b, 300.0).unwrap();
        assert_eq!((r.width, r.height, r.channels), (3, 2, 1));
        assert_eq!(r.data, vec![0, 1, 2, 3, 4, 5]);
        let mut c = b"P6 1 1 255\n".to_vec();
        c.extend_from_slice(&[9, 8, 7]);
        assert_eq!(parse_pnm(&c, 1.0).unwrap().channels, 3);
        assert!(parse_pnm(b"P5\n3 2\n255\n\x00", 1.0).is_none(), "truncated");
        assert!(parse_pnm(b"P2\n1 1\n255\n0", 1.0).is_none(), "ascii not supported");
    }

    /// Real call, only if `pdftoppm` is installed (otherwise skipped).
    #[test]
    fn renders_a_page_when_pdftoppm_is_installed() {
        let t = tempfile::tempdir().unwrap();
        let Some(r) = PdftoppmRenderer::detect(t.path()) else {
            eprintln!("pdftoppm not installed - skipped");
            return;
        };
        // smallest valid PDF with an empty page (72 × 72 pt)
        let pdf = minimal_pdf();
        let doc = r.open(&pdf).unwrap();
        assert_eq!(doc.page_count(), 1);
        let img = doc.render(0, 72.0, true).unwrap();
        assert_eq!((img.width, img.height, img.channels), (72, 72, 1));
        assert!(img.is_blank());
    }

    fn minimal_pdf() -> Vec<u8> {
        use lopdf::{Document, Object, dictionary};
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![0.into(), 0.into(), 72.into(), 72.into()],
        });
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }
}
