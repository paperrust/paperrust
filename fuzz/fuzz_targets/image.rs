//! Image front stage (pl-ocr `imageinfo::probe` for the PNG/JPEG/TIFF/BMP/GIF/WebP/HEIC header and
//! `native::decode_frames` incl. multi-page TIFFs), with the bomb check as on ingestion
//! (Pillow default limit 89 478 485 pixels).
#![no_main]
use libfuzzer_sys::fuzz_target;
include!("sancov_coff.rs");

fuzz_target!(|data: &[u8]| {
    let Ok(info) = pl_ocr::imageinfo::probe(data) else { return };
    let limit = pl_ocr::imageinfo::pil_pixel_limit(None);
    if info.decompression_bomb_check(limit).is_err() {
        return;
    }
    if let Ok(frames) = pl_ocr::native::decode_frames(data, &info) {
        let _ = frames.len();
    }
});
