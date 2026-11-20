//! E3–E6: PDF/A cleanup and self-check, color conversion, archive size reduction,
//! OCRmyPDF options from `OCR_USER_ARGS`. Without external tools; with PDFium (if present)
//! JBIG2/Indexed/ICC images are additionally rendered and compared pixel by pixel.

use std::path::PathBuf;
use std::sync::OnceLock;

use lopdf::{Dictionary, Document, Object, Stream, StringFormat};
use pl_ocr::config::{ColorConversion, OutputType};
use pl_ocr::engine::{PageOcr, Word};
use pl_ocr::optimize::OptimizeOptions;
use pl_ocr::pdfwrite::{self, ImagePageSpec, OutputMeta, PageImage};
use pl_ocr::render::{PdfRenderer, PdfiumRenderer, Raster};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pdfium() -> Option<PdfiumRenderer> {
    static R: OnceLock<Option<PdfiumRenderer>> = OnceLock::new();
    R.get_or_init(|| {
        let p = std::env::var_os("PAPERRUST_PDFIUM_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo().join("reference/venv/Lib/site-packages/pypdfium2_raw/pdfium.dll"));
        PdfiumRenderer::load(&p).ok()
    })
    .clone()
}

fn word(t: &str, x0: f64, y0: f64, x1: f64, y1: f64, line: u32) -> Word {
    Word { text: t.into(), x0, y0, x1, y1, conf: 90.0, line: (1, 1, line) }
}

fn ocr(w: u32, h: u32) -> PageOcr {
    PageOcr { width: w, height: h, words: vec![word("Rechnung", 50.0, 50.0, 250.0, 90.0, 1), word("Größe€", 300.0, 50.0, 450.0, 90.0, 1)], text: String::new() }
}

/// Black/white test page (text-like bars) as an RGB raster.
fn bitonal_rgb(w: u32, h: u32) -> Raster {
    // Lines made of "letters" of different shapes (pseudo-random), like a text page.
    let mut data = vec![255u8; (w * h * 3) as usize];
    let mut s = 99u32;
    let mut rnd = || {
        s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
        s >> 16
    };
    let mut y = 60;
    while y + 20 < h - 60 {
        let mut x = 60;
        while x + 14 < w - 60 {
            let gw = 6 + rnd() % 8;
            if rnd() % 6 != 0 {
                let shape = rnd();
                for dy in 0..18u32 {
                    for dx in 0..gw {
                        let on = match shape % 4 {
                            0 => dx < 2 || dy < 2,
                            1 => (dx + dy) % 5 < 2,
                            2 => dy > 14 || dx > gw - 3,
                            _ => (dx as i32 - gw as i32 / 2).abs() < 2 || dy % 8 < 2,
                        };
                        if on {
                            let i = (((y + dy) * w + x + dx) * 3) as usize;
                            data[i..i + 3].fill(0);
                        }
                    }
                }
            }
            x += gw + 3;
        }
        y += 30;
    }
    Raster { width: w, height: h, channels: 3, data, dpi: 150.0 }
}

fn photo_rgb(w: u32, h: u32) -> Raster {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let v = ((x as f64 / 13.0).sin() * 60.0 + (y as f64 / 17.0).cos() * 60.0 + 128.0) as u8;
            data.extend_from_slice(&[v, v.wrapping_add(40), 255 - v]);
        }
    }
    Raster { width: w, height: h, channels: 3, data, dpi: 150.0 }
}

fn meta(t: OutputType) -> OutputMeta {
    OutputMeta { output_type: t, ..Default::default() }
}

/// PDF without object streams and compression as text (for searching in all objects).
fn flat(pdf: &[u8]) -> String {
    let mut d = Document::load_mem(pdf).unwrap();
    d.decompress();
    d.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceTable;
    let mut out = Vec::new();
    d.save_to(&mut out).unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

fn has_claim(pdf: &[u8], part: u8) -> bool {
    let needle = format!("<pdfaid:part>{part}</pdfaid:part>");
    pdf.windows(needle.len()).any(|w| w == needle.as_bytes())
}

// ---------------------------------------------------------------------------------------------
// E3: PDF/A
// ---------------------------------------------------------------------------------------------

#[test]
fn ocr_output_passes_pdfa_self_check_for_all_parts() {
    for (t, part) in [(OutputType::PdfA1, 1), (OutputType::PdfA2, 2), (OutputType::PdfA3, 3), (OutputType::PdfA, 2)] {
        let spec = ImagePageSpec { image: PageImage::Raw(bitonal_rgb(600, 400)), dpi_x: 150.0, dpi_y: 150.0, rotate: 0, ocr: Some(ocr(600, 400)) };
        let pdf = pdfwrite::build_from_images(&[spec], &meta(t)).unwrap();
        let v = pl_ocr::pdfa::check(&pdf, part);
        assert!(v.is_empty(), "{t:?}: {v:?}");
        assert!(has_claim(&pdf, part), "{t:?}");
        // text layer still extractable, font embedded
        let text = pdfwrite::extract_text(&pdf).unwrap();
        assert!(text.contains("Rechnung") && text.contains("Größe€"), "{text}");
        let doc = Document::load_mem(&pdf).unwrap();
        let embedded = doc.objects.values().any(|o| o.as_dict().ok().is_some_and(|d| d.has(b"FontFile2")));
        assert!(embedded);
        if part == 1 {
            assert!(pdf.starts_with(b"%PDF-1.4"));
            assert!(!pdf.windows(7).any(|w| w == b"/ObjStm"));
        }
    }
}

/// Born-digital PDF with typical PDF/A violations.
fn dirty_pdf(with_unembedded_font: bool) -> Vec<u8> {
    let mut doc = Document::with_version("1.6");
    let pages_id = doc.new_object_id();
    // CMYK image (Flate) with SMask
    let mut cmyk = Vec::new();
    for i in 0..(20 * 20) {
        cmyk.extend_from_slice(&[(i % 255) as u8, 0, 128, 10]);
    }
    let mut mask_d = Dictionary::new();
    mask_d.set("Type", Object::Name(b"XObject".to_vec()));
    mask_d.set("Subtype", Object::Name(b"Image".to_vec()));
    mask_d.set("Width", 20);
    mask_d.set("Height", 20);
    mask_d.set("BitsPerComponent", 8);
    mask_d.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
    let mask_id = doc.add_object(Stream::new(mask_d, vec![128; 400]));
    let mut img_d = Dictionary::new();
    img_d.set("Type", Object::Name(b"XObject".to_vec()));
    img_d.set("Subtype", Object::Name(b"Image".to_vec()));
    img_d.set("Width", 20);
    img_d.set("Height", 20);
    img_d.set("BitsPerComponent", 8);
    img_d.set("ColorSpace", Object::Name(b"DeviceCMYK".to_vec()));
    img_d.set("SMask", Object::Reference(mask_id));
    img_d.set("Interpolate", true);
    let img_id = doc.add_object(Stream::new(img_d, cmyk));
    let mut gs = Dictionary::new();
    gs.set("Type", Object::Name(b"ExtGState".to_vec()));
    gs.set("ca", Object::Real(0.5));
    gs.set("BM", Object::Name(b"Multiply".to_vec()));
    gs.set("TR", Object::Name(b"Identity".to_vec()));
    let gs_id = doc.add_object(gs);
    let mut res = Dictionary::new();
    let mut xo = Dictionary::new();
    xo.set("Im1", Object::Reference(img_id));
    res.set("XObject", xo);
    let mut gsd = Dictionary::new();
    gsd.set("G1", Object::Reference(gs_id));
    res.set("ExtGState", gsd);
    let mut content = b"q /G1 gs 0 0 0 1 k 10 10 100 100 re f Q q 100 0 0 100 50 50 cm /Im1 Do Q".to_vec();
    if with_unembedded_font {
        let mut f = Dictionary::new();
        f.set("Type", Object::Name(b"Font".to_vec()));
        f.set("Subtype", Object::Name(b"Type1".to_vec()));
        // symbolic font: there is no substitute font for it (E3), so it stays not embedded
        f.set("BaseFont", Object::Name(b"ZapfDingbats".to_vec()));
        let fid = doc.add_object(f);
        let mut fd = Dictionary::new();
        fd.set("F1", Object::Reference(fid));
        res.set("Font", fd);
        content.extend_from_slice(b" BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
    }
    let cid = doc.add_object(Stream::new(Dictionary::new(), content));
    // JavaScript action, annotation without print flag and appearance
    let mut js = Dictionary::new();
    js.set("S", Object::Name(b"JavaScript".to_vec()));
    js.set("JS", Object::String(b"app.alert(1)".to_vec(), StringFormat::Literal));
    let js_id = doc.add_object(js);
    let mut annot = Dictionary::new();
    annot.set("Type", Object::Name(b"Annot".to_vec()));
    annot.set("Subtype", Object::Name(b"Square".to_vec()));
    annot.set("Rect", vec![10.into(), 10.into(), 60.into(), 60.into()]);
    annot.set("A", Object::Reference(js_id));
    let annot_id = doc.add_object(annot);
    let mut snd = Dictionary::new();
    snd.set("Type", Object::Name(b"Annot".to_vec()));
    snd.set("Subtype", Object::Name(b"Sound".to_vec()));
    snd.set("Rect", vec![0.into(), 0.into(), 5.into(), 5.into()]);
    let snd_id = doc.add_object(snd);
    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", Object::Reference(pages_id));
    page.set("MediaBox", vec![0.into(), 0.into(), 300.into(), 300.into()]);
    page.set("Resources", res);
    page.set("Contents", Object::Reference(cid));
    page.set("Annots", vec![Object::Reference(annot_id), Object::Reference(snd_id)]);
    let mut aa = Dictionary::new();
    aa.set("O", Object::Reference(js_id));
    page.set("AA", aa);
    let page_id = doc.add_object(page);
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", vec![Object::Reference(page_id)]);
    pages.set("Count", 1);
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    // catalog: OpenAction JS, Names/JavaScript + EmbeddedFiles, NeedAppearances
    let mut ef_stream_d = Dictionary::new();
    ef_stream_d.set("Type", Object::Name(b"EmbeddedFile".to_vec()));
    let ef_id = doc.add_object(Stream::new(ef_stream_d, b"hello".to_vec()));
    let mut ef = Dictionary::new();
    ef.set("F", Object::Reference(ef_id));
    let mut fs = Dictionary::new();
    fs.set("Type", Object::Name(b"Filespec".to_vec()));
    fs.set("F", Object::String(b"a.txt".to_vec(), StringFormat::Literal));
    fs.set("EF", ef);
    let fs_id = doc.add_object(fs);
    let mut efn = Dictionary::new();
    efn.set("Names", vec![Object::String(b"a.txt".to_vec(), StringFormat::Literal), Object::Reference(fs_id)]);
    let mut jsn = Dictionary::new();
    jsn.set("Names", vec![Object::String(b"x".to_vec(), StringFormat::Literal), Object::Reference(js_id)]);
    let mut names = Dictionary::new();
    names.set("EmbeddedFiles", efn);
    names.set("JavaScript", jsn);
    let mut acro = Dictionary::new();
    acro.set("Fields", Vec::<Object>::new());
    acro.set("NeedAppearances", true);
    let mut cat = Dictionary::new();
    cat.set("Type", Object::Name(b"Catalog".to_vec()));
    cat.set("Pages", Object::Reference(pages_id));
    cat.set("OpenAction", Object::Reference(js_id));
    cat.set("Names", names);
    cat.set("AcroForm", acro);
    let cat_id = doc.add_object(cat);
    doc.trailer.set("Root", Object::Reference(cat_id));
    let mut info = Dictionary::new();
    info.set("Title", Object::String(b"Testdokument".to_vec(), StringFormat::Literal));
    info.set("CreationDate", Object::String(b"kein Datum".to_vec(), StringFormat::Literal));
    let info_id = doc.add_object(info);
    doc.trailer.set("Info", Object::Reference(info_id));
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn born_digital_violations_are_fixed() {
    let raw = dirty_pdf(false);
    // before: many violations
    let before = pl_ocr::pdfa::check(&raw, 2);
    assert!(before.len() >= 5, "{before:?}");
    for (t, part) in [(OutputType::PdfA1, 1u8), (OutputType::PdfA2, 2), (OutputType::PdfA3, 3)] {
        let doc = pdfwrite::load_pdf(&raw).unwrap();
        let (pdf, rep) = pdfwrite::finish_with_report(doc, &meta(t)).unwrap();
        assert!(rep.pdfa_violations.is_empty(), "PDF/A-{part}: {:?} (fixes {:?})", rep.pdfa_violations, rep.pdfa_fixes);
        assert!(rep.pdfa_claimed && has_claim(&pdf, part));
        let d = Document::load_mem(&pdf).unwrap();
        let cat = d.catalog().unwrap();
        assert!(!cat.has(b"OpenAction"));
        let s = flat(&pdf);
        assert!(!s.contains("/JavaScript"), "PDF/A-{part}");
        assert!(!s.contains("/Sound"));
        // DeviceCMYK → RGB (k operator and image)
        assert!(!s.contains("/DeviceCMYK"));
        if part < 3 {
            assert!(!s.contains("/EmbeddedFile"), "PDF/A-{part}");
        } else {
            assert!(s.contains("/AFRelationship"));
        }
        if part == 1 {
            assert!(!s.contains("/SMask "), "soft mask flattened");
        }
        // annotation printable with appearance (PDF/A-2/3)
        let pid = *d.get_pages().values().next().unwrap();
        let annots = d.get_page_annotations(pid).unwrap();
        assert_eq!(annots.len(), 1);
        assert_eq!(annots[0].get(b"F").unwrap().as_i64().unwrap() & 4, 4);
        if part >= 2 {
            assert!(annots[0].has(b"AP"));
        }
    }
}

#[test]
fn unembedded_visible_font_drops_claim() {
    let raw = dirty_pdf(true);
    let doc = pdfwrite::load_pdf(&raw).unwrap();
    let (pdf, rep) = pdfwrite::finish_with_report(doc, &meta(OutputType::PdfA2)).unwrap();
    assert!(!rep.pdfa_claimed);
    assert!(rep.pdfa_violations.iter().any(|v| v.rule == "6.2.11.4" && v.detail.contains("ZapfDingbats")), "{:?}", rep.pdfa_violations);
    assert!(!has_claim(&pdf, 2));
    // file stays readable, text preserved
    assert!(pdfwrite::extract_text(&pdf).unwrap().contains("Hello"));
}

/// Substitute font for the test: Liberation (Linux/Docker) or Arial/Times/Courier New (Windows).
fn substitute_font(face: pl_ocr::pdfa_fonts::Face) -> Option<Vec<u8>> {
    use pl_ocr::pdfa_fonts::Family;
    if let Some(d) = pl_ocr::pdfa_fonts::load_from_dirs(face) {
        return Some(d);
    }
    let win = match face.family {
        Family::Sans => "arial.ttf",
        Family::Serif => "times.ttf",
        Family::Mono => "cour.ttf",
    };
    std::fs::read(std::path::Path::new("C:/Windows/Fonts").join(win)).ok()
}

/// Gap E3: non-embedded Helvetica (without /Widths, StandardEncoding) gets an
/// embedded substitute font; the file is then PDF/A-2b according to the self-check.
#[test]
fn unembedded_standard_font_gets_substitute() {
    if substitute_font(pl_ocr::pdfa_fonts::Face { family: pl_ocr::pdfa_fonts::Family::Sans, bold: false, italic: false }).is_none() {
        eprintln!("skipped: no Liberation/Arial font on this system");
        return;
    }
    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let mut f = Dictionary::new();
    f.set("Type", Object::Name(b"Font".to_vec()));
    f.set("Subtype", Object::Name(b"Type1".to_vec()));
    f.set("BaseFont", Object::Name(b"Helvetica-Bold".to_vec()));
    let fid = doc.add_object(f);
    let mut fd = Dictionary::new();
    fd.set("F1", Object::Reference(fid));
    let mut res = Dictionary::new();
    res.set("Font", fd);
    // 0x27 is "quoteright" in StandardEncoding (in WinAnsi it would be "quotesingle")
    let cid = doc.add_object(Stream::new(Dictionary::new(), b"BT /F1 12 Tf 72 700 Td (Hallo Welt 'x') Tj ET".to_vec()));
    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", Object::Reference(pages_id));
    page.set("MediaBox", vec![0.into(), 0.into(), 612.into(), 792.into()]);
    page.set("Resources", res);
    page.set("Contents", Object::Reference(cid));
    let pid = doc.add_object(page);
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", vec![Object::Reference(pid)]);
    pages.set("Count", 1);
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let mut cat = Dictionary::new();
    cat.set("Type", Object::Name(b"Catalog".to_vec()));
    cat.set("Pages", Object::Reference(pages_id));
    let cat_id = doc.add_object(cat);
    doc.trailer.set("Root", Object::Reference(cat_id));

    let msgs = pl_ocr::pdfa_fonts::embed_missing_with(&mut doc, &substitute_font);
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(msgs[0].contains("LiberationSans-Bold.ttf") && msgs[0].contains("Helvetica-Bold"), "{msgs:?}");
    {
        let font = doc.get_dictionary(fid).unwrap();
        assert_eq!(font.get(b"Subtype").unwrap().as_name().unwrap(), b"TrueType");
        let enc = font.get(b"Encoding").unwrap().as_dict().unwrap();
        assert_eq!(enc.get(b"BaseEncoding").unwrap().as_name().unwrap(), b"WinAnsiEncoding");
        let diffs = enc.get(b"Differences").unwrap().as_array().unwrap();
        let pos = diffs.iter().position(|o| o.as_i64().ok() == Some(0x27)).expect("0x27 in Differences");
        assert_eq!(diffs[pos + 1].as_name().unwrap(), b"quoteright");
        // widths like Helvetica-Bold/Arial Bold: "H" = 722
        let first = font.get(b"FirstChar").unwrap().as_i64().unwrap();
        let w = font.get(b"Widths").unwrap().as_array().unwrap();
        assert_eq!(w[(0x48 - first) as usize].as_i64().unwrap(), 722);
        let fdesc = doc.get_dictionary(font.get(b"FontDescriptor").unwrap().as_reference().unwrap()).unwrap();
        let ff = doc.get_object(fdesc.get(b"FontFile2").unwrap().as_reference().unwrap()).unwrap().as_stream().unwrap();
        let data = ff.decompressed_content().unwrap_or_else(|_| ff.content.clone());
        let ttf = pl_ocr::pdfa_fonts::ttf::Ttf::parse(&data).expect("subset is a valid TrueType font");
        assert_ne!(ttf.glyph(u32::from('H')), 0);
        let orig = substitute_font(pl_ocr::pdfa_fonts::Face { family: pl_ocr::pdfa_fonts::Family::Sans, bold: true, italic: false }).unwrap();
        assert!(data.len() * 3 < orig.len(), "subset {} vs font {}", data.len(), orig.len());
        eprintln!("substitute subset {} bytes (stream {} bytes) of {} bytes", data.len(), ff.content.len(), orig.len());
    }
    let (pdf, rep) = pdfwrite::finish_with_report(doc, &meta(OutputType::PdfA2)).unwrap();
    assert!(rep.pdfa_claimed, "{:?}", rep.pdfa_violations);
    assert!(has_claim(&pdf, 2));
    let text = pdfwrite::extract_text(&pdf).unwrap();
    assert!(text.contains("Hallo Welt"), "{text}");
    assert!(text.contains('\u{2019}'), "quoteright kept: {text}");
}

#[test]
fn invisible_unembedded_glyphless_font_is_replaced() {
    // text layer of a foreign OCR program: GlyphLessFont without FontFile2, only Tr 3
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut fdesc = Dictionary::new();
    fdesc.set("Type", Object::Name(b"FontDescriptor".to_vec()));
    fdesc.set("FontName", Object::Name(b"GlyphLessFont".to_vec()));
    fdesc.set("Flags", 5);
    let fd_id = doc.add_object(fdesc);
    let mut cidf = Dictionary::new();
    cidf.set("Type", Object::Name(b"Font".to_vec()));
    cidf.set("Subtype", Object::Name(b"CIDFontType2".to_vec()));
    cidf.set("BaseFont", Object::Name(b"GlyphLessFont".to_vec()));
    cidf.set("FontDescriptor", Object::Reference(fd_id));
    cidf.set("CIDToGIDMap", Object::Name(b"Identity".to_vec()));
    let cidf_id = doc.add_object(cidf);
    let mut f0 = Dictionary::new();
    f0.set("Type", Object::Name(b"Font".to_vec()));
    f0.set("Subtype", Object::Name(b"Type0".to_vec()));
    f0.set("BaseFont", Object::Name(b"GlyphLessFont".to_vec()));
    f0.set("Encoding", Object::Name(b"Identity-H".to_vec()));
    f0.set("DescendantFonts", vec![Object::Reference(cidf_id)]);
    let f0_id = doc.add_object(f0);
    let mut fonts = Dictionary::new();
    fonts.set("F0", Object::Reference(f0_id));
    let mut res = Dictionary::new();
    res.set("Font", fonts);
    let cid = doc.add_object(Stream::new(Dictionary::new(), b"BT 3 Tr /F0 10 Tf 10 10 Td <00410042> Tj ET".to_vec()));
    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", Object::Reference(pages_id));
    page.set("MediaBox", vec![0.into(), 0.into(), 100.into(), 100.into()]);
    page.set("Resources", res);
    page.set("Contents", Object::Reference(cid));
    let page_id = doc.add_object(page);
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", vec![Object::Reference(page_id)]);
    pages.set("Count", 1);
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let mut cat = Dictionary::new();
    cat.set("Type", Object::Name(b"Catalog".to_vec()));
    cat.set("Pages", Object::Reference(pages_id));
    let cat_id = doc.add_object(cat);
    doc.trailer.set("Root", Object::Reference(cat_id));
    let mut raw = Vec::new();
    doc.save_to(&mut raw).unwrap();
    // PDF/A-1 has no Tr 3 exception → conformant only with embedding
    let (pdf, rep) = pdfwrite::finish_with_report(pdfwrite::load_pdf(&raw).unwrap(), &meta(OutputType::PdfA1)).unwrap();
    assert!(rep.pdfa_violations.is_empty(), "{:?}", rep.pdfa_violations);
    assert!(rep.pdfa_fixes.iter().any(|f| f.contains("GlyphLessFont")));
    assert!(has_claim(&pdf, 1));
}

#[test]
fn check_detects_core_violations() {
    // PDF without any PDF/A structure
    let spec = ImagePageSpec { image: PageImage::Raw(photo_rgb(50, 50)), dpi_x: 72.0, dpi_y: 72.0, rotate: 0, ocr: None };
    let plain = pdfwrite::build_from_images(&[spec], &meta(OutputType::Pdf)).unwrap();
    let v = pl_ocr::pdfa::check(&plain, 2);
    let rules: Vec<&str> = v.iter().map(|x| x.rule).collect();
    assert!(rules.contains(&"6.6.2.1"), "{v:?}"); // no XMP
    assert!(rules.contains(&"6.2.3"), "{v:?}"); // no OutputIntent
    assert!(rules.contains(&"6.2.4.3"), "{v:?}"); // DeviceRGB without intent
}

// ---------------------------------------------------------------------------------------------
// E4: color conversion
// ---------------------------------------------------------------------------------------------

fn image_colorspaces(pdf: &[u8]) -> Vec<String> {
    let d = Document::load_mem(pdf).unwrap();
    d.objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| matches!(s.dict.get(b"Subtype"), Ok(Object::Name(n)) if n.as_slice() == b"Image"))
        .map(|s| match s.dict.get(b"ColorSpace") {
            Ok(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
            Ok(Object::Array(a)) => a.first().and_then(|x| x.as_name().ok()).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default(),
            _ => String::new(),
        })
        .collect()
}

#[test]
fn color_conversion_strategies() {
    let mk = |c: ColorConversion| {
        let spec = ImagePageSpec { image: PageImage::Raw(photo_rgb(120, 80)), dpi_x: 150.0, dpi_y: 150.0, rotate: 0, ocr: Some(ocr(120, 80)) };
        let m = OutputMeta { color: c, optimize: OptimizeOptions::none(), ..meta(OutputType::PdfA2) };
        let doc = pdfwrite::load_pdf(&pdfwrite::build_from_images(&[spec], &OutputMeta { output_type: OutputType::Pdf, optimize: OptimizeOptions::none(), ..Default::default() }).unwrap()).unwrap();
        pdfwrite::finish_with_report(doc, &m).unwrap()
    };
    let (gray, r) = mk(ColorConversion::Gray);
    assert_eq!(image_colorspaces(&gray), vec!["DeviceGray"]);
    assert_eq!(r.color.images_converted, 1);
    assert!(r.pdfa_violations.is_empty(), "{:?}", r.pdfa_violations);
    let (icc, r) = mk(ColorConversion::UseDeviceIndependentColor);
    assert_eq!(image_colorspaces(&icc), vec!["ICCBased"]);
    assert!(r.pdfa_violations.is_empty(), "{:?}", r.pdfa_violations);
    let (rgb, r) = mk(ColorConversion::Rgb);
    assert_eq!(image_colorspaces(&rgb), vec!["DeviceRGB"]);
    assert_eq!(r.color.images_converted, 0);
    // CMYK with PDF/A → RGB (no CMYK intent)
    let (cmyk, _) = mk(ColorConversion::Cmyk);
    assert_eq!(image_colorspaces(&cmyk), vec!["DeviceRGB"]);
    // Without PDF/A the strategy has no effect (upstream passes it on only with PDF/A)
    let spec = ImagePageSpec { image: PageImage::Raw(photo_rgb(20, 20)), dpi_x: 72.0, dpi_y: 72.0, rotate: 0, ocr: None };
    let plain = pdfwrite::build_from_images(&[spec], &OutputMeta { output_type: OutputType::Pdf, color: ColorConversion::Gray, ..Default::default() }).unwrap();
    assert_eq!(image_colorspaces(&plain), vec!["DeviceRGB"]);
    // Rendered: gray result has the same brightness as the original (PDFium)
    if let Some(r) = pdfium() {
        let a = r.open(&rgb).unwrap().render(0, 72.0, true).unwrap();
        let b = r.open(&gray).unwrap().render(0, 72.0, true).unwrap();
        let c = r.open(&icc).unwrap().render(0, 72.0, true).unwrap();
        let diff = |x: &Raster, y: &Raster| x.data.iter().zip(&y.data).map(|(p, q)| u32::from(p.abs_diff(*q))).max().unwrap_or(0);
        assert!(diff(&a, &b) <= 6, "gray {}", diff(&a, &b));
        assert!(diff(&a, &c) <= 6, "icc {}", diff(&a, &c));
    } else {
        eprintln!("SKIP render comparison: pdfium not available");
    }
}

// ---------------------------------------------------------------------------------------------
// E5: optimization
// ---------------------------------------------------------------------------------------------

fn opt_meta(level: u8) -> OutputMeta {
    OutputMeta { optimize: OptimizeOptions { level, ..Default::default() }, ..meta(OutputType::PdfA2) }
}

#[test]
fn optimize_levels_shrink_archive_and_render_identically() {
    let scan = bitonal_rgb(1240, 1754); // A4 at 150 dpi
    let photo = photo_rgb(600, 400);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95).encode(&photo.data, 600, 400, image::ExtendedColorType::Rgb8).unwrap();
    let pages = || {
        vec![
            ImagePageSpec { image: PageImage::Raw(scan.clone()), dpi_x: 150.0, dpi_y: 150.0, rotate: 0, ocr: Some(ocr(1240, 1754)) },
            ImagePageSpec { image: PageImage::Raw(photo.clone()), dpi_x: 150.0, dpi_y: 150.0, rotate: 0, ocr: None },
            ImagePageSpec { image: PageImage::Jpeg { data: jpeg.clone(), width: 600, height: 400, components: 3 }, dpi_x: 150.0, dpi_y: 150.0, rotate: 0, ocr: None },
        ]
    };
    let mut sizes = Vec::new();
    let mut pdfs = Vec::new();
    for level in 0..=3u8 {
        let (pdf, rep) = pdfwrite::finish_with_report(
            pdfwrite::load_pdf(&pdfwrite::build_from_images(&pages(), &OutputMeta { output_type: OutputType::Pdf, optimize: OptimizeOptions::none(), ..Default::default() }).unwrap()).unwrap(),
            &opt_meta(level),
        )
        .unwrap();
        assert!(rep.pdfa_violations.is_empty(), "level {level}: {:?}", rep.pdfa_violations);
        if level >= 1 {
            assert_eq!(rep.optimize.jbig2, 1, "level {level}: {:?}", rep.optimize);
        }
        if level >= 2 {
            assert!(rep.optimize.jpeg_recoded >= 1, "level {level}: {:?}", rep.optimize);
        }
        eprintln!("optimize {level}: {} bytes ({:?})", pdf.len(), rep.optimize);
        sizes.push(pdf.len());
        pdfs.push(pdf);
    }
    assert!(sizes[1] * 10 < sizes[0] * 6, "{sizes:?}");
    assert!(sizes[2] < sizes[1], "{sizes:?}");
    assert!(sizes[3] <= sizes[2], "{sizes:?}");
    // Text bleibt
    for p in &pdfs {
        assert!(pdfwrite::extract_text(p).unwrap().contains("Rechnung"));
    }
    // Lossless level 1 renders pixel-identical to level 0 (JBIG2 page and Flate photo page)
    match pdfium() {
        Some(r) => {
            for page in 0..2 {
                let a = r.open(&pdfs[0]).unwrap().render(page, 150.0, true).unwrap();
                let b = r.open(&pdfs[1]).unwrap().render(page, 150.0, true).unwrap();
                assert_eq!((a.width, a.height), (b.width, b.height));
                let diff = a.data.iter().zip(&b.data).filter(|(p, q)| p != q).count();
                assert_eq!(diff, 0, "page {page}: {diff} pixels differ between optimize 0 and 1");
            }
        }
        None => eprintln!("SKIP render comparison: pdfium not available"),
    }
}

#[test]
fn jbig2_roundtrip_through_pdfium() {
    let Some(r) = pdfium() else {
        eprintln!("SKIP: pdfium not available");
        return;
    };
    // Irregular 1-bit pattern incl. width ≠ multiple of 8
    for (w, h) in [(203u32, 97u32), (8, 8), (1000, 3)] {
        let stride = w.div_ceil(8) as usize;
        let mut packed = vec![0xFFu8; stride * h as usize];
        let mut s = 7u32;
        for y in 0..h as usize {
            for x in 0..w as usize {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
                if (s >> 16).is_multiple_of(7) || (x + y).is_multiple_of(23) {
                    packed[y * stride + x / 8] &= !(0x80 >> (x % 8));
                }
            }
        }
        let data = pl_ocr::jbig2::encode_generic(&packed, w, h).unwrap();
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let mut d = Dictionary::new();
        d.set("Type", Object::Name(b"XObject".to_vec()));
        d.set("Subtype", Object::Name(b"Image".to_vec()));
        d.set("Width", i64::from(w));
        d.set("Height", i64::from(h));
        d.set("BitsPerComponent", 1);
        d.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
        d.set("Filter", Object::Name(b"JBIG2Decode".to_vec()));
        let img = doc.add_object(Stream::new(d, data).with_compression(false));
        let mut xo = Dictionary::new();
        xo.set("I", Object::Reference(img));
        let mut res = Dictionary::new();
        res.set("XObject", xo);
        let c = doc.add_object(Stream::new(Dictionary::new(), format!("q {w} 0 0 {h} 0 0 cm /I Do Q").into_bytes()));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("MediaBox", vec![0.into(), 0.into(), i64::from(w).into(), i64::from(h).into()]);
        page.set("Resources", res);
        page.set("Contents", Object::Reference(c));
        let pid = doc.add_object(page);
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", vec![Object::Reference(pid)]);
        pages.set("Count", 1);
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let mut cat = Dictionary::new();
        cat.set("Type", Object::Name(b"Catalog".to_vec()));
        cat.set("Pages", Object::Reference(pages_id));
        let cid = doc.add_object(cat);
        doc.trailer.set("Root", Object::Reference(cid));
        let mut pdf = Vec::new();
        doc.save_to(&mut pdf).unwrap();
        let ras = r.open(&pdf).unwrap().render(0, 72.0, true).unwrap();
        assert_eq!((ras.width, ras.height), (w, h));
        let mut wrong = 0;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let bit = (packed[y * stride + x / 8] >> (7 - x % 8)) & 1;
                let px = ras.data[y * w as usize + x];
                if (bit == 1) != (px > 127) {
                    wrong += 1;
                }
            }
        }
        assert_eq!(wrong, 0, "{w}x{h}: {wrong} pixels wrong");
    }
}

// ---------------------------------------------------------------------------------------------
// E6: OCR_USER_ARGS
// ---------------------------------------------------------------------------------------------

#[test]
fn user_args_are_parsed() {
    let mut m = serde_json::Map::new();
    for (k, v) in [
        ("input_file_or_options", serde_json::json!("in.pdf")),
        ("output_file", serde_json::json!("out.pdf")),
        ("optimize", serde_json::json!(3)),
        ("jpeg_quality", serde_json::json!(60)),
        ("png_quality", serde_json::json!("50")),
        ("title", serde_json::json!("Titel")),
        ("author", serde_json::json!("Autor")),
        ("tesseract_pagesegmode", serde_json::json!(6)),
        ("tesseract_oem", serde_json::json!(1)),
        ("tesseract_thresholding", serde_json::json!("sauvola")),
        ("tesseract_config", serde_json::json!(["digits"])),
        ("user_words", serde_json::json!("/tmp/words.txt")),
        ("tesseract_downsample_large_images", serde_json::json!(true)),
        ("tesseract_downsample_above", serde_json::json!(5000)),
        ("max_ocr_image_mpixels", serde_json::json!(20)),
        ("remove_vectors", serde_json::json!(true)),
        ("pdfa_image_compression", serde_json::json!("jpeg")),
        ("mode", serde_json::json!("redo")),
        ("color_conversion_strategy", serde_json::json!("Gray")),
        ("jbig2_lossy", serde_json::json!(true)),
        ("unpaper_args", serde_json::json!("--layout double")),
    ] {
        m.insert(k.into(), v);
    }
    let o = pl_ocr::native::parse_options(&pl_ocr::OcrArgs(m)).unwrap();
    assert_eq!(o.optimize, OptimizeOptions { level: 3, jpeg_quality: 60, png_quality: 50, force_jpeg: true });
    assert_eq!(o.title.as_deref(), Some("Titel"));
    assert_eq!(o.tess.psm, Some(6));
    assert_eq!(o.tess.oem, Some(1));
    assert_eq!(o.tess.thresholding, Some(2));
    assert_eq!(o.tess.config, vec!["digits".to_string()]);
    assert!(o.tess.needs_cli());
    assert_eq!(o.ocr_max_side, Some(5000));
    assert_eq!(o.ocr_max_pixels, Some(20.0));
    assert!(o.remove_vectors);
    assert_eq!(o.mode, pl_ocr::native::ProcessingMode::Redo);
    assert_eq!(o.color, ColorConversion::Gray);
    assert!(pl_ocr::native::unsupported_reason("jbig2_lossy").is_some());
    assert!(pl_ocr::native::unsupported_reason("optimize").is_none());
    // invalid values → BadArgsError
    let mut bad = serde_json::Map::new();
    bad.insert("input_file_or_options".into(), serde_json::json!("in.pdf"));
    bad.insert("output_file".into(), serde_json::json!("out.pdf"));
    bad.insert("optimize".into(), serde_json::json!(7));
    assert!(pl_ocr::native::parse_options(&pl_ocr::OcrArgs(bad)).is_err());
}

#[test]
fn metadata_args_reach_info_and_xmp() {
    let spec = ImagePageSpec { image: PageImage::Raw(bitonal_rgb(100, 100)), dpi_x: 72.0, dpi_y: 72.0, rotate: 0, ocr: None };
    let m = OutputMeta { title: Some("Rechnung März".into()), author: Some("Ä & B".into()), keywords: Some("a, b".into()), ..meta(OutputType::PdfA2) };
    let (pdf, rep) = pdfwrite::finish_with_report(pdfwrite::load_pdf(&pdfwrite::build_from_images(&[spec], &OutputMeta { output_type: OutputType::Pdf, ..Default::default() }).unwrap()).unwrap(), &m).unwrap();
    assert!(rep.pdfa_violations.is_empty(), "{:?}", rep.pdfa_violations);
    let s = String::from_utf8_lossy(&pdf);
    assert!(s.contains("<rdf:li xml:lang=\"x-default\">Rechnung März</rdf:li>"));
    assert!(s.contains("Ä &amp; B"));
}

/// Real sample files: archive size per optimization level (output for the measured values in the contract),
/// level 1 lossless (PDFium pixel-identical), PDF/A result of the conversion (`_convert_pdf_to_pdfa`).
#[test]
fn real_samples_sizes_and_pdfa() {
    let files = [
        "upstream/src/paperless_testing/sample_files/multi-page-images.pdf",
        "upstream/src/paperless_testing/sample_files/simple-digital.pdf",
        "upstream/src/paperless_testing/sample_files/multi-page-digital.pdf",
        "upstream/src/paperless_testing/sample_files/with-form.pdf",
        "upstream/src/paperless/tests/samples/tesseract/rotated.pdf",
        "upstream/src/paperless/tests/samples/tesseract/multi-page-mixed.pdf",
        "upstream/src/paperless/tests/samples/tesseract/rtl-test.pdf",
        "results/bench/ingest/scan-16p.pdf",
    ];
    for f in files {
        let path = repo().join(f);
        let Ok(raw) = std::fs::read(&path) else {
            eprintln!("SKIP {f}: missing");
            continue;
        };
        let mut sizes = Vec::new();
        let mut outs = Vec::new();
        let mut claimed = None;
        for level in 0..=3u8 {
            let doc = pdfwrite::load_pdf(&raw).unwrap();
            let (pdf, rep) = pdfwrite::finish_with_report(doc, &opt_meta(level)).unwrap();
            if level == 1 {
                claimed = Some((rep.pdfa_claimed, rep.pdfa_violations.iter().map(ToString::to_string).collect::<Vec<_>>()));
            }
            sizes.push(pdf.len());
            outs.push(pdf);
        }
        eprintln!("SIZE {f}: original {} | optimize 0..3 {:?} | PDF/A-2 {:?}", raw.len(), sizes, claimed);
        assert!(sizes[1] <= sizes[0], "{f}: {sizes:?}");
        if let Some(r) = pdfium() {
            let a = r.open(&outs[0]).unwrap();
            let b = r.open(&outs[1]).unwrap();
            for page in 0..a.page_count().min(3) {
                let x = a.render(page, 50.0, true).unwrap();
                let y = b.render(page, 50.0, true).unwrap();
                let diff = x.data.iter().zip(&y.data).filter(|(p, q)| p != q).count();
                assert_eq!(diff, 0, "{f} page {page}: optimize 1 not lossless ({diff} px)");
            }
        }
    }
}
