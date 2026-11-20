//! Office readers (pl-parsers `office`): OOXML/ODF (ZIP), OLE (doc/xls/ppt), RTF – text and slides.
//! The type is guessed from the content as on ingestion; ZIP/OLE are tried with all matching
//! MIME types.
#![no_main]
use libfuzzer_sys::fuzz_target;
include!("sancov_coff.rs");
use pl_parsers::ParseContext;

const ZIP_TYPES: &[&str] = &[
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.oasis.opendocument.text",
    "application/vnd.oasis.opendocument.spreadsheet",
    "application/vnd.oasis.opendocument.presentation",
];
const OLE_TYPES: &[&str] = &["application/msword", "application/vnd.ms-excel", "application/vnd.ms-powerpoint"];

fuzz_target!(|data: &[u8]| {
    let ctx = ParseContext::default();
    let types: &[&str] = if data.starts_with(b"PK") {
        ZIP_TYPES
    } else if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        OLE_TYPES
    } else {
        &["text/rtf"]
    };
    for m in types {
        if let Ok(c) = pl_parsers::office::extract(data, m, &ctx) {
            let _ = c;
        }
        if pl_parsers::office::is_presentation(m) {
            if let Ok(s) = pl_parsers::office::slides(data, m, &ctx) {
                let _ = pl_parsers::office::slides_html(&s);
            }
        }
    }
});
