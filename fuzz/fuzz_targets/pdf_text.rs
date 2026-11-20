//! PDF text extraction (pl-parsers `pdf`/`pdftext`): loading with limits, tagged?, text, pages, XMP.
#![no_main]
use libfuzzer_sys::fuzz_target;
include!("sancov_coff.rs");
use pl_parsers::Limits;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    if let Some(doc) = pl_parsers::pdf::load_document(data, &limits) {
        let _ = pl_parsers::pdf::is_tagged(&doc);
        let text = pl_parsers::pdf::extract_text_from(&doc, &limits);
        let _ = pl_parsers::pdf::post_process_text(text.as_deref());
        for (_, id) in doc.get_pages().into_iter().take(20) {
            let _ = pl_parsers::pdftext::page_text(&doc, id, 200_000);
        }
    }
    let _ = pl_parsers::pdf::xmp_entries(data, &limits);
});
