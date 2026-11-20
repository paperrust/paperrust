//! Windows OCR (`Windows.Media.Ocr`) with real WinRT. Without Windows, without the WinRT class or
//! without an installed OCR language pack every test ends immediately with `SKIP` on stderr.

#[cfg(windows)]
mod win {
    use std::path::{Path, PathBuf};

    use pl_ocr::config::OcrConfig;
    use pl_ocr::engine::{OcrEngine, OcrRequest};
    use pl_ocr::engine_select::{EngineChoice, select_engine, tess_for_tag};
    use pl_ocr::native::{NativeOcrmypdf, parse_document};
    use pl_ocr::render::{PdfiumRenderer, Raster};
    use pl_ocr::winocr::WindowsOcr;

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn sample(name: &str) -> PathBuf {
        repo().join("upstream/src/paperless/tests/samples/tesseract").join(name)
    }

    /// Engine + Tesseract code of an installed language.
    fn win() -> Option<(WindowsOcr, String)> {
        let w = match WindowsOcr::new(None) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("SKIP: Windows OCR not available: {e}");
                return None;
            }
        };
        let tags = WindowsOcr::installed_tags().ok()?;
        match tags.iter().find_map(|t| tess_for_tag(t)) {
            Some(code) => Some((w, code.to_string())),
            None => {
                eprintln!("SKIP: no Windows OCR language pack installed ({tags:?})");
                None
            }
        }
    }

    fn raster(name: &str) -> Raster {
        Raster::from_dynamic(&image::open(sample(name)).unwrap(), 300.0)
    }

    fn req(lang: &str) -> OcrRequest {
        OcrRequest { languages: lang.into(), dpi: Some(300), timeout: Some(std::time::Duration::from_secs(60)), ..Default::default() }
    }

    #[test]
    fn recognizes_upstream_sample_with_boxes_and_lines() {
        let Some((w, lang)) = win() else { return };
        let r = raster("simple.png");
        let tmp = tempfile::tempdir().unwrap();
        let p = w.recognize_raster(&r, &tmp.path().join("x.tif"), &req(&lang)).unwrap();
        let text = pl_ocr::text::post_process_text(Some(&p.text)).unwrap();
        assert!(text.to_lowercase().contains("test document"), "{text:?}");
        assert_eq!((p.width, p.height), (r.width, r.height));
        assert!(p.words.len() >= 4, "{:?}", p.words);
        for wd in &p.words {
            assert!(wd.x0 < wd.x1 && wd.y0 < wd.y1, "{wd:?}");
            assert!(wd.x1 <= f64::from(r.width) + 1.0 && wd.y1 <= f64::from(r.height) + 1.0, "{wd:?}");
        }
        assert!(!p.lines().is_empty());
        // file variant (recognize) delivers the same
        let q = w.recognize(&sample("simple.png"), &req(&lang)).unwrap();
        assert_eq!(q.text, p.text);
        assert!(w.name().contains("windows-ocr"));
        assert!(w.languages().unwrap().contains(&lang));
    }

    #[test]
    fn concurrent_equals_serial() {
        let Some((w, lang)) = win() else { return };
        let rs = [raster("simple.png"), raster("simple.jpg"), raster("simple.tif")];
        let tmp = tempfile::tempdir().unwrap();
        let serial: Vec<_> = rs.iter().map(|r| w.recognize_raster(r, &tmp.path().join("s.tif"), &req(&lang)).unwrap()).collect();
        let par: Vec<Vec<_>> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..6)
                .map(|t| {
                    let (w, rs, lang) = (&w, &rs, &lang);
                    s.spawn(move || rs.iter().map(|r| w.recognize_raster(r, Path::new(&format!("unused-{t}.tif")), &req(lang)).unwrap()).collect())
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for p in par {
            assert_eq!(p, serial);
        }
    }

    #[test]
    fn missing_language_pack_is_reported_clearly() {
        let Some((w, lang)) = win() else { return };
        let installed = w.languages().unwrap();
        let Some(missing) = ["kor", "jpn", "chi_sim", "ara", "ell", "rus"].into_iter().find(|c| !installed.iter().any(|i| i == c)) else {
            eprintln!("SKIP: all probe languages installed");
            return;
        };
        let e = w.check_languages(missing).unwrap_err().to_string();
        assert!(e.contains("MissingDependencyError") && e.contains(missing) && e.contains("Add-WindowsCapability"), "{e}");
        // same message on recognition
        let e = w.recognize_raster(&raster("simple.png"), Path::new("unused.tif"), &req(missing)).unwrap_err().to_string();
        assert!(e.contains(missing), "{e}");
        // language of the same script without pack: allowed (via the installed model)
        let latin = ["eng", "fra", "deu", "ita", "spa"].into_iter().find(|c| !installed.iter().any(|i| i == c));
        if let Some(l) = latin
            && pl_ocr::engine_select::script_of(&lang) == Some(pl_ocr::engine_select::Script::Latin)
        {
            w.check_languages(&format!("{lang}+{l}")).unwrap();
        }
        // Unknown code
        assert!(w.check_languages("frk").unwrap_err().to_string().contains("does not support"));
    }

    #[test]
    fn selection_windows_and_default() {
        let Some((_, lang)) = win() else { return };
        let e = select_engine(EngineChoice::Windows, None, &lang).unwrap();
        assert!(e.name().contains("windows-ocr"), "{}", e.name());
        let e = select_engine(EngineChoice::Auto, None, &lang).unwrap();
        assert!(e.name().contains("windows-ocr"), "{}", e.name());
        // default stays Tesseract (even if Windows OCR is present)
        let t = pl_ocr::engine::TesseractCli::new("tesseract-not-run-here");
        let e = select_engine(EngineChoice::Tesseract, Some(t), &lang).unwrap();
        assert!(e.name().starts_with("tesseract"), "{}", e.name());
    }

    #[test]
    fn ingestion_path_with_windows_engine() {
        let Some((w, lang)) = win() else { return };
        let pdfium = std::env::var_os("PAPERRUST_PDFIUM_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll"));
        let Ok(pdfium) = PdfiumRenderer::load(&pdfium) else {
            eprintln!("SKIP ingestion_path_with_windows_engine: pdfium not available");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeOcrmypdf::new(Some(Box::new(w)), Box::new(pdfium), tmp.path().join("pages"));
        let mut cfg = OcrConfig::defaults();
        cfg.language = lang;
        cfg.threads_per_worker = 2;
        let r = parse_document(&backend, &cfg, tmp.path(), &sample("simple.png"), "image/png", true).unwrap();
        assert!(r.text.to_lowercase().contains("test document"), "{:?}", r.text);
        let archive = std::fs::read(r.archive_path.unwrap()).unwrap();
        let layer = pl_ocr::text::post_process_text(pl_ocr::pdfwrite::extract_text(&archive).as_deref()).unwrap_or_default();
        assert!(layer.to_lowercase().contains("test document"), "{layer:?}");
        // multi-page (TIFF) via the parallel pipeline
        let r = parse_document(&backend, &cfg, tmp.path(), &sample("multi-page-images.tiff"), "image/tiff", true).unwrap();
        let t = r.text.to_lowercase();
        let (a, b, c) = (t.find("page 1"), t.find("page 2"), t.find("page 3"));
        assert!(a.is_some() && a < b && b < c, "{t:?}");
    }
}

#[cfg(not(windows))]
#[test]
fn windows_ocr_skipped_on_this_platform() {
    eprintln!("SKIP: Windows OCR tests need Windows");
}
