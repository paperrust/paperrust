//! PDF page → raster image.
//!
//! Implemented via PDFium as a dynamic library (`pdfium.dll` / `libpdfium.so` /
//! `libpdfium.dylib`), loaded with `libloading` – no build-time link, no dependency if
//! PDFium is missing (then [`UnavailableRenderer`] or error `EngineUnavailable`). Path from
//! `PAPERRUST_PDFIUM_PATH` (file or directory). OCRmyPDF 17 also rasterizes with PDFium
//! (plugin `pypdfium`); only the roughly 15 required C functions are bound (own minimal
//! FFI instead of the large `pdfium-render` crate).
//!
//! PDFium is not thread-safe: all calls run under a global lock.

use std::ffi::{c_char, c_int, c_ulong, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{OcrError, Result};

/// Maximum number of pixels a rasterized page may have (protection against huge pages).
pub const MAX_RENDER_PIXELS: u64 = 400_000_000;

/// Largest width/height of a rasterized page in pixels.
pub const MAX_RENDER_SIDE: f64 = 65_000.0;

/// Pixels of a page `w_pt` × `h_pt` (points) at `dpi` (rounded up like when rasterizing).
pub fn page_pixels(w_pt: f64, h_pt: f64, dpi: f64) -> (f64, f64) {
    ((w_pt * dpi / 72.0).ceil().max(1.0), (h_pt * dpi / 72.0).ceil().max(1.0))
}

/// PaperRust: highest resolution ≤ `dpi` at which a page `w_pt` × `h_pt` has at most
/// `max_pixels` pixels and at most [`MAX_RENDER_SIDE`] per side. Huge pages (PDF
/// allows up to 14400 × 14400 pt) are thus rasterized more coarsely instead of rejected (test log
/// 2026-11-16 C-017). Invalid dimensions: `dpi` unchanged (the caller keeps checking).
pub fn fit_dpi(w_pt: f64, h_pt: f64, dpi: f64, max_pixels: u64) -> f64 {
    if !(w_pt.is_finite() && h_pt.is_finite() && dpi.is_finite()) || w_pt <= 0.0 || h_pt <= 0.0 || dpi <= 0.0 {
        return dpi;
    }
    let fits = |d: f64| {
        let (w, h) = page_pixels(w_pt, h_pt, d);
        w * h <= max_pixels as f64 && w <= MAX_RENDER_SIDE && h <= MAX_RENDER_SIDE
    };
    if fits(dpi) {
        return dpi;
    }
    let by_area = 72.0 * (max_pixels as f64 / (w_pt * h_pt)).sqrt();
    let by_side = 72.0 * MAX_RENDER_SIDE / w_pt.max(h_pt);
    // round down to 1/100 dpi, because of the rounding up of pixels if necessary step by step further
    let mut d = (by_area.min(by_side).min(dpi) * 100.0).floor() / 100.0;
    for _ in 0..1000 {
        if d <= 0.01 || fits(d) {
            break;
        }
        d = ((d - 0.01) * 100.0).round() / 100.0;
    }
    d.max(0.01)
}

/// Rasterized image: 1 channel (gray) or 3 channels (RGB), 8 bit, rows without padding.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub data: Vec<u8>,
    /// Resolution at which it was rasterized.
    pub dpi: f64,
}

impl Raster {
    /// Write as PNM (P5/P6) – every Tesseract/Leptonica version reads it.
    pub fn write_pnm(&self, path: &Path) -> Result<()> {
        let magic = if self.channels == 1 { "P5" } else { "P6" };
        let mut buf = format!("{magic}\n{} {}\n255\n", self.width, self.height).into_bytes();
        buf.extend_from_slice(&self.data);
        std::fs::write(path, buf)?;
        Ok(())
    }

    /// Write as uncompressed TIFF (one strip). Resolution 0/1 like PNM (no value):
    /// Leptonica then reads `xres = 0`, Tesseract behaves as with the PNM file.
    pub fn write_tiff(&self, path: &Path) -> Result<()> {
        use tiff::encoder::{Rational, TiffEncoder, colortype};
        use tiff::tags::ResolutionUnit;
        let err = |e: tiff::TiffError| OcrError::Other(format!("TIFF write failed: {e}"));
        let mut buf = Vec::with_capacity(self.data.len() + 512);
        {
            let mut enc = TiffEncoder::new(std::io::Cursor::new(&mut buf)).map_err(err)?;
            let zero = Rational { n: 0, d: 1 };
            macro_rules! write {
                ($ct:ty) => {{
                    let mut img = enc.new_image::<$ct>(self.width, self.height).map_err(err)?;
                    img.resolution(ResolutionUnit::None, zero);
                    img.rows_per_strip(self.height.max(1)).map_err(err)?;
                    img.write_data(&self.data).map_err(err)?;
                }};
            }
            if self.channels == 1 {
                write!(colortype::Gray8);
            } else {
                write!(colortype::RGB8);
            }
        }
        std::fs::write(path, buf)?;
        Ok(())
    }

    pub fn to_dynamic(&self) -> Option<image::DynamicImage> {
        if self.channels == 1 {
            image::GrayImage::from_raw(self.width, self.height, self.data.clone()).map(image::DynamicImage::ImageLuma8)
        } else {
            image::RgbImage::from_raw(self.width, self.height, self.data.clone()).map(image::DynamicImage::ImageRgb8)
        }
    }

    pub fn from_dynamic(img: &image::DynamicImage, dpi: f64) -> Self {
        use image::DynamicImage as D;
        match img {
            D::ImageLuma8(g) => Raster { width: g.width(), height: g.height(), channels: 1, data: g.as_raw().clone(), dpi },
            D::ImageLuma16(_) | D::ImageLumaA8(_) | D::ImageLumaA16(_) => {
                let g = img.to_luma8();
                Raster { width: g.width(), height: g.height(), channels: 1, data: g.into_raw(), dpi }
            }
            _ => {
                // Pillow `convert("RGB")`: alpha is discarded, not blended.
                let r = img.to_rgb8();
                Raster { width: r.width(), height: r.height(), channels: 3, data: r.into_raw(), dpi }
            }
        }
    }

    /// Rotate by 90/180/270 degrees clockwise.
    pub fn rotate_cw(&self, degrees: u16) -> Raster {
        let Some(img) = self.to_dynamic() else { return self.clone() };
        let r = match degrees % 360 {
            90 => img.rotate90(),
            180 => img.rotate180(),
            270 => img.rotate270(),
            _ => img,
        };
        Raster::from_dynamic(&r, self.dpi)
    }

    pub fn is_blank(&self) -> bool {
        self.data.iter().all(|b| *b == 255)
    }
}

/// An opened document.
pub trait RenderDoc {
    fn page_count(&self) -> usize;
    /// Page size in points (after `/Rotate`).
    fn page_size_pt(&self, index: usize) -> Result<(f64, f64)>;
    /// Rasterize a page (white background, annotations with appearance).
    fn render(&self, index: usize, dpi: f64, gray: bool) -> Result<Raster>;
}

pub trait PdfRenderer: Send + Sync {
    fn name(&self) -> String;
    fn open<'a>(&'a self, pdf: &'a [u8]) -> Result<Box<dyn RenderDoc + 'a>>;
}

/// Placeholder when no renderer is available.
#[derive(Debug, Default, Clone)]
pub struct UnavailableRenderer {
    pub reason: String,
}

impl PdfRenderer for UnavailableRenderer {
    fn name(&self) -> String {
        "unavailable".into()
    }
    fn open<'a>(&'a self, _: &'a [u8]) -> Result<Box<dyn RenderDoc + 'a>> {
        Err(OcrError::Unavailable(format!("no PDF renderer available: {}", self.reason)))
    }
}

// ----------------------------------------------------------------------------------------------
// PDFium
// ----------------------------------------------------------------------------------------------

type Doc = *mut c_void;
type Page = *mut c_void;
type Bitmap = *mut c_void;

struct Api {
    _lib: libloading::Library,
    load_mem_document: unsafe extern "C" fn(*const c_void, c_int, *const c_char) -> Doc,
    close_document: unsafe extern "C" fn(Doc),
    get_last_error: unsafe extern "C" fn() -> c_ulong,
    get_page_count: unsafe extern "C" fn(Doc) -> c_int,
    load_page: unsafe extern "C" fn(Doc, c_int) -> Page,
    close_page: unsafe extern "C" fn(Page),
    page_width: unsafe extern "C" fn(Page) -> f32,
    page_height: unsafe extern "C" fn(Page) -> f32,
    bitmap_create: unsafe extern "C" fn(c_int, c_int, c_int) -> Bitmap,
    bitmap_fill_rect: unsafe extern "C" fn(Bitmap, c_int, c_int, c_int, c_int, c_ulong) -> c_int,
    bitmap_buffer: unsafe extern "C" fn(Bitmap) -> *mut c_void,
    bitmap_stride: unsafe extern "C" fn(Bitmap) -> c_int,
    bitmap_destroy: unsafe extern "C" fn(Bitmap),
    render_page_bitmap: unsafe extern "C" fn(Bitmap, Page, c_int, c_int, c_int, c_int, c_int, c_int),
}

// Function pointers + library handle; access only under `LOCK`.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static LOCK: Mutex<()> = Mutex::new(());

const FPDF_ANNOT: c_int = 0x01;
const FPDF_PRINTING: c_int = 0x800;

/// PDFium-Renderer.
#[derive(Clone)]
pub struct PdfiumRenderer {
    api: Arc<Api>,
    pub library_path: PathBuf,
    /// Upper limit for pixels per page (protection against huge pages).
    pub max_pixels: u64,
}

impl std::fmt::Debug for PdfiumRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdfiumRenderer").field("library_path", &self.library_path).finish()
    }
}

fn lib_file_name() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

/// PaperRust (test log 2026-11-16 A-294): places where PDFium is searched, in this
/// order. If `PAPERRUST_PDFIUM_PATH` is set, only that. Otherwise explicitly the
/// program directory and its subfolder `pdfium/` (on Linux/macOS the loader does not look there
/// by itself; the system status page does report the program directory as location), finally
/// the bare library name (system search path).
pub fn pdfium_candidates(configured: Option<std::ffi::OsString>, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    if let Some(p) = configured.filter(|p| !p.is_empty()) {
        return vec![PathBuf::from(p)];
    }
    let mut out = Vec::new();
    if let Some(d) = exe_dir {
        for c in [d.join(lib_file_name()), d.join("pdfium").join(lib_file_name())] {
            if c.is_file() {
                out.push(c);
            }
        }
    }
    out.push(PathBuf::from(lib_file_name()));
    out
}

impl PdfiumRenderer {
    /// `PAPERRUST_PDFIUM_PATH` (file or directory; environment or stored in the
    /// UI), otherwise next to the program (also subfolder `pdfium/` as in the desktop package),
    /// otherwise system search for the library name (see [`pdfium_candidates`]).
    pub fn from_env() -> Result<Self> {
        let configured = pl_envlive::var_os("PAPERRUST_PDFIUM_PATH");
        let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
        let mut first_err = None;
        for c in pdfium_candidates(configured, exe_dir.as_deref()) {
            match Self::load(&c) {
                Ok(r) => return Ok(r),
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        Err(first_err.unwrap_or_else(|| OcrError::Unavailable("PDFium not found".into())))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let path = if path.is_dir() { path.join(lib_file_name()) } else { path.to_path_buf() };
        // SAFETY: loading a PDFium library configured by the operator; its
        // initialization has no preconditions.
        let lib = unsafe { libloading::Library::new(&path) }
            .map_err(|e| OcrError::Unavailable(format!("PDFium not loadable from {}: {e}", path.display())))?;
        macro_rules! sym {
            ($name:literal) => {
                // SAFETY: signatures correspond to fpdfview.h (PDFium chromium/7xxx).
                *unsafe { lib.get($name) }.map_err(|e| {
                    OcrError::Unavailable(format!("PDFium symbol {} missing: {e}", String::from_utf8_lossy($name)))
                })?
            };
        }
        let init: unsafe extern "C" fn() = sym!(b"FPDF_InitLibrary\0");
        let api = Api {
            load_mem_document: sym!(b"FPDF_LoadMemDocument\0"),
            close_document: sym!(b"FPDF_CloseDocument\0"),
            get_last_error: sym!(b"FPDF_GetLastError\0"),
            get_page_count: sym!(b"FPDF_GetPageCount\0"),
            load_page: sym!(b"FPDF_LoadPage\0"),
            close_page: sym!(b"FPDF_ClosePage\0"),
            page_width: sym!(b"FPDF_GetPageWidthF\0"),
            page_height: sym!(b"FPDF_GetPageHeightF\0"),
            bitmap_create: sym!(b"FPDFBitmap_Create\0"),
            bitmap_fill_rect: sym!(b"FPDFBitmap_FillRect\0"),
            bitmap_buffer: sym!(b"FPDFBitmap_GetBuffer\0"),
            bitmap_stride: sym!(b"FPDFBitmap_GetStride\0"),
            bitmap_destroy: sym!(b"FPDFBitmap_Destroy\0"),
            render_page_bitmap: sym!(b"FPDF_RenderPageBitmap\0"),
            _lib: lib,
        };
        {
            let _g = LOCK.lock().map_err(|_| OcrError::Other("PDFium lock poisoned".into()))?;
            // SAFETY: repeated init is allowed in PDFium (reference counting since chromium/4xxx).
            unsafe { init() };
        }
        Ok(Self { api: Arc::new(api), library_path: path, max_pixels: MAX_RENDER_PIXELS })
    }
}

struct PdfiumDoc<'a> {
    r: &'a PdfiumRenderer,
    doc: Doc,
    pages: usize,
    _data: &'a [u8],
}

impl Drop for PdfiumDoc<'_> {
    fn drop(&mut self) {
        let _g = LOCK.lock();
        // SAFETY: `doc` comes from FPDF_LoadMemDocument and is closed exactly once.
        unsafe { (self.r.api.close_document)(self.doc) };
    }
}

fn pdfium_error(code: c_ulong) -> OcrError {
    match code {
        4 => OcrError::EncryptedPdf,
        2 => OcrError::InputFile("PDFium: file not found or could not be opened".into()),
        3 => OcrError::InputFile("PDFium: file not in PDF format or corrupted".into()),
        5 => OcrError::InputFile("PDFium: unsupported security scheme".into()),
        6 => OcrError::InputFile("PDFium: page not found or content error".into()),
        n => OcrError::InputFile(format!("PDFium: unknown error {n}")),
    }
}

impl PdfRenderer for PdfiumRenderer {
    fn name(&self) -> String {
        format!("pdfium ({})", self.library_path.display())
    }

    fn open<'a>(&'a self, pdf: &'a [u8]) -> Result<Box<dyn RenderDoc + 'a>> {
        let len = c_int::try_from(pdf.len()).map_err(|_| OcrError::InputFile("PDF too large for PDFium".into()))?;
        let _g = LOCK.lock().map_err(|_| OcrError::Other("PDFium lock poisoned".into()))?;
        // SAFETY: `pdf` lives at least as long as the document (lifetime 'a in PdfiumDoc).
        let doc = unsafe { (self.api.load_mem_document)(pdf.as_ptr().cast(), len, std::ptr::null()) };
        if doc.is_null() {
            // SAFETY: simple query.
            return Err(pdfium_error(unsafe { (self.api.get_last_error)() }));
        }
        // SAFETY: valid document.
        let pages = unsafe { (self.api.get_page_count)(doc) }.max(0) as usize;
        Ok(Box::new(PdfiumDoc { r: self, doc, pages, _data: pdf }))
    }
}

impl PdfiumDoc<'_> {
    fn with_page<T>(&self, index: usize, f: impl FnOnce(&Api, Page) -> Result<T>) -> Result<T> {
        if index >= self.pages {
            return Err(OcrError::InputFile(format!("page {index} out of range")));
        }
        let _g = LOCK.lock().map_err(|_| OcrError::Other("PDFium lock poisoned".into()))?;
        let api = &self.r.api;
        // SAFETY: index checked, document valid.
        let page = unsafe { (api.load_page)(self.doc, index as c_int) };
        if page.is_null() {
            return Err(OcrError::InputFile(format!("PDFium could not load page {}", index + 1)));
        }
        let r = f(api, page);
        // SAFETY: page from FPDF_LoadPage.
        unsafe { (api.close_page)(page) };
        r
    }
}

impl RenderDoc for PdfiumDoc<'_> {
    fn page_count(&self) -> usize {
        self.pages
    }

    fn page_size_pt(&self, index: usize) -> Result<(f64, f64)> {
        // SAFETY: valid page.
        self.with_page(index, |api, p| Ok(unsafe { (f64::from((api.page_width)(p)), f64::from((api.page_height)(p))) }))
    }

    fn render(&self, index: usize, dpi: f64, gray: bool) -> Result<Raster> {
        let max_pixels = self.r.max_pixels;
        self.with_page(index, |api, p| {
            // SAFETY: valid page.
            let (w_pt, h_pt) = unsafe { (f64::from((api.page_width)(p)), f64::from((api.page_height)(p))) };
            // PaperRust: rasterize pages that are too large more coarsely instead of rejecting (C-017); `Raster::dpi`
            // gives the actual resolution so that coordinates (OCR words) are correct.
            let fitted = fit_dpi(w_pt, h_pt, dpi, max_pixels);
            if fitted < dpi {
                tracing::warn!("Page {} is very large; rendering at {fitted:.2} dpi instead of {dpi} dpi", index + 1);
            }
            let dpi = fitted;
            let (w, h) = page_pixels(w_pt, h_pt, dpi);
            if !(w.is_finite() && h.is_finite()) || w * h > max_pixels as f64 || w > 65_000.0 || h > 65_000.0 {
                return Err(OcrError::DecompressionBomb(format!(
                    "Page {} at {dpi} dpi would be {w}x{h} pixels",
                    index + 1
                )));
            }
            let (w, h) = (w as c_int, h as c_int);
            // SAFETY: BGRx bitmap; buffer belongs to PDFium until Destroy.
            unsafe {
                let bmp = (api.bitmap_create)(w, h, 0);
                if bmp.is_null() {
                    return Err(OcrError::Other("PDFium could not allocate bitmap".into()));
                }
                (api.bitmap_fill_rect)(bmp, 0, 0, w, h, 0xFFFF_FFFF);
                (api.render_page_bitmap)(bmp, p, 0, 0, w, h, 0, FPDF_ANNOT | FPDF_PRINTING);
                let stride = (api.bitmap_stride)(bmp).max(0) as usize;
                let buf = (api.bitmap_buffer)(bmp) as *const u8;
                let (wu, hu) = (w as usize, h as usize);
                let ch = if gray { 1 } else { 3 };
                let mut data = Vec::with_capacity(wu * hu * ch);
                if !buf.is_null() && stride >= wu * 4 {
                    let src = std::slice::from_raw_parts(buf, stride * hu);
                    for y in 0..hu {
                        let row = &src[y * stride..y * stride + wu * 4];
                        for px in row.as_chunks::<4>().0 {
                            let (b, g, r) = (px[0], px[1], px[2]);
                            if gray {
                                // ITU-R 601-2 like Pillow convert("L")
                                let l = (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 + 500) / 1000;
                                data.push(l as u8);
                            } else {
                                data.extend_from_slice(&[r, g, b]);
                            }
                        }
                    }
                }
                (api.bitmap_destroy)(bmp);
                if data.len() != wu * hu * ch {
                    return Err(OcrError::Other("PDFium returned no bitmap buffer".into()));
                }
                Ok(Raster { width: w as u32, height: h as u32, channels: ch as u8, data, dpi })
            }
        })
    }
}

/// Renderer from the environment: PDFium if loadable, otherwise placeholder.
pub fn default_renderer() -> Box<dyn PdfRenderer> {
    match PdfiumRenderer::from_env() {
        Ok(r) => Box::new(r),
        Err(e) => Box::new(UnavailableRenderer { reason: e.message() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdfium_is_looked_up_next_to_the_program() {
        let t = tempfile::tempdir().unwrap();
        // nothing next to the program: only the system search
        assert_eq!(pdfium_candidates(None, Some(t.path())), vec![PathBuf::from(lib_file_name())]);
        // desktop package: <program>/pdfium/<library>
        std::fs::create_dir_all(t.path().join("pdfium")).unwrap();
        std::fs::write(t.path().join("pdfium").join(lib_file_name()), b"x").unwrap();
        assert_eq!(
            pdfium_candidates(None, Some(t.path())),
            vec![t.path().join("pdfium").join(lib_file_name()), PathBuf::from(lib_file_name())]
        );
        // directly next to it takes precedence
        std::fs::write(t.path().join(lib_file_name()), b"x").unwrap();
        assert_eq!(pdfium_candidates(None, Some(t.path()))[0], t.path().join(lib_file_name()));
        // set: only that; empty does not count
        assert_eq!(pdfium_candidates(Some("C:/x/pdfium.dll".into()), Some(t.path())), vec![PathBuf::from("C:/x/pdfium.dll")]);
        assert_eq!(pdfium_candidates(Some("".into()), None), vec![PathBuf::from(lib_file_name())]);
    }

    #[test]
    fn fit_dpi_only_lowers_when_needed() {
        assert_eq!(fit_dpi(595.0, 842.0, 300.0, MAX_RENDER_PIXELS), 300.0);
        let d = fit_dpi(14_400.0, 14_400.0, 400.0, MAX_RENDER_PIXELS);
        let (w, h) = page_pixels(14_400.0, 14_400.0, d);
        assert!(d < 400.0 && w * h <= MAX_RENDER_PIXELS as f64 && w <= MAX_RENDER_SIDE, "{d}");
    }
}
