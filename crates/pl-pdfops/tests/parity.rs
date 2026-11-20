//! Rust side of the pl-pdfops parity (tests/parity/pl-pdfops, see the README there).
//!
//! `cargo test -p pl-pdfops --test parity -- --ignored --nocapture`
//! Prerequisite: `gen_reference.py` has run (creates samples/), PDFium under
//! `PAPERRUST_PDFIUM_PATH` (default: reference/venv/…/pypdfium2_raw/pdfium.dll).
//! Writes tests/parity/pl-pdfops/out/rust/results.json + PDFs; `compare.py` compares.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pl_pdfops::barcodes::{self, Barcode, BarcodeConfig, RxingReader, ScanSettings};
use pl_pdfops::{PageOperation, PageSpec, PdfMerger, PdfOpsError, ops};
use serde_json::{Value, json};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn here() -> PathBuf {
    repo().join("tests/parity/pl-pdfops")
}

fn resolve(rel: &str) -> PathBuf {
    if let Some(r) = rel.strip_prefix("S/") {
        repo().join("upstream/src/documents/tests/samples").join(r)
    } else if let Some(r) = rel.strip_prefix("G/") {
        here().join("samples").join(r)
    } else {
        repo().join(rel)
    }
}

fn norm_msg(m: &str) -> String {
    let base = format!("{}/", repo().display().to_string().replace('\\', "/").trim_start_matches("//?/"));
    m.replace('\\', "/").replace("//?/", "").replace(&base, "")
}

fn err(e: &PdfOpsError) -> Value {
    json!({"ok": false, "error_kind": e.kind(), "error": norm_msg(&e.to_string())})
}

fn tmpdir(prefix: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("{prefix}{}", std::process::id())).join(format!("{}", rand_id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rand_id() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

fn run_op(case: &Value, out: &Path) -> Value {
    let id = case["id"].as_str().unwrap();
    let op = case["op"].as_str().unwrap();
    let src = case.get("src").and_then(Value::as_str).map(resolve);
    let r: Result<Value, PdfOpsError> = (|| match op {
        "rotate" => {
            let dst = out.join(format!("{id}.pdf"));
            ops::rotate_pdf(src.as_ref().unwrap(), &dst, case["degrees"].as_i64().unwrap())?;
            Ok(json!({"ok": true, "outputs": [format!("{id}.pdf")]}))
        }
        "remove_pages" => {
            let dst = out.join(format!("{id}.pdf"));
            let pages: Vec<i64> = serde_json::from_value(case["pages"].clone()).unwrap();
            ops::remove_pages(src.as_ref().unwrap(), &dst, &pages)?;
            Ok(json!({"ok": true, "outputs": [format!("{id}.pdf")]}))
        }
        "build" => {
            let outputs: Vec<Vec<PageSpec>> = serde_json::from_value(case["outputs"].clone()).unwrap();
            let written = ops::build_pdfs(src.as_ref().unwrap(), &outputs, |i| Ok(out.join(format!("{id}_{i}.pdf"))))?;
            let names: Vec<String> = written.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
            Ok(json!({"ok": true, "outputs": names}))
        }
        "merge" => {
            let dst = out.join(format!("{id}.pdf"));
            let mut m = PdfMerger::new();
            let mut skipped = Vec::new();
            for (i, s) in case["srcs"].as_array().unwrap().iter().enumerate() {
                if let Err(e) = m.add(&resolve(s.as_str().unwrap())) {
                    skipped.push(json!({"index": i, "error_kind": e.kind()}));
                }
            }
            m.save(&dst)?;
            Ok(json!({"ok": true, "outputs": [format!("{id}.pdf")], "skipped": skipped}))
        }
        "needs_decrypt" => Ok(json!({"ok": true, "result": ops::needs_decrypt(src.as_ref().unwrap())?})),
        "decrypt" => {
            let dst = out.join(format!("{id}.pdf"));
            ops::decrypt_pdf(src.as_ref().unwrap(), || Ok(dst.clone()), case["password"].as_str().unwrap())?;
            Ok(json!({"ok": true, "outputs": [format!("{id}.pdf")]}))
        }
        "validate" => {
            let ops_: Vec<PageOperation> = serde_json::from_value(case["operations"].clone()).unwrap();
            Ok(json!({"ok": true, "result": ops::validate_page_operations(&ops_, case["single_output"].as_bool().unwrap())?}))
        }
        "tiff_to_pdf" => {
            let work = tmpdir("plpdfops-tiff-");
            let s = src.clone().unwrap();
            let in_dir = work.join("in");
            std::fs::create_dir_all(&in_dir).unwrap();
            let tmp_src = in_dir.join(s.file_name().unwrap());
            std::fs::copy(&s, &tmp_src)?;
            let pdf = pl_pdfops::convert::convert_from_tiff_to_pdf(&tmp_src, &work)?;
            std::fs::copy(&pdf, out.join(format!("{id}.pdf")))?;
            Ok(json!({"ok": true, "outputs": [format!("{id}.pdf")], "name": pdf.file_name().unwrap().to_string_lossy()}))
        }
        other => panic!("unknown op {other}"),
    })();
    r.unwrap_or_else(|e| err(&e))
}

fn config_from(v: &Value) -> BarcodeConfig {
    let mut c = BarcodeConfig { barcode_enable_tiff_support: true, barcode_store_values: true, ..Default::default() };
    let b = |k: &str, d: bool| v.get(k).and_then(Value::as_bool).unwrap_or(d);
    c.barcodes_enabled = b("barcodes_enabled", false);
    c.barcode_retain_split_pages = b("barcode_retain_split_pages", false);
    c.barcode_enable_asn = b("barcode_enable_asn", false);
    c.barcode_enable_tag = b("barcode_enable_tag", false);
    c.barcode_tag_split = b("barcode_tag_split", false);
    if let Some(s) = v.get("barcode_string").and_then(Value::as_str) {
        c.barcode_string = s.into();
    }
    if let Some(s) = v.get("barcode_asn_prefix").and_then(Value::as_str) {
        c.barcode_asn_prefix = s.into();
    }
    if let Some(m) = v.get("barcode_tag_mapping").and_then(Value::as_object) {
        c.barcode_tag_mapping = m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect();
    }
    c
}

fn codes_json(codes: &[Barcode]) -> Value {
    Value::Array(codes.iter().map(|c| json!([c.page, c.value, c.format])).collect())
}

fn scan(path: &Path, s: ScanSettings, work: &Path, renderer: &dyn pl_ocr::render::PdfRenderer) -> Result<(PathBuf, Vec<Barcode>), PdfOpsError> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let pdf = if ext == "tif" || ext == "tiff" { pl_pdfops::convert::convert_from_tiff_to_pdf(path, work)? } else { path.to_path_buf() };
    let t = std::time::Instant::now();
    let codes = barcodes::scan_pdf(&pdf, s, renderer, &RxingReader)?;
    eprintln!("  scanned {} in {:?}", path.display(), t.elapsed());
    Ok((pdf, codes))
}

fn run_barcodes(cases: &Value, out: &Path, renderer: &dyn pl_ocr::render::PdfRenderer) -> Value {
    let only: Option<String> = std::env::var("PLPDFOPS_ONLY").ok();
    let mut files = serde_json::Map::new();
    for rel in cases["barcode_files"].as_array().unwrap() {
        let rel = rel.as_str().unwrap();
        if only.as_deref().is_some_and(|o| !rel.contains(o)) {
            continue;
        }
        let path = resolve(rel);
        let work = tmpdir("plpdfops-bc-");
        let in_dir = work.join("in");
        std::fs::create_dir_all(&in_dir).unwrap();
        let tmp_src = in_dir.join(path.file_name().unwrap());
        std::fs::copy(&path, &tmp_src).unwrap();
        let (pdf, codes) = match scan(&tmp_src, ScanSettings { dpi: 300, upscale: 0.0, max_pages: 0 }, &work, renderer) {
            Ok(x) => x,
            Err(e) => {
                files.insert(rel.into(), err(&e));
                continue;
            }
        };
        let mut configs = serde_json::Map::new();
        for cfg_v in cases["configs"].as_array().unwrap() {
            let cfg = config_from(cfg_v);
            let cid = cfg_v["id"].as_str().unwrap();
            let seps = if cfg.barcodes_enabled { barcodes::separation_pages(&codes, &cfg) } else { Default::default() };
            let mut warnings = Vec::new();
            let asn = if cfg.barcode_enable_asn {
                match barcodes::located_asn(&codes, &cfg) {
                    Ok(a) => json!(a),
                    Err(e) => {
                        warnings.push(format!("Failed to parse ASN number because: {e}"));
                        Value::Null
                    }
                }
            } else {
                Value::Null
            };
            let tags: Vec<String> =
                if cfg.barcode_enable_tag { barcodes::tag_names(&codes, &cfg).into_iter().map(|(_, t)| t).collect() } else { Vec::new() };
            let stored: Vec<Value> = codes.iter().map(|c| serde_json::to_value(c.stored()).unwrap()).collect();
            let mut res = json!({
                "separators": seps.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                "asn": asn, "tags": tags, "stored": stored,
            });
            if !seps.is_empty() {
                let split_dir = tmpdir("plpdfops-split-");
                let stem = tmp_src.file_stem().unwrap().to_string_lossy().into_owned();
                match barcodes::separate_pages(&pdf, &seps, &stem, &split_dir, &tmp_src) {
                    Ok(parts) => {
                        let mut names = Vec::new();
                        for p in parts {
                            let stem_rel = Path::new(rel).file_stem().unwrap().to_string_lossy().into_owned();
                            let tgt = format!("bc_{stem_rel}_{cid}_{}", p.file_name().unwrap().to_string_lossy());
                            std::fs::copy(&p, out.join(&tgt)).unwrap();
                            names.push(tgt);
                        }
                        res["parts"] = json!(names);
                    }
                    Err(e) => res = err(&e),
                }
            }
            res["warnings"] = json!(warnings);
            configs.insert(cid.into(), res);
        }
        files.insert(rel.into(), json!({"ok": true, "barcodes": codes_json(&codes), "configs": configs}));
    }
    let mut variants = serde_json::Map::new();
    if std::env::var("PLPDFOPS_SKIP_VARIANTS").is_err() {
        for v in cases["scan_variants"].as_array().unwrap() {
            let s = ScanSettings {
                dpi: v["dpi"].as_u64().unwrap() as u32,
                upscale: v["upscale"].as_f64().unwrap(),
                max_pages: v.get("max_pages").and_then(Value::as_u64).unwrap_or(0) as u32,
            };
            let work = tmpdir("plpdfops-bcv-");
            let r = match scan(&resolve(v["file"].as_str().unwrap()), s, &work, renderer) {
                Ok((_, codes)) => json!({"ok": true, "barcodes": codes_json(&codes)}),
                Err(e) => err(&e),
            };
            variants.insert(v["id"].as_str().unwrap().into(), r);
        }
    }
    json!({"files": files, "variants": variants})
}

#[test]
#[ignore = "braucht gen_reference.py-Ausgaben und PDFium (tests/parity/pl-pdfops)"]
fn parity() {
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(here().join("cases.json")).unwrap()).unwrap();
    assert!(here().join("samples/outline.pdf").exists(), "zuerst gen_reference.py laufen lassen");
    let out = here().join("out/rust");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let mut ops_res = serde_json::Map::new();
    for case in cases["ops"].as_array().unwrap() {
        let r = run_op(case, &out);
        eprintln!("{} {} {}", case["id"].as_str().unwrap(), r["ok"], r.get("error").and_then(Value::as_str).unwrap_or(""));
        ops_res.insert(case["id"].as_str().unwrap().into(), r);
    }
    let mut results = json!({"ops": ops_res, "barcodes": {}});
    if std::env::var("PLPDFOPS_OPS_ONLY").is_err() {
        if std::env::var_os("PAPERRUST_PDFIUM_PATH").is_none() {
            // SAFETY: test process, before the first load of PDFium, no other threads read the environment.
            unsafe {
                std::env::set_var("PAPERRUST_PDFIUM_PATH", repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll"));
            }
        }
        let renderer: Arc<dyn pl_ocr::render::PdfRenderer> =
            Arc::new(pl_ocr::render::PdfiumRenderer::from_env().expect("PDFium (PAPERRUST_PDFIUM_PATH)"));
        results["barcodes"] = run_barcodes(&cases, &out, renderer.as_ref());
    }
    std::fs::write(out.join("results.json"), serde_json::to_string_pretty(&results).unwrap()).unwrap();
    eprintln!("written {}", out.join("results.json").display());
}
