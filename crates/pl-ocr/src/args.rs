// upstream: src/paperless/parsers/tesseract.py@v3.4.2 (RasterisedDocumentParser.construct_ocrmypdf_parameters)
//
// Result is an ordered map like the Python dict (insertion order; `{**args, **user_args}`
// keeps the position of existing keys and appends new ones at the end).

use serde_json::{Map, Value, json};

use crate::config::{Clean, Mode, OcrConfig};
use crate::imageinfo::is_image;
use crate::{OcrError, Result};

/// OCRmyPDF arguments that the parser sets itself (own temp files, read back after OCR);
/// values from `user_args` for them are ignored (warning). Sorted like Python's `sorted(...)`.
///
/// upstream fix d05e96cda (#14734, paperless-ngx `dev` after v3.4.2): `paperless/config.py`
/// `OCR_RESERVED_USER_ARGS`, `parsers/tesseract.py::construct_ocrmypdf_parameters`. Same list
/// as `pl_system::config_write::OCR_RESERVED_USER_ARGS` (API validation).
pub const OCR_RESERVED_USER_ARGS: &[&str] = &["input_file_or_options", "output_file", "sidecar"];

/// The arguments passed to `ocrmypdf.ocr(**args)`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OcrArgs(pub Map<String, Value>);

impl OcrArgs {
    pub fn get(&self, k: &str) -> Option<&Value> {
        self.0.get(k)
    }
    pub fn flag(&self, k: &str) -> bool {
        self.0.get(k).is_some_and(crate::config::py_truthy)
    }
    pub fn str(&self, k: &str) -> Option<&str> {
        self.0.get(k).and_then(Value::as_str)
    }
    pub fn f64(&self, k: &str) -> Option<f64> {
        self.0.get(k).and_then(Value::as_f64)
    }
    pub fn keys(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
    pub fn to_json(&self) -> Value {
        Value::Object(self.0.clone())
    }
}

/// Image helper functions that `construct_ocrmypdf_parameters` calls for images
/// (`get_dpi`, `calculate_a4_dpi`, `has_alpha`, `remove_alpha`).
pub trait ImageOps {
    /// `get_dpi` (error → `None` + warning).
    fn get_dpi(&self, path: &str) -> Option<i64>;
    /// `calculate_a4_dpi` (error → `None` + warning).
    fn calculate_a4_dpi(&self, path: &str) -> Option<i64>;
    /// `has_alpha` (errors are passed on, like upstream).
    fn has_alpha(&self, path: &str) -> Result<bool>;
    /// `remove_alpha` → path of the copy without alpha (`<tempdir>/image-no-alpha`).
    fn remove_alpha(&self, path: &str) -> Result<String>;
}

#[derive(Debug, Clone, Default)]
pub struct ArgsRequest<'a> {
    pub input_file: &'a str,
    pub mime_type: &'a str,
    pub output_file: &'a str,
    pub sidecar_file: &'a str,
    pub safe_fallback: bool,
    pub skip_text: bool,
}

/// `construct_ocrmypdf_parameters`.
pub fn construct_ocrmypdf_parameters(cfg: &OcrConfig, req: &ArgsRequest<'_>, images: &dyn ImageOps) -> Result<OcrArgs> {
    let mut a = Map::new();
    a.insert("input_file_or_options".into(), json!(req.input_file));
    a.insert("output_file".into(), json!(req.output_file));
    a.insert("use_threads".into(), json!(true));
    a.insert("jobs".into(), json!(cfg.threads_per_worker));
    a.insert("language".into(), json!(cfg.language));
    a.insert("output_type".into(), json!(cfg.output_type.as_str()));
    a.insert("progress_bar".into(), json!(false));

    if cfg.output_type.is_pdfa() {
        a.insert("color_conversion_strategy".into(), json!(cfg.color_conversion_strategy.as_str()));
    }

    if req.safe_fallback || cfg.mode == Mode::Force {
        a.insert("force_ocr".into(), json!(true));
    } else if cfg.mode == Mode::Redo {
        a.insert("redo_ocr".into(), json!(true));
    } else if req.skip_text || cfg.mode == Mode::Off {
        a.insert("skip_text".into(), json!(true));
    }
    // Mode::Auto: no extra flag

    match cfg.clean {
        Clean::Clean => {
            a.insert("clean".into(), json!(true));
        }
        Clean::Final => {
            if cfg.mode == Mode::Redo {
                a.insert("clean".into(), json!(true));
            } else {
                // --clean-final is not compatible with --redo-ocr
                a.insert("clean_final".into(), json!(true));
            }
        }
        Clean::None => {}
    }

    if cfg.deskew && cfg.mode != Mode::Redo {
        a.insert("deskew".into(), json!(true));
    }

    if cfg.rotate {
        a.insert("rotate_pages".into(), json!(true));
        a.insert("rotate_pages_threshold".into(), json!(cfg.rotate_threshold));
    }

    match cfg.pages {
        Some(p) if p > 0 => {
            a.insert("pages".into(), json!(format!("1-{p}")));
        }
        _ => {
            // sidecar is incompatible with pages
            a.insert("sidecar".into(), json!(req.sidecar_file));
        }
    }

    if is_image(req.mime_type) {
        let dpi = images.get_dpi(req.input_file);
        let a4_dpi = images.calculate_a4_dpi(req.input_file);

        if images.has_alpha(req.input_file)? {
            tracing::info!("Removing alpha layer from {} for compatibility with img2pdf", req.input_file);
            let no_alpha = images.remove_alpha(req.input_file)?;
            a.insert("input_file_or_options".into(), json!(no_alpha));
        }

        let chosen = match (dpi, cfg.image_dpi, a4_dpi) {
            (Some(d), _, _) if d != 0 => {
                tracing::debug!("Detected DPI for image {}: {d}", req.input_file);
                d
            }
            (_, Some(d), _) => d,
            (_, None, Some(d)) if d != 0 => d,
            _ => {
                return Err(OcrError::Parse(format!(
                    "Cannot produce archive PDF for image {}, no DPI information is present in this image and OCR_IMAGE_DPI is not set.",
                    req.input_file
                )));
            }
        };
        a.insert("image_dpi".into(), json!(chosen));
        if chosen < 70 {
            tracing::warn!("Image DPI of {chosen} is low, OCR may fail");
        }
    }

    if let Some(user) = &cfg.user_args {
        match user {
            Value::Object(m) => {
                // upstream fix d05e96cda (#14734): arguments the parser manages itself are
                // ignored (warning), the rest still applies.
                let reserved: Vec<&str> = OCR_RESERVED_USER_ARGS.iter().copied().filter(|k| m.contains_key(*k)).collect();
                if !reserved.is_empty() {
                    tracing::warn!("Ignoring OCR user arguments managed by PaperRust: {}", reserved.join(", "));
                }
                for (k, v) in m {
                    if !OCR_RESERVED_USER_ARGS.contains(&k.as_str()) {
                        a.insert(k.clone(), v.clone());
                    }
                }
            }
            other => tracing::warn!(
                "There is an issue with PAPERRUST_OCR_USER_ARGS, so they will not be used. Error: {}",
                py_mapping_error(other)
            ),
        }
    }

    if let Some(px) = cfg.max_image_pixel
        && px >= 0.0
    {
        let mp = px / 1_000_000.0;
        if mp == 0.0 {
            tracing::debug!("OCR pixel limit is disabled!");
        } else {
            tracing::debug!("Calculated {mp} megapixels for OCR");
        }
        a.insert("max_image_mpixels".into(), json!(mp));
    }

    Ok(OcrArgs(a))
}

/// Text of the Python exception for `{**args, **x}` with a non-dict.
fn py_mapping_error(v: &Value) -> String {
    let t = match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    };
    format!("'{t}' object is not a mapping")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfigOverrides, OcrSettings, OutputType};
    use std::cell::RefCell;

    struct FakeImages {
        dpi: Option<i64>,
        a4: Option<i64>,
        alpha: bool,
        removed: RefCell<bool>,
    }
    impl ImageOps for FakeImages {
        fn get_dpi(&self, _: &str) -> Option<i64> {
            self.dpi
        }
        fn calculate_a4_dpi(&self, _: &str) -> Option<i64> {
            self.a4
        }
        fn has_alpha(&self, _: &str) -> Result<bool> {
            Ok(self.alpha)
        }
        fn remove_alpha(&self, _: &str) -> Result<String> {
            *self.removed.borrow_mut() = true;
            Ok("/tmp/x/image-no-alpha".into())
        }
    }
    fn no_img() -> FakeImages {
        FakeImages { dpi: None, a4: None, alpha: false, removed: RefCell::new(false) }
    }

    /// `make_tesseract_parser(**settings)`: Settings-Overrides → OcrConfig.
    fn cfg(f: impl FnOnce(&mut OcrSettings)) -> OcrConfig {
        let mut s = OcrSettings { threads_per_worker: 1, ..Default::default() };
        f(&mut s);
        OcrConfig::new(&s, &AppConfigOverrides::default()).unwrap()
    }
    fn cfg_db(app: AppConfigOverrides, f: impl FnOnce(&mut OcrSettings)) -> OcrConfig {
        let mut s = OcrSettings { threads_per_worker: 1, ..Default::default() };
        f(&mut s);
        OcrConfig::new(&s, &app).unwrap()
    }
    fn params(c: &OcrConfig) -> OcrArgs {
        let r = ArgsRequest { input_file: "", mime_type: "", output_file: "", sidecar_file: "", ..Default::default() };
        construct_ocrmypdf_parameters(c, &r, &no_img()).unwrap()
    }
    fn params_flags(c: &OcrConfig, safe_fallback: bool, skip_text: bool) -> OcrArgs {
        let r = ArgsRequest { safe_fallback, skip_text, mime_type: "application/pdf", ..Default::default() };
        construct_ocrmypdf_parameters(c, &r, &no_img()).unwrap()
    }

    // --- upstream: tests/parsers/test_tesseract_parser.py::TestOcrmypdfParameters ---
    #[test]
    fn basic_parameter_mapping() {
        let r = ArgsRequest {
            input_file: "input.pdf",
            output_file: "output.pdf",
            sidecar_file: "sidecar.txt",
            mime_type: "application/pdf",
            ..Default::default()
        };
        let p = construct_ocrmypdf_parameters(&cfg(|_| {}), &r, &no_img()).unwrap();
        assert_eq!(p.str("input_file_or_options"), Some("input.pdf"));
        assert_eq!(p.str("output_file"), Some("output.pdf"));
        assert_eq!(p.str("sidecar"), Some("sidecar.txt"));
        assert_eq!(
            p.keys(),
            vec![
                "input_file_or_options",
                "output_file",
                "use_threads",
                "jobs",
                "language",
                "output_type",
                "progress_bar",
                "color_conversion_strategy",
                "clean",
                "deskew",
                "rotate_pages",
                "rotate_pages_threshold",
                "sidecar"
            ]
        );
    }

    #[test]
    fn clean_option() {
        let p = params(&cfg(|s| s.ocr_clean = "none".into()));
        assert!(!p.0.contains_key("clean") && !p.0.contains_key("clean_final"));
        let p = params(&cfg(|s| s.ocr_clean = "clean".into()));
        assert!(p.0.contains_key("clean") && !p.0.contains_key("clean_final"));
    }

    #[test]
    fn clean_final_auto_and_redo() {
        let p = params(&cfg(|s| {
            s.ocr_clean = "clean-final".into();
            s.ocr_mode = Mode::Auto;
        }));
        assert_eq!(p.get("clean_final"), Some(&json!(true)));
        assert!(!p.0.contains_key("clean"));
        let p = params(&cfg(|s| {
            s.ocr_clean = "clean-final".into();
            s.ocr_mode = Mode::Redo;
        }));
        assert_eq!(p.get("clean"), Some(&json!(true)));
        assert!(!p.0.contains_key("clean_final"));
    }

    #[test]
    fn deskew_option() {
        for (mode, deskew, expect) in [(Mode::Auto, true, true), (Mode::Redo, true, false), (Mode::Auto, false, false)] {
            let p = params(&cfg(|s| {
                s.ocr_mode = mode;
                s.ocr_deskew = deskew;
            }));
            assert_eq!(p.0.contains_key("deskew"), expect, "{mode:?} {deskew}");
        }
    }

    #[test]
    fn max_image_pixels() {
        let p = params(&cfg(|s| s.ocr_max_image_pixels = Some(1_000_001)));
        assert!((p.f64("max_image_mpixels").unwrap() - 1.0).abs() < 1e-4);
        let p = params(&cfg(|s| s.ocr_max_image_pixels = Some(-1_000_001)));
        assert!(!p.0.contains_key("max_image_mpixels"));
        let p = params(&cfg(|s| s.ocr_max_image_pixels = Some(0)));
        assert_eq!(p.f64("max_image_mpixels"), Some(0.0));
    }

    // --- upstream: tests/parsers/test_tesseract_custom_settings.py (DB values beat environment) ---
    #[test]
    fn db_settings_ocr_pages() {
        let c = cfg_db(AppConfigOverrides { pages: Some(5), ..Default::default() }, |s| s.ocr_pages = Some(10));
        assert_eq!(params(&c).str("pages"), Some("1-5"));
        assert!(!params(&c).0.contains_key("sidecar"));
    }
    #[test]
    fn db_settings_ocr_language() {
        let c =
            cfg_db(AppConfigOverrides { language: Some("fra+ita".into()), ..Default::default() }, |s| s.ocr_language = "eng+deu".into());
        assert_eq!(params(&c).str("language"), Some("fra+ita"));
    }
    #[test]
    fn db_settings_ocr_output_type() {
        let c =
            cfg_db(AppConfigOverrides { output_type: Some("pdfa".into()), ..Default::default() }, |s| s.ocr_output_type = "pdfa-3".into());
        assert_eq!(params(&c).str("output_type"), Some("pdfa"));
        assert_eq!(c.output_type, OutputType::PdfA);
    }
    #[test]
    fn db_settings_ocr_mode() {
        // AUTO from the DB + skip_text=True explicit → skip_text
        let c = cfg_db(AppConfigOverrides { mode: Some("auto".into()), ..Default::default() }, |s| s.ocr_mode = Mode::Redo);
        let p = params_flags(&c, false, true);
        assert!(p.flag("skip_text"));
        assert!(!p.0.contains_key("redo_ocr") && !p.0.contains_key("force_ocr"));
        // AUTO alone → no flag
        let p = params(&c);
        assert!(!p.0.contains_key("skip_text") && !p.0.contains_key("redo_ocr") && !p.0.contains_key("force_ocr"));
        // OFF → skip_text
        let c = cfg_db(AppConfigOverrides { mode: Some("off".into()), ..Default::default() }, |s| s.ocr_mode = Mode::Redo);
        assert!(params(&c).flag("skip_text"));
    }
    #[test]
    fn db_settings_ocr_clean() {
        let c = cfg_db(AppConfigOverrides { unpaper_clean: Some("clean".into()), ..Default::default() }, |s| {
            s.ocr_clean = "clean-final".into()
        });
        let p = params(&c);
        assert!(p.flag("clean") && !p.0.contains_key("clean_final"));
        let c = cfg_db(AppConfigOverrides { unpaper_clean: Some("clean-final".into()), ..Default::default() }, |s| {
            s.ocr_clean = "clean-final".into()
        });
        let p = params(&c);
        assert!(p.flag("clean_final") && !p.0.contains_key("clean"));
    }
    #[test]
    fn db_settings_ocr_deskew() {
        let c = cfg_db(AppConfigOverrides { deskew: Some(true), ..Default::default() }, |s| s.ocr_deskew = false);
        assert!(params(&c).flag("deskew"));
    }
    #[test]
    fn db_settings_ocr_rotate() {
        let c = cfg_db(AppConfigOverrides { rotate_pages: Some(true), rotate_pages_threshold: Some(15.0), ..Default::default() }, |s| {
            s.ocr_rotate_pages = false;
            s.ocr_rotate_pages_threshold = 30.0;
        });
        let p = params(&c);
        assert!(p.flag("rotate_pages"));
        assert_eq!(p.f64("rotate_pages_threshold"), Some(15.0));
    }
    #[test]
    fn db_settings_ocr_max_pixels() {
        let c = cfg_db(AppConfigOverrides { max_image_pixels: Some(1_000_000.0), ..Default::default() }, |s| {
            s.ocr_max_image_pixels = Some(2_000_000)
        });
        assert_eq!(params(&c).f64("max_image_mpixels"), Some(1.0));
    }
    #[test]
    fn db_settings_ocr_color_convert() {
        let c =
            cfg_db(AppConfigOverrides { color_conversion_strategy: Some("UseDeviceIndependentColor".into()), ..Default::default() }, |s| {
                s.ocr_color_conversion_strategy = "LeaveColorUnchanged".into()
            });
        assert_eq!(params(&c).str("color_conversion_strategy"), Some("UseDeviceIndependentColor"));
        // without PDF/A no color_conversion_strategy
        let c = cfg(|s| s.ocr_output_type = "pdf".into());
        assert!(!params(&c).0.contains_key("color_conversion_strategy"));
    }
    #[test]
    fn ocr_user_args() {
        // DB value beats environment
        let c = cfg_db(AppConfigOverrides { user_args: Some(json!({"unpaper_args": "--pre-rotate 90"})), ..Default::default() }, |s| {
            s.ocr_user_args = Some(r#"{"continue_on_soft_render_error": true}"#.into())
        });
        let p = params(&c);
        assert_eq!(p.str("unpaper_args"), Some("--pre-rotate 90"));
        assert!(!p.0.contains_key("continue_on_soft_render_error"));
        let c = cfg(|s| s.ocr_user_args = Some(r#"{"continue_on_soft_render_error": true, "language": "deu"}"#.into()));
        let p = params(&c);
        assert!(p.flag("continue_on_soft_render_error"));
        // existing key keeps its position, value overwritten
        assert_eq!(p.keys()[4], "language");
        assert_eq!(p.str("language"), Some("deu"));
        // non-dict → warning, ignored
        let c = cfg(|s| s.ocr_user_args = Some("[1, 2]".into()));
        assert_eq!(params(&c).keys().len(), 13);
    }

    /// upstream fix d05e96cda (#14734): `test_ocr_user_args_cannot_override_reserved`.
    #[test]
    fn ocr_user_args_cannot_override_reserved() {
        let c = cfg_db(
            AppConfigOverrides {
                user_args: Some(json!({
                    "sidecar": "/tmp/evil.py",
                    "output_file": "/tmp/out.pdf",
                    "input_file_or_options": "/etc/passwd",
                    "unpaper_args": "--pre-rotate 90",
                })),
                ..Default::default()
            },
            |_| {},
        );
        let r = ArgsRequest {
            input_file: "input.pdf",
            output_file: "output.pdf",
            sidecar_file: "sidecar.txt",
            mime_type: "application/pdf",
            ..Default::default()
        };
        let p = construct_ocrmypdf_parameters(&c, &r, &no_img()).unwrap();
        assert_eq!(p.str("sidecar"), Some("sidecar.txt"));
        assert_eq!(p.str("output_file"), Some("output.pdf"));
        assert_eq!(p.str("input_file_or_options"), Some("input.pdf"));
        assert_eq!(p.str("unpaper_args"), Some("--pre-rotate 90"));
    }

    #[test]
    fn mode_flags_and_fallback() {
        let p = params_flags(&cfg(|s| s.ocr_mode = Mode::Redo), true, false);
        assert!(p.flag("force_ocr") && !p.0.contains_key("redo_ocr"));
        let r = ArgsRequest { skip_text: true, ..Default::default() };
        let p = construct_ocrmypdf_parameters(&cfg(|_| {}), &r, &no_img()).unwrap();
        assert!(p.flag("skip_text"));
        let p = params(&cfg(|s| s.ocr_mode = Mode::Force));
        assert!(p.flag("force_ocr"));
    }

    // --- images: DPI selection, alpha ---
    #[test]
    fn image_dpi_selection() {
        let r = ArgsRequest { input_file: "a.png", mime_type: "image/png", ..Default::default() };
        let imgs = FakeImages { dpi: Some(300), a4: Some(62), alpha: true, removed: RefCell::new(false) };
        let p = construct_ocrmypdf_parameters(&cfg(|s| s.ocr_image_dpi = Some(150)), &r, &imgs).unwrap();
        assert_eq!(p.get("image_dpi"), Some(&json!(300)));
        assert_eq!(p.str("input_file_or_options"), Some("/tmp/x/image-no-alpha"));
        assert!(*imgs.removed.borrow());
        // no DPI → setting
        let imgs = FakeImages { dpi: None, a4: Some(62), alpha: false, removed: RefCell::new(false) };
        let p = construct_ocrmypdf_parameters(&cfg(|s| s.ocr_image_dpi = Some(150)), &r, &imgs).unwrap();
        assert_eq!(p.get("image_dpi"), Some(&json!(150)));
        // no DPI, no setting → A4 estimate
        let p = construct_ocrmypdf_parameters(&cfg(|_| {}), &r, &imgs).unwrap();
        assert_eq!(p.get("image_dpi"), Some(&json!(62)));
        // DPI 0 counts as "none"
        let imgs = FakeImages { dpi: Some(0), a4: Some(62), alpha: false, removed: RefCell::new(false) };
        let p = construct_ocrmypdf_parameters(&cfg(|_| {}), &r, &imgs).unwrap();
        assert_eq!(p.get("image_dpi"), Some(&json!(62)));
        // test_no_dpi_no_fallback_raises
        let imgs = FakeImages { dpi: None, a4: None, alpha: false, removed: RefCell::new(false) };
        let e = construct_ocrmypdf_parameters(&cfg(|_| {}), &r, &imgs).unwrap_err();
        assert_eq!(
            e.message(),
            "Cannot produce archive PDF for image a.png, no DPI information is present in this image and OCR_IMAGE_DPI is not set."
        );
        // PDF never gets image_dpi
        let r = ArgsRequest { mime_type: "application/pdf", ..Default::default() };
        let p = construct_ocrmypdf_parameters(&cfg(|s| s.ocr_image_dpi = Some(150)), &r, &no_img()).unwrap();
        assert!(!p.0.contains_key("image_dpi"));
    }
}
