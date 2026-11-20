// upstream: src/documents/barcodes.py@v3.4.2 (BarcodePlugin.detect/separate_pages/
// convert_from_tiff_to_pdf), src/documents/double_sided.py (CollatePlugin, pikepdf-Teil)
//
//! Building blocks for pl-consumer: [`NativeBarcodes`] fits `stages::BarcodeScanner`
//! (`available`/`scan`/`split`) and `stages::PdfPageOps::tiff_to_pdf`. pl-consumer attaches them
//! with a thin adapter (barcode fields identical, errors → `StageError::Other(text)`),
//! see contract. No back reference to pl-consumer (otherwise a dependency cycle).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::barcodes::{self, Barcode, BarcodeReader, RxingReader, ScanSettings};
use crate::{PdfOpsError, Result};

/// PDFium-Raster + rxing.
pub struct NativeBarcodes {
    pub renderer: Arc<dyn pl_ocr::render::PdfRenderer>,
    pub reader: Arc<dyn BarcodeReader>,
    pub settings: ScanSettings,
}

impl NativeBarcodes {
    pub fn new(renderer: Arc<dyn pl_ocr::render::PdfRenderer>, settings: ScanSettings) -> Self {
        NativeBarcodes { renderer, reader: Arc::new(RxingReader), settings }
    }

    /// PDFium from `PAPERRUST_PDFIUM_PATH`, settings from `PAPERLESS_CONSUMER_BARCODE_*`.
    pub fn from_env() -> Self {
        let renderer: Arc<dyn pl_ocr::render::PdfRenderer> = Arc::from(pl_ocr::render::default_renderer());
        Self::new(renderer, ScanSettings::from(&barcodes::BarcodeConfig::from_env()))
    }

    /// Scanner available = renderer loadable.
    pub fn available(&self) -> bool {
        self.renderer.name() != "unavailable"
    }

    /// `scan_pdf` (errors are passed on).
    pub fn scan(&self, pdf: &Path, _tempdir: &Path) -> Result<Vec<Barcode>> {
        barcodes::scan_pdf(pdf, self.settings, self.renderer.as_ref(), self.reader.as_ref())
    }

    /// `BarcodePlugin.detect`: like [`Self::scan`], but errors are logged (password: own
    /// message) and treated as "no barcodes".
    pub fn detect(&self, pdf: &Path, tempdir: &Path) -> Vec<Barcode> {
        match self.scan(pdf, tempdir) {
            Ok(v) => v,
            Err(PdfOpsError::Password(e)) => {
                tracing::warn!(target: "paperless.barcodes", "File is likely password protected, not checking for barcodes: {e}");
                Vec::new()
            }
            Err(e) => {
                tracing::warn!(target: "paperless.barcodes", "Exception during barcode scanning: {e}");
                Vec::new()
            }
        }
    }

    /// `separate_pages` with file names from the stem of `pdf` (for TIFF this is the
    /// converted PDF with the same stem), outputs in `tempdir`.
    pub fn split(&self, pdf: &Path, separator_pages: &[(i64, bool)], tempdir: &Path) -> Result<Vec<PathBuf>> {
        split_pdf(pdf, separator_pages, tempdir)
    }

    /// `convert_from_tiff_to_pdf`.
    pub fn tiff_to_pdf(&self, tiff: &Path, tempdir: &Path) -> Result<PathBuf> {
        crate::convert::convert_from_tiff_to_pdf(tiff, tempdir)
    }
}

/// Like [`NativeBarcodes::split`], without a scanner.
pub fn split_pdf(pdf: &Path, separator_pages: &[(i64, bool)], tempdir: &Path) -> Result<Vec<PathBuf>> {
    let map: BTreeMap<i64, bool> = separator_pages.iter().copied().collect();
    let stem = pdf.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    barcodes::separate_pages(pdf, &map, &stem, tempdir, pdf)
}
