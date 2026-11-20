// PaperRust-specific (no upstream counterpart): "Convert to PDF" for images and Office files.
// Contract: docs/contract/pl-pdftools.md §7.
// Pattern: src/documents/views.py@v3.4.2 DocumentViewSet.update_version (permissions, new version via
// consume_file with root_document_id); image → PDF like src/paperless_tesseract/parsers.py@v3.4.2
// RasterisedDocumentParser._convert_image_to_pdfa (without OCR) or OCRmyPDF on the image (with OCR).
//
//! `POST /api/documents/{id}/convert_pdf/` `{"ocr": bool, "version_label": str}`
//!
//! * The source is the **latest version**; the result becomes a **new version** of the same
//!   document (the previous versions remain unchanged). Afterwards redacting and
//!   shrinking work, because the latest version is a PDF.
//! * Images (PNG, JPEG, TIFF also multi-page, WebP, BMP, GIF): in the server process with pl-ocr
//!   (`image_to_pdfa`, JPEG embedded unchanged). With `ocr: true` the OCR of pl-ocr
//!   (Tesseract) runs and the PDF gets an invisible text layer. Images with transparency are placed on
//!   white.
//! * Office (docx/odt/rtf, also Markdown/HTML): WordCraft in the child process `paperrust-pdftool`
//!   (time/memory limit). The text is already contained in the PDF as real text; `ocr` is
//!   ignored.
//!
//! Permissions: like redact/shrink (`documents.change_document` + `change_document` on the
//! root document, via pl-authz).

use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use pl_authz::AuthUser;
use pl_core::{AppState, RequestContext};
use pl_pdftools::job::{Op, Reply, document_format};
use pl_pdftools::runner::RunError;

use crate::drf::Errors;
use crate::error::{ApiError, ApiResult};
use crate::pdftools::{body_map, bool_of, cleanup, detail, label_of, ok, output_name, paths, queue_version, run_error, s, scratch_file, target_any};

const LOG: &str = "paperrust.convert";

/// What is converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Office(&'static str),
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::Office(_) => "office",
        }
    }
}

/// Convertible? (`None` = PDF or unknown format). Images including HEIC (previously in the
/// child process to PNG, as on ingestion); Office as on ingestion ([`document_format`]: also
/// old Word and spreadsheets).
pub fn kind_of(mime_type: &str) -> Option<Kind> {
    if pl_ocr::imageinfo::is_image(mime_type) {
        return Some(Kind::Image);
    }
    document_format(mime_type).map(Kind::Office)
}

/// Error of the image conversion (text for users, without paths).
#[derive(Debug, PartialEq)]
pub enum ImageError {
    /// OCR requested, but no OCR engine available.
    NoOcr,
    /// Image not readable, too large, etc.
    Bad(String),
}

impl ImageError {
    fn into_api(self) -> ApiError {
        match self {
            ImageError::NoOcr => detail(StatusCode::SERVICE_UNAVAILABLE, "Text recognition (OCR) is not available on this server. Convert without OCR."),
            ImageError::Bad(m) => detail(StatusCode::BAD_REQUEST, m),
        }
    }
}

fn bad(e: impl std::fmt::Display) -> ImageError {
    ImageError::Bad(format!("The image could not be converted: {e}"))
}

/// Place an image with transparency on white; returns the path of the flat PNG file (all pages of a TIFF
/// not needed: TIFF with alpha is rare, there only the first page – see Known differences).
fn flatten_alpha(data: &[u8], info: &pl_ocr::imageinfo::ImageInfo, work: &FsPath) -> Result<PathBuf, ImageError> {
    let frames = pl_ocr::native::decode_frames(data, info).map_err(|e| bad(e.message()))?;
    let Some(first) = frames.into_iter().next() else { return Err(bad("no image")) };
    let rgba = first.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut rgb = image::RgbImage::new(w, h);
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = u32::from(p[3]);
        let mix = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
        rgb.put_pixel(x, y, image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])]));
    }
    let out = work.join("flattened.png");
    rgb.save_with_format(&out, image::ImageFormat::Png).map_err(bad)?;
    Ok(out)
}

/// Image → PDF (synchronous, for `spawn_blocking`). With `ocr` the PDF gets a text layer.
pub fn convert_image(src: &FsPath, mime_type: &str, ocr: bool, cfg: &pl_ocr::OcrConfig, work: &FsPath) -> Result<PathBuf, ImageError> {
    std::fs::create_dir_all(work).map_err(bad)?;
    // HEIC/HEIF: first to PNG in the child process (rotation/ICC applied), as on ingestion.
    let heic_png;
    let (src, mime_type) = if mime_type == "image/heic" {
        heic_png = work.join("heic.png");
        pl_consumer::office::heic_to_png_blocking(src, &heic_png, 0).map_err(|e| bad(e.user_message()))?;
        (heic_png.as_path(), "image/png")
    } else {
        (src, mime_type)
    };
    let data = std::fs::read(src).map_err(|_| bad("file not readable"))?;
    let info = pl_ocr::imageinfo::probe(&data).map_err(|e| bad(e.message()))?;
    let limit = pl_ocr::imageinfo::pil_pixel_limit(pl_core::config::var("PAPERLESS_MAX_IMAGE_PIXELS").and_then(|v| v.trim().parse::<i64>().ok()));
    info.decompression_bomb_check(limit).map_err(|e| bad(e.message()))?;
    let (input, mime) = if info.has_alpha { (flatten_alpha(&data, &info, work)?, "image/png") } else { (src.to_path_buf(), mime_type) };
    drop(data);
    let renderer: Box<dyn pl_ocr::render::PdfRenderer> = match pl_ocr::render::PdfiumRenderer::from_env() {
        Ok(p) => Box::new(p),
        Err(_) => Box::new(pl_ocr::render::UnavailableRenderer { reason: "PDFium not loaded".into() }),
    };
    if !ocr {
        let backend = pl_ocr::native::NativeOcrmypdf::new(None, renderer, work.join("pages"));
        return backend.image_to_pdfa(&input, cfg.image_dpi, work).map_err(|e| bad(e.message()));
    }
    let tesseract = Some(pl_ocr::engine::TesseractCli::from_env()).filter(|t| t.is_available());
    let Some(engine) = pl_ocr::engine_select::engine_from_env(tesseract, &cfg.language) else {
        return Err(ImageError::NoOcr);
    };
    let backend = pl_ocr::native::NativeOcrmypdf::new(Some(engine), renderer, work.join("pages"));
    let mut cfg = cfg.clone();
    if cfg.mode == pl_ocr::config::Mode::Off {
        // explicitly requested: OCR even with OCR_MODE=off
        cfg.mode = pl_ocr::config::Mode::Auto;
    }
    let out = pl_ocr::native::parse_document(&backend, &cfg, work, &input, mime, true).map_err(|e| bad(e.message()))?;
    out.archive_path.filter(|p| p.is_file()).ok_or_else(|| bad("no PDF was produced"))
}

/// OCR configuration as on ingestion (environment + settings in the database).
async fn ocr_config(state: &AppState) -> ApiResult<pl_ocr::OcrConfig> {
    let settings = pl_ocr::OcrSettings::from_process_env().map_err(|e| ApiError::Uncaught(e.to_string()))?;
    let app = pl_consumer::consumer::load_app_config(state.db()).await.map_err(|e| ApiError::Uncaught(e.to_string()))?;
    pl_ocr::OcrConfig::new(&settings, &app).map_err(|e| ApiError::Uncaught(e.to_string()))
}

/// Office → PDF in the child process (limits from `PAPERRUST_OFFICE_TO_PDF_TIMEOUT` / `PAPERRUST_PDFTOOL_*`).
async fn convert_office(src: &FsPath, format: &str, out: &FsPath) -> Result<pl_pdftools::ConvertReport, RunError> {
    let tool = pl_pdftools::runner::tool_path().ok_or(RunError::NotInstalled)?;
    let op = Op::Office { input: s(src), output: s(out), format: format.to_string() };
    match pl_pdftools::runner::run_with(&tool, op, pl_consumer::office::limits()).await? {
        Reply::Converted(r) => Ok(r),
        _ => Err(RunError::Protocol("unexpected reply".into())),
    }
}

/// POST /api/documents/{id}/convert_pdf/
pub async fn convert_pdf_handler(State(state): State<AppState>, ctx: RequestContext, auth: AuthUser, Path(pk): Path<String>, body: axum::body::Bytes) -> Response {
    match convert_pdf(&state, &ctx, &auth, &pk, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn convert_pdf(state: &AppState, ctx: &RequestContext, auth: &AuthUser, pk: &str, body: &axum::body::Bytes) -> ApiResult<Response> {
    let m = body_map(ctx, body)?;
    let mut errors = Errors::default();
    let ocr = bool_of(&m, "ocr", &mut errors);
    let label = label_of(&m, &mut errors).or_else(|| Some("PDF".to_string()));
    if !errors.is_empty() {
        return Err(ApiError::Validation(Value::Object(errors.0)));
    }
    let t = target_any(state, auth, pk).await?;
    if t.source.mime_type == "application/pdf" {
        return Err(detail(StatusCode::BAD_REQUEST, "This document is already a PDF."));
    }
    let Some(kind) = kind_of(&t.source.mime_type) else {
        return Err(detail(StatusCode::BAD_REQUEST, "This document cannot be converted to PDF."));
    };
    let (src, _) = paths(state, &t.source)?;
    let out = scratch_file(state, &output_name(t.source.original_filename.as_deref(), t.root.id, "pdf"))?;
    let result: Result<PathBuf, ApiError> = match kind {
        Kind::Office(format) => {
            if !pl_consumer::office::enabled() {
                Err(detail(StatusCode::SERVICE_UNAVAILABLE, "Office conversion is switched off on this server."))
            } else {
                match convert_office(&src, format, &out).await {
                    Ok(r) => {
                        tracing::info!(target: LOG, "Converted document {} ({format}, {} bytes) to PDF ({} bytes)", t.root.id, r.size_before, r.size_after);
                        Ok(out.clone())
                    }
                    Err(e) => Err(run_error(e)),
                }
            }
        }
        Kind::Image => {
            let cfg = ocr_config(state).await?;
            let work = out.parent().map(|p| p.join("work")).unwrap_or_else(|| out.with_extension("work"));
            let (src2, mime, work2) = (src.clone(), t.source.mime_type.clone(), work.clone());
            let r = tokio::task::spawn_blocking(move || convert_image(&src2, &mime, ocr, &cfg, &work2)).await.map_err(|e| ApiError::Uncaught(e.to_string()))?;
            let moved = r.map_err(ImageError::into_api).and_then(|pdf| std::fs::rename(&pdf, &out).or_else(|_| std::fs::copy(&pdf, &out).map(|_| ())).map_err(|e| ApiError::Uncaught(e.to_string())));
            let _ = std::fs::remove_dir_all(&work);
            moved.map(|()| {
                tracing::info!(target: LOG, "Converted image document {} to PDF (OCR: {ocr})", t.root.id);
                out.clone()
            })
        }
    };
    let file = match result {
        Ok(f) => f,
        Err(e) => {
            cleanup(&out);
            return Err(e);
        }
    };
    let task_id = match queue_version(state, auth.user().id, t.root.id, &file, label).await {
        Ok(id) => id,
        Err(e) => {
            cleanup(&file);
            return Err(e);
        }
    };
    let used_ocr = ocr && kind == Kind::Image;
    Ok(ok(ctx, json!({"task_id": task_id, "document": t.root.id, "source_version": t.source.id, "kind": kind.name(), "ocr": used_ocr})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_of("image/png"), Some(Kind::Image));
        assert_eq!(kind_of("image/tiff"), Some(Kind::Image));
        assert_eq!(kind_of("image/webp"), Some(Kind::Image));
        assert_eq!(kind_of("image/heic"), Some(Kind::Image));
        assert_eq!(kind_of("application/pdf"), None);
        assert_eq!(kind_of("application/vnd.openxmlformats-officedocument.wordprocessingml.document"), Some(Kind::Office("docx")));
        assert_eq!(kind_of("application/vnd.oasis.opendocument.spreadsheet"), Some(Kind::Office("ods")));
        assert_eq!(kind_of("application/msword"), Some(Kind::Office("doc")));
        assert_eq!(kind_of("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"), Some(Kind::Office("xlsx")));
        assert_eq!(kind_of("application/vnd.openxmlformats-officedocument.presentationml.presentation"), None);
    }
}
