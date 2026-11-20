//! Export/import ZIP (pl-cli `export::zip::ZipReader`): read the central directory and extract like
//! `document_importer` – with a check that nothing lands outside the target folder (zip slip).
#![no_main]
use libfuzzer_sys::fuzz_target;
include!("sancov_coff.rs");
use pl_cli::export::zip::ZipReader;

fuzz_target!(|data: &[u8]| {
    let Ok(tmp) = tempfile::tempdir() else { return };
    let src = tmp.path().join("in.zip");
    if std::fs::write(&src, data).is_err() {
        return;
    }
    let out = tmp.path().join("out");
    let _ = std::fs::create_dir_all(&out);
    let Ok(mut z) = ZipReader::open(&src) else { return };
    let _ = z.unsupported_methods();
    let _ = z.extract_all_limited(&out, 4 << 20);
    // Zip slip: apart from in.zip and out/ nothing may have been created in the root folder
    if let Ok(rd) = std::fs::read_dir(tmp.path()) {
        for e in rd.flatten() {
            let n = e.file_name();
            assert!(n == "in.zip" || n == "out", "zip-slip: {n:?} outside the target folder");
        }
    }
});
