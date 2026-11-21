//! Build a searchable PDF (pure Rust, lopdf).
//!
//! - Invisible text layer (render mode 3) at the word boxes, font "GlyphLessFont" as Type0/
//!   Identity-H with ToUnicode CMap (CID per character, hence also characters outside the BMP). The text
//!   is directly in the page content (not in a form XObject), so that even simple extractors
//!   (lopdf, pl-parsers) find it. One `BT … ET` per line, words positioned via `Tm` and
//!   stretched to the box width with `Tz` (model: Tesseract PDF renderer / OCRmyPDF hocrtransform).
//! - Image pages like img2pdf with fixed DPI (JPEG unchanged as DCTDecode, otherwise Flate).
//! - PDF/A: cleanup ([`crate::pdfa::normalize`]), color conversion ([`crate::color`]),
//!   optimization ([`crate::optimize`]), XMP (`pdfaid:part`, `pdfaid:conformance` = B, Info fields
//!   mirrored), OutputIntent `GTS_PDFA1` with sRGB profile ([`crate::icc`]), trailer `/ID`,
//!   self-check ([`crate::pdfa::check`]); limits in docs/contract/pl-ocr.md §9.

use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};

use crate::config::{ColorConversion, OutputType};
use crate::engine::PageOcr;
use crate::render::Raster;
use crate::{OcrError, Result};

const FONT_RES_NAME: &[u8] = b"PlOcrGlyphLess";

// ----------------------------------------------------------------------------------------------
// Loading / helpers
// ----------------------------------------------------------------------------------------------

/// Load a PDF. Encrypted (even with an empty password) → `EncryptedPdfError` like OCRmyPDF.
pub fn load_pdf(data: &[u8]) -> Result<Document> {
    let doc = std::panic::catch_unwind(|| Document::load_mem(data))
        .map_err(|_| OcrError::InputFile("PDF parser panicked".into()))?
        .map_err(|e| match e {
            lopdf::Error::Decryption(_) | lopdf::Error::InvalidPassword => OcrError::EncryptedPdf,
            other => OcrError::InputFile(format!("{other}")),
        })?;
    if doc.is_encrypted() || doc.trailer.has(b"Encrypt") {
        return Err(OcrError::EncryptedPdf);
    }
    Ok(doc)
}

/// Like [`load_pdf`], but PDFs decrypted with an empty password also count as encrypted
/// (OCRmyPDF: `pdf.is_encrypted` → `EncryptedPdfError`). For text extraction (pdftotext reads such
/// files) `load_pdf` is enough.
pub fn load_pdf_for_ocr(data: &[u8]) -> Result<Document> {
    let doc = load_pdf(data)?;
    if doc.was_encrypted() {
        return Err(OcrError::EncryptedPdf);
    }
    Ok(doc)
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

fn num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)),
        _ => None,
    }
}

/// Page entry incl. inheritance via `/Parent`.
fn inherited<'a>(doc: &'a Document, page_id: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut id = page_id;
    for _ in 0..64 {
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(v) = d.get(key) {
            return Some(deref(doc, v));
        }
        id = d.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn rect(doc: &Document, o: &Object) -> Option<[f64; 4]> {
    let a = deref(doc, o).as_array().ok()?;
    if a.len() != 4 {
        return None;
    }
    let v: Vec<f64> = a.iter().filter_map(|x| num(deref(doc, x))).collect();
    if v.len() != 4 {
        return None;
    }
    Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

/// Visible area (CropBox ∩ MediaBox, otherwise MediaBox, otherwise Letter).
pub fn display_box(doc: &Document, page_id: ObjectId) -> [f64; 4] {
    let media = inherited(doc, page_id, b"MediaBox").and_then(|o| rect(doc, o)).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    match inherited(doc, page_id, b"CropBox").and_then(|o| rect(doc, o)) {
        Some(c) => {
            let r = [c[0].max(media[0]), c[1].max(media[1]), c[2].min(media[2]), c[3].min(media[3])];
            if r[2] > r[0] && r[3] > r[1] { r } else { media }
        }
        None => media,
    }
}

/// `/Rotate` (inherited, normalized to 0/90/180/270).
pub fn page_rotation(doc: &Document, page_id: ObjectId) -> u16 {
    let r = inherited(doc, page_id, b"Rotate").and_then(num).unwrap_or(0.0) as i64;
    (((r % 360) + 360) % 360 / 90 * 90) as u16
}

/// Matrix from display coordinates (origin bottom left of the displayed page, points)
/// into the page's user space.
pub fn display_matrix(b: [f64; 4], rotate: u16) -> [f64; 6] {
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    let (llx, lly) = (b[0], b[1]);
    match rotate {
        90 => [0.0, 1.0, -1.0, 0.0, w + llx, lly],
        180 => [-1.0, 0.0, 0.0, -1.0, w + llx, h + lly],
        270 => [0.0, -1.0, 1.0, 0.0, llx, h + lly],
        _ => [1.0, 0.0, 0.0, 1.0, llx, lly],
    }
}

/// Size of the displayed page in points.
pub fn display_size(b: [f64; 4], rotate: u16) -> (f64, f64) {
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    if rotate == 90 || rotate == 270 { (h, w) } else { (w, h) }
}

fn fmt(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e12 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

// ----------------------------------------------------------------------------------------------
// Page analysis (OCRmyPDF pdfinfo: has_text / images / vector)
// ----------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageAnalysis {
    pub has_text: bool,
    pub has_images: bool,
    pub has_vector: bool,
    /// Largest effective image resolution on the page (dpi).
    pub image_dpi: Option<f64>,
    /// At least one image is not a JPEG (inline image or a filter other than `DCTDecode`).
    /// OCRmyPDF `should_visible_page_image_use_jpg`: only pure JPEG pages are replaced as JPEG.
    pub has_non_jpeg_images: bool,
}

impl PageAnalysis {
    /// All images of the page are JPEG (`DCTDecode`).
    pub fn all_images_jpeg(&self) -> bool {
        self.has_images && !self.has_non_jpeg_images
    }
}

fn mul(m: [f64; 6], n: [f64; 6]) -> [f64; 6] {
    // m * n (PDF: row vector · m · n)
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn ops_of(content: &[u8]) -> Vec<Operation> {
    Content::decode(content).map(|c| c.operations).unwrap_or_default()
}

fn resources_dict<'a>(doc: &'a Document, o: Option<&'a Object>) -> Option<&'a Dictionary> {
    o.map(|o| deref(doc, o)).and_then(|o| o.as_dict().ok())
}

fn analyze_ops(
    doc: &Document,
    ops: &[Operation],
    res: Option<&Dictionary>,
    ctm0: [f64; 6],
    depth: u32,
    out: &mut PageAnalysis,
    seen: &mut HashSet<ObjectId>,
) {
    let mut stack: Vec<[f64; 6]> = Vec::new();
    let mut ctm = ctm0;
    for op in ops {
        match op.operator.as_str() {
            "q" => stack.push(ctm),
            "Q" => ctm = stack.pop().unwrap_or(ctm0),
            "cm" => {
                let v: Vec<f64> = op.operands.iter().filter_map(num).collect();
                if v.len() == 6 {
                    ctm = mul([v[0], v[1], v[2], v[3], v[4], v[5]], ctm);
                }
            }
            "Tj" | "TJ" | "'" | "\"" => out.has_text = true,
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "sh" => out.has_vector = true,
            "BI" | "ID" | "EI" => {
                out.has_images = true;
                out.has_non_jpeg_images = true;
            }
            "Do" => {
                let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else { continue };
                let Some(xo) = res
                    .and_then(|r| r.get(b"XObject").ok())
                    .map(|o| deref(doc, o))
                    .and_then(|o| o.as_dict().ok())
                    .and_then(|d| d.get(name).ok())
                else {
                    continue;
                };
                let id = xo.as_reference().ok();
                let Ok(stream) = deref(doc, xo).as_stream() else { continue };
                match stream.dict.get(b"Subtype").and_then(Object::as_name) {
                    Ok(b"Image") => {
                        out.has_images = true;
                        let jpeg = match stream.dict.get(b"Filter").map(|o| deref(doc, o)) {
                            Ok(Object::Name(n)) => n.as_slice() == b"DCTDecode",
                            Ok(Object::Array(a)) => {
                                a.iter().any(|f| matches!(deref(doc, f), Object::Name(n) if n.as_slice() == b"DCTDecode"))
                            }
                            _ => false,
                        };
                        if !jpeg {
                            out.has_non_jpeg_images = true;
                        }
                        let w = stream.dict.get(b"Width").ok().and_then(num).unwrap_or(0.0);
                        let h = stream.dict.get(b"Height").ok().and_then(num).unwrap_or(0.0);
                        let sx = (ctm[0] * ctm[0] + ctm[1] * ctm[1]).sqrt();
                        let sy = (ctm[2] * ctm[2] + ctm[3] * ctm[3]).sqrt();
                        if sx > 0.0 && sy > 0.0 {
                            let dpi = (w / (sx / 72.0)).max(h / (sy / 72.0));
                            if dpi.is_finite() {
                                out.image_dpi = Some(out.image_dpi.map_or(dpi, |d: f64| d.max(dpi)));
                            }
                        }
                    }
                    Ok(b"Form") if depth < 8 => {
                        if let Some(i) = id
                            && !seen.insert(i)
                        {
                            continue;
                        }
                        let m = stream
                            .dict
                            .get(b"Matrix")
                            .ok()
                            .and_then(|o| o.as_array().ok())
                            .map(|a| a.iter().filter_map(num).collect::<Vec<_>>())
                            .filter(|v| v.len() == 6)
                            .map(|v| [v[0], v[1], v[2], v[3], v[4], v[5]])
                            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
                        let content = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
                        let sub_res = resources_dict(doc, stream.dict.get(b"Resources").ok()).or(res);
                        analyze_ops(doc, &ops_of(&content), sub_res, mul(m, ctm), depth + 1, out, seen);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

pub fn analyze_page(doc: &Document, page_id: ObjectId) -> PageAnalysis {
    let mut out = PageAnalysis::default();
    let content = doc.get_page_content(page_id);
    let res = resources_dict(doc, inherited(doc, page_id, b"Resources"));
    let mut seen = HashSet::new();
    analyze_ops(doc, &ops_of(&content), res, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, &mut out, &mut seen);
    out
}

/// OCRmyPDF `PdfInfo`: signature (`/AcroForm /SigFlags & 1`), form, XFA, tagged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocInfo {
    pub has_signature: bool,
    pub has_acroform: bool,
    pub needs_rendering: bool,
    pub is_tagged: bool,
    pub has_structure_tree: bool,
}

pub fn doc_info(doc: &Document) -> DocInfo {
    let mut i = DocInfo::default();
    let Ok(cat) = doc.catalog() else { return i };
    i.needs_rendering = matches!(cat.get(b"NeedsRendering").map(|o| deref(doc, o)), Ok(Object::Boolean(true)));
    if let Ok(af) = cat.get(b"AcroForm").map(|o| deref(doc, o)).and_then(Object::as_dict) {
        let fields = af.get(b"Fields").map(|o| deref(doc, o)).and_then(Object::as_array).map(|a| a.len()).unwrap_or(0);
        i.has_acroform = fields > 0 || af.has(b"XFA");
        let flags = af.get(b"SigFlags").ok().map(|o| deref(doc, o)).and_then(num).unwrap_or(0.0) as i64;
        i.has_signature = flags & 1 == 1;
    }
    i.is_tagged = cat
        .get(b"MarkInfo")
        .map(|o| deref(doc, o))
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Marked"))
        .map(|o| matches!(deref(doc, o), Object::Boolean(true)))
        .unwrap_or(false);
    i.has_structure_tree = cat.has(b"StructTreeRoot");
    i
}

// ----------------------------------------------------------------------------------------------
// Font + text layer
// ----------------------------------------------------------------------------------------------

/// Invisible font for the text layer (one per document).
pub struct GlyphLessFont {
    pub id: ObjectId,
    cids: BTreeMap<char, u16>,
}

impl GlyphLessFont {
    pub fn new(doc: &mut Document) -> Self {
        Self { id: doc.new_object_id(), cids: BTreeMap::new() }
    }

    fn cid(&mut self, c: char) -> u16 {
        let next = self.cids.len() as u16 + 1;
        if let Some(v) = self.cids.get(&c) {
            return *v;
        }
        if next == u16::MAX {
            return 0;
        }
        self.cids.insert(c, next);
        next
    }

    fn encode(&mut self, s: &str) -> Vec<u8> {
        let mut b = Vec::with_capacity(s.len() * 2);
        for c in s.chars() {
            b.extend_from_slice(&self.cid(c).to_be_bytes());
        }
        b
    }

    fn to_unicode_cmap(&self) -> Vec<u8> {
        let mut s = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
        );
        let entries: Vec<(&char, &u16)> = self.cids.iter().collect();
        for chunk in entries.chunks(100) {
            s.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (c, cid) in chunk {
                let mut u = [0u16; 2];
                let units = c.encode_utf16(&mut u);
                let hex: String = units.iter().map(|x| format!("{x:04X}")).collect();
                s.push_str(&format!("<{cid:04X}> <{hex}>\n"));
            }
            s.push_str("endbfchar\n");
        }
        s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        s.into_bytes()
    }

    /// Write font objects into the document (call after all pages).
    pub fn finish(self, doc: &mut Document) {
        let tounicode = doc.add_object(Stream::new(Dictionary::new(), self.to_unicode_cmap()));
        let mut desc = Dictionary::new();
        desc.set("Type", Object::Name(b"FontDescriptor".to_vec()));
        desc.set("FontName", Object::Name(b"GlyphLessFont".to_vec()));
        desc.set("Flags", 5);
        desc.set("FontBBox", vec![0.into(), 0.into(), i64::from(crate::glyphless::ADVANCE).into(), 1000.into()]);
        desc.set("ItalicAngle", 0);
        desc.set("Ascent", 1000);
        desc.set("Descent", 0);
        desc.set("CapHeight", 1000);
        desc.set("StemV", 80);
        // PDF/A: font program embedded (FontFile2, Length1 = uncompressed length).
        let ttf = crate::glyphless::glyphless_ttf();
        let mut ff = Dictionary::new();
        ff.set("Length1", ttf.len() as i64);
        let ff_id = doc.add_object(Stream::new(ff, ttf));
        desc.set("FontFile2", Object::Reference(ff_id));
        let desc_id = doc.add_object(desc);
        let max_cid = self.cids.values().copied().max().unwrap_or(0);
        let c2g = doc.add_object(Stream::new(Dictionary::new(), crate::glyphless::cid_to_gid_map(max_cid)));
        let mut info = Dictionary::new();
        info.set("Registry", Object::String(b"Adobe".to_vec(), StringFormat::Literal));
        info.set("Ordering", Object::String(b"Identity".to_vec(), StringFormat::Literal));
        info.set("Supplement", 0);
        let mut cid = Dictionary::new();
        cid.set("Type", Object::Name(b"Font".to_vec()));
        cid.set("Subtype", Object::Name(b"CIDFontType2".to_vec()));
        cid.set("BaseFont", Object::Name(b"GlyphLessFont".to_vec()));
        cid.set("CIDSystemInfo", info);
        cid.set("FontDescriptor", Object::Reference(desc_id));
        cid.set("DW", i64::from(crate::glyphless::ADVANCE));
        cid.set("CIDToGIDMap", Object::Reference(c2g));
        let cid_id = doc.add_object(cid);
        let mut f = Dictionary::new();
        f.set("Type", Object::Name(b"Font".to_vec()));
        f.set("Subtype", Object::Name(b"Type0".to_vec()));
        f.set("BaseFont", Object::Name(b"GlyphLessFont".to_vec()));
        f.set("Encoding", Object::Name(b"Identity-H".to_vec()));
        f.set("DescendantFonts", vec![Object::Reference(cid_id)]);
        f.set("ToUnicode", Object::Reference(tounicode));
        doc.objects.insert(self.id, Object::Dictionary(f));
    }
}

/// Content stream of the text layer in display coordinates (points, origin bottom left).
/// `px_w`/`px_h`: raster size, `disp_w`/`disp_h`: displayed page size.
pub fn text_layer_content(ocr: &PageOcr, px_w: f64, px_h: f64, disp_w: f64, disp_h: f64, font: &mut GlyphLessFont) -> Vec<u8> {
    let sx = disp_w / px_w.max(1.0);
    let sy = disp_h / px_h.max(1.0);
    let mut out = Vec::new();
    let name = String::from_utf8_lossy(FONT_RES_NAME).into_owned();
    for line in ocr.lines() {
        if line.is_empty() {
            continue;
        }
        // Line height = largest word box; baseline = lowest word edge.
        let top = line.iter().map(|w| w.y0).fold(f64::INFINITY, f64::min);
        let bottom = line.iter().map(|w| w.y1).fold(f64::NEG_INFINITY, f64::max);
        let fs = ((bottom - top) * sy).max(1.0);
        let base = disp_h - bottom * sy;
        let _ = writeln!(out, "BT\n3 Tr\n/{name} {} Tf", fmt(fs));
        let n = line.len();
        for (i, w) in line.iter().enumerate() {
            let mut text = w.text.clone();
            let chars = text.chars().count().max(1) as f64;
            let box_w = ((w.x1 - w.x0) * sx).max(0.1);
            let natural = chars * 0.5 * fs;
            let tz = (100.0 * box_w / natural).clamp(1.0, 10_000.0);
            if i + 1 < n {
                text.push(' ');
            }
            let hex: String = font.encode(&text).iter().map(|b| format!("{b:02X}")).collect();
            let _ = writeln!(out, "{} Tz\n1 0 0 1 {} {} Tm\n<{hex}> Tj", fmt(tz), fmt(w.x0 * sx), fmt(base));
        }
        out.extend_from_slice(b"ET\n");
    }
    out
}

fn page_dict_mut(doc: &mut Document, page_id: ObjectId) -> Result<&mut Dictionary> {
    doc.get_dictionary_mut(page_id).map_err(|e| OcrError::InputFile(format!("page object: {e}")))
}

/// Resources of the page as its own direct dictionary (inherited/shared ones are copied).
fn materialize_resources(doc: &mut Document, page_id: ObjectId) -> Result<()> {
    let res = inherited(doc, page_id, b"Resources").and_then(|o| o.as_dict().ok()).cloned().unwrap_or_default();
    page_dict_mut(doc, page_id)?.set("Resources", res);
    Ok(())
}

fn resources_mut(doc: &mut Document, page_id: ObjectId) -> Result<&mut Dictionary> {
    page_dict_mut(doc, page_id)?
        .get_mut(b"Resources")
        .and_then(Object::as_dict_mut)
        .map_err(|e| OcrError::InputFile(format!("resources: {e}")))
}

/// Ensure `key` (Font/XObject) in the resources dictionary as a direct dictionary and
/// enter `name → value`.
fn add_resource(doc: &mut Document, page_id: ObjectId, key: &[u8], name: &[u8], value: Object) -> Result<()> {
    materialize_resources(doc, page_id)?;
    let sub = {
        let res = resources_mut(doc, page_id)?;
        res.get(key).ok().cloned()
    };
    let mut sub_dict = match sub {
        Some(Object::Reference(id)) => doc.get_dictionary(id).cloned().unwrap_or_default(),
        Some(Object::Dictionary(d)) => d,
        _ => Dictionary::new(),
    };
    sub_dict.set(name.to_vec(), value);
    resources_mut(doc, page_id)?.set(key.to_vec(), sub_dict);
    Ok(())
}

/// Wrap existing content in `q … Q` and append `extra`.
fn append_content(doc: &mut Document, page_id: ObjectId, extra: Vec<u8>) -> Result<()> {
    let old = page_dict_mut(doc, page_id)?.get(b"Contents").ok().cloned();
    let mut arr: Vec<Object> = Vec::new();
    let q = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    arr.push(Object::Reference(q));
    match old {
        Some(Object::Array(a)) => arr.extend(a),
        Some(Object::Reference(id)) => match doc.get_object(id) {
            Ok(Object::Array(a)) => arr.extend(a.clone()),
            _ => arr.push(Object::Reference(id)),
        },
        Some(other) => arr.push(other),
        None => {}
    }
    let mut tail = b"\nQ\n".to_vec();
    tail.extend_from_slice(&extra);
    let t = doc.add_object(Stream::new(Dictionary::new(), tail));
    arr.push(Object::Reference(t));
    page_dict_mut(doc, page_id)?.set("Contents", arr);
    Ok(())
}

fn set_contents(doc: &mut Document, page_id: ObjectId, content: Vec<u8>) -> Result<()> {
    let s = doc.add_object(Stream::new(Dictionary::new(), content));
    page_dict_mut(doc, page_id)?.set("Contents", Object::Reference(s));
    Ok(())
}

/// Put the text layer on an existing page. `ocr` refers to the displayed raster.
pub fn overlay_text(doc: &mut Document, page_id: ObjectId, ocr: &PageOcr, font: &mut GlyphLessFont) -> Result<()> {
    if ocr.words.is_empty() {
        return Ok(());
    }
    let b = display_box(doc, page_id);
    let rot = page_rotation(doc, page_id);
    let (dw, dh) = display_size(b, rot);
    let m = display_matrix(b, rot);
    let body = text_layer_content(ocr, f64::from(ocr.width), f64::from(ocr.height), dw, dh, font);
    let mut c = format!("q {} {} {} {} {} {} cm\n", fmt(m[0]), fmt(m[1]), fmt(m[2]), fmt(m[3]), fmt(m[4]), fmt(m[5])).into_bytes();
    c.extend_from_slice(&body);
    c.extend_from_slice(b"Q\n");
    add_resource(doc, page_id, b"Font", FONT_RES_NAME, Object::Reference(font.id))?;
    append_content(doc, page_id, c)
}

/// Remove text operators: `only_invisible` → only render mode 3 (old OCR layer, `--redo-ocr`),
/// otherwise any text (for rasterizing without visible text, like OCRmyPDF redo).
pub fn strip_text(content: &[u8], only_invisible: bool) -> Vec<u8> {
    let Ok(c) = Content::decode(content) else { return content.to_vec() };
    let mut tr = 0i64;
    let mut stack = Vec::new();
    let ops: Vec<Operation> = c
        .operations
        .into_iter()
        .filter(|op| match op.operator.as_str() {
            "q" => {
                stack.push(tr);
                true
            }
            "Q" => {
                tr = stack.pop().unwrap_or(0);
                true
            }
            "Tr" => {
                tr = op.operands.first().and_then(|o| o.as_i64().ok()).unwrap_or(0);
                true
            }
            "Tj" | "TJ" | "'" | "\"" => only_invisible && tr != 3,
            _ => true,
        })
        .collect();
    Content { operations: ops }.encode().unwrap_or_else(|_| content.to_vec())
}

/// Remove vector graphics (OCRmyPDF `--remove-vectors`, for the recognition image only): path painting commands
/// become `n` (discard path), gradients (`sh`) are dropped. Page content only, not form XObjects.
pub fn strip_vectors(content: &[u8]) -> Vec<u8> {
    let Ok(c) = Content::decode(content) else { return content.to_vec() };
    let ops: Vec<Operation> = c
        .operations
        .into_iter()
        .filter(|op| op.operator != "sh")
        .map(|op| match op.operator.as_str() {
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => Operation::new("n", vec![]),
            _ => op,
        })
        .collect();
    Content { operations: ops }.encode().unwrap_or_else(|_| content.to_vec())
}

pub fn strip_page_vectors(doc: &mut Document, page_id: ObjectId) -> Result<()> {
    let content = doc.get_page_content(page_id);
    set_contents(doc, page_id, strip_vectors(&content))
}

/// Replace page content with `strip_text`.
pub fn strip_page_text(doc: &mut Document, page_id: ObjectId, only_invisible: bool) -> Result<()> {
    let content = doc.get_page_content(page_id);
    set_contents(doc, page_id, strip_text(&content, only_invisible))
}

// ----------------------------------------------------------------------------------------------
// Images
// ----------------------------------------------------------------------------------------------

/// Image data for a PDF page.
#[derive(Debug, Clone)]
pub enum PageImage {
    /// JPEG unchanged (DCTDecode), `components` 1 or 3.
    Jpeg { data: Vec<u8>, width: u32, height: u32, components: u8 },
    /// 8-bit raster (gray/RGB), Flate-compressed.
    Raw(Raster),
}

impl PageImage {
    fn size(&self) -> (u32, u32) {
        match self {
            PageImage::Jpeg { width, height, .. } => (*width, *height),
            PageImage::Raw(r) => (r.width, r.height),
        }
    }

    fn to_xobject(&self) -> Stream {
        let (w, h) = self.size();
        let mut d = Dictionary::new();
        d.set("Type", Object::Name(b"XObject".to_vec()));
        d.set("Subtype", Object::Name(b"Image".to_vec()));
        d.set("Width", i64::from(w));
        d.set("Height", i64::from(h));
        d.set("BitsPerComponent", 8);
        match self {
            PageImage::Jpeg { data, components, .. } => {
                let cs: &[u8] = if *components == 1 { b"DeviceGray" } else { b"DeviceRGB" };
                d.set("ColorSpace", Object::Name(cs.to_vec()));
                d.set("Filter", Object::Name(b"DCTDecode".to_vec()));
                Stream::new(d, data.clone()).with_compression(false)
            }
            PageImage::Raw(r) => {
                let cs: &[u8] = if r.channels == 1 { b"DeviceGray" } else { b"DeviceRGB" };
                d.set("ColorSpace", Object::Name(cs.to_vec()));
                let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                let _ = enc.write_all(&r.data);
                let z = enc.finish().unwrap_or_default();
                d.set("Filter", Object::Name(b"FlateDecode".to_vec()));
                Stream::new(d, z).with_compression(false)
            }
        }
    }
}

/// An image page (img2pdf: page = pixels / dpi · 72 pt).
#[derive(Debug, Clone)]
pub struct ImagePageSpec {
    pub image: PageImage,
    pub dpi_x: f64,
    pub dpi_y: f64,
    /// `/Rotate` (EXIF orientation like img2pdf).
    pub rotate: u16,
    /// OCR of the *displayed* (rotated) image.
    pub ocr: Option<PageOcr>,
}

fn new_doc(version: &str) -> (Document, ObjectId) {
    let mut doc = Document::with_version(version);
    let pages_id = doc.new_object_id();
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Vec::<Object>::new());
    pages.set("Count", 0);
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let mut cat = Dictionary::new();
    cat.set("Type", Object::Name(b"Catalog".to_vec()));
    cat.set("Pages", Object::Reference(pages_id));
    let cat_id = doc.add_object(cat);
    doc.trailer.set("Root", Object::Reference(cat_id));
    (doc, pages_id)
}

fn push_page(doc: &mut Document, pages_id: ObjectId, page: Dictionary) -> Result<ObjectId> {
    let id = doc.add_object(page);
    let pages = doc.get_dictionary_mut(pages_id).map_err(|e| OcrError::Other(e.to_string()))?;
    if let Ok(Object::Array(k)) = pages.get_mut(b"Kids") {
        k.push(Object::Reference(id));
        let n = k.len() as i64;
        pages.set("Count", n);
    }
    Ok(id)
}

/// Page whose content consists only of `image` (over the displayed area).
fn image_page_content(image_name: &str, b: [f64; 4], rotate: u16, img_w_pt: f64, img_h_pt: f64) -> Vec<u8> {
    let _ = (b, rotate);
    format!("q {} 0 0 {} 0 0 cm /{image_name} Do Q\n", fmt(img_w_pt), fmt(img_h_pt)).into_bytes()
}

pub fn pdf_version_for(output: OutputType) -> &'static str {
    match output {
        OutputType::PdfA1 => "1.4",
        _ => "1.7",
    }
}

/// Images → PDF (one page per image), optionally with a text layer. `None` OCR = without text (img2pdf).
pub fn build_from_images(pages: &[ImagePageSpec], meta: &OutputMeta) -> Result<Vec<u8>> {
    let (mut doc, pages_id) = new_doc(pdf_version_for(meta.output_type));
    let mut font = GlyphLessFont::new(&mut doc);
    let mut used_font = false;
    for p in pages {
        let (w, h) = p.image.size();
        let wpt = f64::from(w) * 72.0 / p.dpi_x.max(1.0);
        let hpt = f64::from(h) * 72.0 / p.dpi_y.max(1.0);
        let img_id = doc.add_object(p.image.to_xobject());
        let mut xo = Dictionary::new();
        xo.set("Im0", Object::Reference(img_id));
        let mut res = Dictionary::new();
        res.set("XObject", xo);
        let content = image_page_content("Im0", [0.0, 0.0, wpt, hpt], p.rotate, wpt, hpt);
        let c_id = doc.add_object(Stream::new(Dictionary::new(), content));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("MediaBox", vec![0.into(), 0.into(), Object::Real(wpt as f32), Object::Real(hpt as f32)]);
        page.set("Resources", res);
        page.set("Contents", Object::Reference(c_id));
        if p.rotate != 0 {
            page.set("Rotate", i64::from(p.rotate));
        }
        let page_id = push_page(&mut doc, pages_id, page)?;
        if let Some(ocr) = &p.ocr
            && !ocr.words.is_empty()
        {
            overlay_text(&mut doc, page_id, ocr, &mut font)?;
            used_font = true;
        }
    }
    if used_font {
        font.finish(&mut doc);
    }
    finish(doc, meta)
}

// ----------------------------------------------------------------------------------------------
// Edit pages of an existing PDF
// ----------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum PageAction {
    /// Unchanged.
    Keep,
    /// Add text layer; `strip_old_ocr` first removes invisible text (redo).
    Overlay { ocr: PageOcr, strip_old_ocr: bool },
    /// Replace the page with an image (force; deskew/clean-final) + text layer. The image is in
    /// display orientation (incl. `rotate_add`).
    Replace { image: PageImage, ocr: PageOcr },
}

#[derive(Debug, Clone)]
pub struct PageEdit {
    pub action: PageAction,
    /// Additional clockwise rotation (rotate_pages), added to `/Rotate`.
    pub rotate_add: u16,
}

/// Metadata and output options.
#[derive(Debug, Clone)]
pub struct OutputMeta {
    pub output_type: OutputType,
    pub producer: String,
    pub creator_tool: String,
    /// `color_conversion_strategy` (effective only with PDF/A, like upstream).
    pub color: ColorConversion,
    /// OCRmyPDF `--optimize`/`--jpeg-quality`/`--png-quality`.
    pub optimize: crate::optimize::OptimizeOptions,
    /// OCRmyPDF `--title`/`--author`/`--subject`/`--keywords`.
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
}

impl Default for OutputMeta {
    fn default() -> Self {
        Self {
            output_type: OutputType::PdfA,
            producer: format!("PaperRust pl-ocr {}", env!("CARGO_PKG_VERSION")),
            creator_tool: "PaperRust OCR".into(),
            color: ColorConversion::Rgb,
            optimize: crate::optimize::OptimizeOptions::default(),
            title: None,
            author: None,
            subject: None,
            keywords: None,
        }
    }
}

/// Edit pages (order = pages of the document; missing entries = `Keep`).
pub fn build_from_pdf(original: &[u8], edits: &[PageEdit], meta: &OutputMeta) -> Result<Vec<u8>> {
    let mut doc = load_pdf(original)?;
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    let mut font = GlyphLessFont::new(&mut doc);
    let mut used_font = false;
    for (i, page_id) in pages.iter().copied().enumerate() {
        let Some(edit) = edits.get(i) else { continue };
        if edit.rotate_add % 360 != 0 {
            let r = (page_rotation(&doc, page_id) + edit.rotate_add) % 360;
            page_dict_mut(&mut doc, page_id)?.set("Rotate", i64::from(r));
        }
        match &edit.action {
            PageAction::Keep => {}
            PageAction::Overlay { ocr, strip_old_ocr } => {
                if *strip_old_ocr {
                    strip_page_text(&mut doc, page_id, true)?;
                }
                if !ocr.words.is_empty() {
                    overlay_text(&mut doc, page_id, ocr, &mut font)?;
                    used_font = true;
                }
            }
            PageAction::Replace { image, ocr } => {
                let b = display_box(&doc, page_id);
                let rot = page_rotation(&doc, page_id);
                let (dw, dh) = display_size(b, rot);
                let m = display_matrix(b, rot);
                let img_id = doc.add_object(image.to_xobject());
                let mut xo = Dictionary::new();
                xo.set("PlOcrPage", Object::Reference(img_id));
                let mut res = Dictionary::new();
                res.set("XObject", xo);
                let content = format!(
                    "q {} {} {} {} {} {} cm {} 0 0 {} 0 0 cm /PlOcrPage Do Q\n",
                    fmt(m[0]),
                    fmt(m[1]),
                    fmt(m[2]),
                    fmt(m[3]),
                    fmt(m[4]),
                    fmt(m[5]),
                    fmt(dw),
                    fmt(dh)
                );
                {
                    let pd = page_dict_mut(&mut doc, page_id)?;
                    pd.set("Resources", res);
                    // Form fields/annotations are rasterized along; links stay.
                    pd.remove(b"Annots");
                }
                set_contents(&mut doc, page_id, content.into_bytes())?;
                if !ocr.words.is_empty() {
                    overlay_text(&mut doc, page_id, ocr, &mut font)?;
                    used_font = true;
                }
            }
        }
    }
    if used_font {
        font.finish(&mut doc);
    }
    if edits.iter().any(|e| matches!(e.action, PageAction::Replace { .. })) {
        // Rasterized pages: form is flat (OCRmyPDF force).
        if let Ok(cat) = doc.catalog_mut() {
            cat.remove(b"AcroForm");
        }
    }
    finish(doc, meta)
}

/// `_convert_pdf_to_pdfa` without Ghostscript: PDF/A cleanup, color conversion, metadata
/// (see [`finish_with_report`]).
pub fn stamp_pdf(original: &[u8], meta: &OutputMeta) -> Result<Vec<u8>> {
    let doc = load_pdf(original)?;
    finish(doc, meta)
}

// ----------------------------------------------------------------------------------------------
// PDF/A
// ----------------------------------------------------------------------------------------------

fn pdf_string_to_text(o: &Object) -> Option<String> {
    let Object::String(b, _) = o else { return None };
    if b.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = b[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return Some(String::from_utf16_lossy(&u));
    }
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Some(String::from_utf8_lossy(&b[3..]).into_owned());
    }
    // PDFDocEncoding ≈ Latin-1 for printable characters
    Some(b.iter().map(|&c| c as char).collect())
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(*c, '\0'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f'))
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// `D:YYYYMMDDHHmmSS+HH'mm'` → ISO 8601 for XMP.
pub fn pdf_date_to_xmp(s: &str) -> Option<String> {
    let s = s.strip_prefix("D:").unwrap_or(s);
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |a: usize, b: usize, d: &str| digits.get(a..b).unwrap_or(d).to_string();
    let (y, mo, da, h, mi, se) =
        (part(0, 4, "0000"), part(4, 6, "01"), part(6, 8, "01"), part(8, 10, "00"), part(10, 12, "00"), part(12, 14, "00"));
    let rest = &s[digits.len()..];
    let tz = match rest.chars().next() {
        Some('Z') => "Z".to_string(),
        Some(c @ ('+' | '-')) => {
            let t: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
            format!("{c}{}:{}", t.get(0..2).unwrap_or("00"), t.get(2..4).unwrap_or("00"))
        }
        _ => String::new(),
    };
    Some(format!("{y}-{mo}-{da}T{h}:{mi}:{se}{tz}"))
}

fn now_pdf_date() -> (String, String) {
    let now = chrono::Utc::now();
    (now.format("D:%Y%m%d%H%M%SZ").to_string(), now.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// Report of [`finish_with_report`] (for tests and log).
#[derive(Debug, Default, Clone)]
pub struct FinishReport {
    pub pdfa_fixes: Vec<String>,
    /// Violations after cleanup (empty = PDF/A self-check passed).
    pub pdfa_violations: Vec<crate::pdfa::Violation>,
    /// PDF/A claim (XMP `pdfaid`) was set.
    pub pdfa_claimed: bool,
    pub color: crate::color::ColorReport,
    pub optimize: crate::optimize::OptimizeReport,
}

fn set_text(info: &mut Dictionary, key: &str, v: &str) {
    // non-ASCII as UTF-16BE (PDF text string)
    let bytes = if v.is_ascii() {
        v.as_bytes().to_vec()
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in v.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        b
    };
    info.set(key, Object::String(bytes, StringFormat::Literal));
}

fn build_xmp(get: &dyn Fn(&[u8]) -> Option<String>, meta: &OutputMeta, part: Option<u8>, xmp_now: &str, trapped: Option<&str>) -> String {
    let mut x = String::new();
    x.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    x.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    x.push_str("<rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">\n");
    if let Some(part) = part {
        x.push_str(&format!("<pdfaid:part>{part}</pdfaid:part>\n<pdfaid:conformance>B</pdfaid:conformance>\n"));
    }
    x.push_str("<dc:format>application/pdf</dc:format>\n");
    if let Some(t) = get(b"Title") {
        x.push_str(&format!("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>\n", xml_escape(&t)));
    }
    if let Some(t) = get(b"Author") {
        x.push_str(&format!("<dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n", xml_escape(&t)));
    }
    if let Some(t) = get(b"Subject") {
        x.push_str(&format!(
            "<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>\n",
            xml_escape(&t)
        ));
    }
    if let Some(t) = get(b"Keywords") {
        x.push_str(&format!("<pdf:Keywords>{}</pdf:Keywords>\n", xml_escape(&t)));
    }
    x.push_str(&format!("<pdf:Producer>{}</pdf:Producer>\n", xml_escape(&meta.producer)));
    if let Some(t) = trapped {
        x.push_str(&format!("<pdf:Trapped>{t}</pdf:Trapped>\n"));
    }
    if let Some(t) = get(b"Creator") {
        x.push_str(&format!("<xmp:CreatorTool>{}</xmp:CreatorTool>\n", xml_escape(&t)));
    }
    let created = get(b"CreationDate").and_then(|d| pdf_date_to_xmp(&d)).unwrap_or_else(|| xmp_now.to_string());
    let modified = get(b"ModDate").and_then(|d| pdf_date_to_xmp(&d)).unwrap_or_else(|| xmp_now.to_string());
    x.push_str(&format!("<xmp:CreateDate>{created}</xmp:CreateDate>\n<xmp:ModifyDate>{modified}</xmp:ModifyDate>\n<xmp:MetadataDate>{modified}</xmp:MetadataDate>\n"));
    x.push_str("</rdf:Description>\n</rdf:RDF>\n</x:xmpmeta>\n");
    x.push_str(&" ".repeat(2000));
    x.push_str("\n<?xpacket end=\"w\"?>");
    x
}

fn save(doc: &mut Document, modern: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    if modern {
        doc.save_modern(&mut out).map_err(|e| OcrError::Other(format!("PDF write failed: {e}")))?;
    } else {
        // Classic xref table (lopdf default would be an xref stream → PDF 1.5, not permitted for PDF/A-1).
        doc.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceTable;
        doc.save_to(&mut out).map_err(|e| OcrError::Other(format!("PDF write failed: {e}")))?;
    }
    Ok(out)
}

fn finish(doc: Document, meta: &OutputMeta) -> Result<Vec<u8>> {
    finish_with_report(doc, meta).map(|(b, _)| b)
}

/// Finish the output: metadata, PDF/A cleanup, color conversion, optimization, XMP/
/// OutputIntent, self-check. If the result does not pass the PDF/A check, the PDF/A claim
/// is omitted (like Ghostscript when it cannot write conformantly) and a warning is issued.
pub fn finish_with_report(mut doc: Document, meta: &OutputMeta) -> Result<(Vec<u8>, FinishReport)> {
    let mut report = FinishReport::default();
    let (pdf_now, xmp_now) = now_pdf_date();
    let part = meta.output_type.pdfa_part();
    // Info dictionary (update Producer/ModDate, keep the rest).
    let info_id = match doc.trailer.get(b"Info").ok().and_then(|o| o.as_reference().ok()) {
        Some(id) if doc.get_dictionary(id).is_ok() => id,
        _ => {
            let direct = doc.trailer.get(b"Info").ok().and_then(|o| o.as_dict().ok()).cloned().unwrap_or_default();
            let id = doc.add_object(direct);
            doc.trailer.set("Info", Object::Reference(id));
            id
        }
    };
    let mut trapped = None;
    {
        let info = doc.get_dictionary_mut(info_id).map_err(|e| OcrError::Other(e.to_string()))?;
        info.set("Producer", Object::String(meta.producer.clone().into_bytes(), StringFormat::Literal));
        info.set("ModDate", Object::String(pdf_now.clone().into_bytes(), StringFormat::Literal));
        // Replace an unreadable creation date (Info and XMP must match).
        let created_ok = info.get(b"CreationDate").ok().and_then(pdf_string_to_text).and_then(|d| pdf_date_to_xmp(&d)).is_some();
        if !created_ok {
            info.set("CreationDate", Object::String(pdf_now.clone().into_bytes(), StringFormat::Literal));
        }
        if !info.has(b"Creator") {
            info.set("Creator", Object::String(meta.creator_tool.clone().into_bytes(), StringFormat::Literal));
        }
        // OCRmyPDF --title/--author/--subject/--keywords
        for (k, v) in [("Title", &meta.title), ("Author", &meta.author), ("Subject", &meta.subject), ("Keywords", &meta.keywords)] {
            if let Some(v) = v {
                set_text(info, k, v);
            }
        }
        match info.get(b"Trapped").ok().and_then(|o| o.as_name().ok()) {
            Some(b"True") => trapped = Some("True"),
            Some(b"False") => trapped = Some("False"),
            _ => {
                info.remove(b"Trapped");
            }
        }
        // Only text strings (and Trapped) make sense in Info: remove other types
        let bad: Vec<Vec<u8>> =
            info.iter().filter(|(_, v)| !matches!(v, Object::String(..) | Object::Name(_))).map(|(k, _)| k.clone()).collect();
        for k in bad {
            info.remove(&k);
        }
    }

    if let Some(part) = part {
        let r = crate::pdfa::normalize(&mut doc, part);
        for f in &r.fixes {
            tracing::info!("PDF/A-{part}: {f}");
        }
        report.pdfa_fixes = r.fixes;
        // upstream passes color_conversion_strategy on only with PDF/A
        report.color = crate::color::apply(&mut doc, crate::color::target_for(meta.color, true));
    }
    report.optimize = crate::optimize::optimize_images(&mut doc, &meta.optimize);
    if meta.optimize.level > 0 && report.optimize.bytes_before > 0 {
        tracing::debug!(
            "optimize level {}: images {} -> {} bytes ({} JBIG2, {} Flate, {} JPEG, {} JPEG+Flate, {} quantized)",
            meta.optimize.level,
            report.optimize.bytes_before,
            report.optimize.bytes_after,
            report.optimize.jbig2,
            report.optimize.flate,
            report.optimize.jpeg_recoded,
            report.optimize.jpeg_deflated,
            report.optimize.quantized
        );
    }

    let info = doc.get_dictionary(info_id).cloned().unwrap_or_default();
    let get = |k: &[u8]| info.get(k).ok().and_then(pdf_string_to_text);
    let md_id = if let Some(part) = part {
        doc.version = pdf_version_for(meta.output_type).to_string();
        let x = build_xmp(&get, meta, Some(part), &xmp_now, trapped);
        let mut md = Dictionary::new();
        md.set("Type", Object::Name(b"Metadata".to_vec()));
        md.set("Subtype", Object::Name(b"XML".to_vec()));
        let md_id = doc.add_object(Stream::new(md, x.into_bytes()).with_compression(false));

        let mut icc_d = Dictionary::new();
        icc_d.set("N", 3);
        let icc_id = doc.add_object(Stream::new(icc_d, crate::icc::srgb_profile()));
        let mut oi = Dictionary::new();
        oi.set("Type", Object::Name(b"OutputIntent".to_vec()));
        oi.set("S", Object::Name(b"GTS_PDFA1".to_vec()));
        oi.set("OutputConditionIdentifier", Object::String(b"sRGB".to_vec(), StringFormat::Literal));
        oi.set("Info", Object::String(b"sRGB".to_vec(), StringFormat::Literal));
        oi.set("DestOutputProfile", Object::Reference(icc_id));
        let oi_arr = doc.add_object(vec![Object::Dictionary(oi)]);
        let cat = doc.catalog_mut().map_err(|e| OcrError::InputFile(format!("catalog: {e}")))?;
        cat.set("Metadata", Object::Reference(md_id));
        cat.set("OutputIntents", Object::Reference(oi_arr));
        Some(md_id)
    } else {
        None
    };

    // trailer ID (PDF/A: required)
    let mut h = sha2::Sha256::default();
    sha2::Digest::update(&mut h, pdf_now.as_bytes());
    sha2::Digest::update(&mut h, meta.producer.as_bytes());
    sha2::Digest::update(&mut h, (doc.objects.len() as u64).to_le_bytes());
    let digest = sha2::Digest::finalize(h);
    let id = digest[..16].to_vec();
    let first = match doc.trailer.get(b"ID").ok().and_then(|o| o.as_array().ok()).and_then(|a| a.first().cloned()) {
        Some(Object::String(b, f)) => Object::String(b, f),
        _ => Object::String(id.clone(), StringFormat::Hexadecimal),
    };
    doc.trailer.set("ID", vec![first, Object::String(id, StringFormat::Hexadecimal)]);
    doc.trailer.remove(b"Encrypt");
    // Leftovers of a read xref stream (Type/W/Index/Filter …) do not belong in the trailer.
    let keep: [&[u8]; 4] = [b"Root", b"Info", b"ID", b"Size"];
    let extra: Vec<Vec<u8>> = doc.trailer.iter().map(|(k, _)| k.clone()).filter(|k| !keep.contains(&k.as_slice())).collect();
    for k in extra {
        doc.trailer.remove(&k);
    }

    if meta.optimize.level > 0 {
        // OCRmyPDF `remove_unreferenced_resources` + pikepdf saving without unused objects
        doc.prune_objects();
    }
    doc.compress();
    // Object streams like OCRmyPDF (`object_stream_mode=generate`), not with PDF/A-1.
    let modern = meta.optimize.level > 0 && part != Some(1);
    let mut out = save(&mut doc, modern)?;

    if let (Some(part), Some(_)) = (part, md_id) {
        let v = crate::pdfa::check(&out, part);
        report.pdfa_claimed = v.is_empty();
        if !v.is_empty() {
            tracing::warn!(
                "Output is not PDF/A-{part} conformant after conversion ({}); the PDF/A claim is omitted",
                crate::pdfa::summarize(&v)
            );
            // XMP is uncompressed in the file: replace the claim with the same number of spaces
            // (offsets stay valid).
            let claim = format!(
                "<pdfaid:part>{part}</pdfaid:part>
<pdfaid:conformance>B</pdfaid:conformance>
"
            );
            if let Some(pos) = out.windows(claim.len()).position(|w| w == claim.as_bytes()) {
                out[pos..pos + claim.len()].fill(b' ');
            }
        }
        report.pdfa_violations = v;
    }
    Ok((out, report))
}

// ----------------------------------------------------------------------------------------------
// Reading
// ----------------------------------------------------------------------------------------------

/// Page count (`get_page_count_for_pdf`): `None` for unreadable/encrypted PDF.
pub fn page_count(data: &[u8]) -> Option<usize> {
    load_pdf(data).ok().map(|d| d.get_pages().len())
}

/// Text of all pages with lopdf (pages separated by `\x0c`) – replacement for `pdftotext`.
pub fn extract_text(data: &[u8]) -> Option<String> {
    let doc = load_pdf(data).ok()?;
    let mut out = String::new();
    for p in doc.get_pages().keys() {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| doc.extract_text(&[*p])));
        if let Ok(Ok(t)) = r {
            out.push_str(&t);
        }
        out.push('\x0c');
    }
    let out = out.replace('\0', "");
    if out.is_empty() { None } else { Some(out) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Word;

    fn word(t: &str, x0: f64, y0: f64, x1: f64, y1: f64, line: u32) -> Word {
        Word { text: t.into(), x0, y0, x1, y1, conf: 90.0, line: (1, 1, line) }
    }

    fn sample_ocr() -> PageOcr {
        PageOcr {
            width: 850,
            height: 1100,
            words: vec![
                word("Hello", 100.0, 100.0, 200.0, 130.0, 1),
                word("Wörld", 220.0, 100.0, 330.0, 130.0, 1),
                word("Rechnung", 100.0, 200.0, 300.0, 230.0, 2),
                word("€42", 320.0, 200.0, 380.0, 230.0, 2),
                word("𝔘nicode", 100.0, 300.0, 300.0, 330.0, 3),
            ],
            text: String::new(),
        }
    }

    fn gray(w: u32, h: u32) -> Raster {
        Raster { width: w, height: h, channels: 1, data: vec![255; (w * h) as usize], dpi: 100.0 }
    }

    #[test]
    fn image_pdf_with_text_layer_is_extractable() {
        let spec = ImagePageSpec { image: PageImage::Raw(gray(850, 1100)), dpi_x: 100.0, dpi_y: 100.0, rotate: 0, ocr: Some(sample_ocr()) };
        let pdf = build_from_images(&[spec], &OutputMeta::default()).unwrap();
        let text = extract_text(&pdf).unwrap();
        let norm = crate::text::post_process_text(Some(&text)).unwrap();
        assert_eq!(norm, "Hello Wörld\nRechnung €42\n𝔘nicode");
        let doc = load_pdf(&pdf).unwrap();
        assert_eq!(doc.version, "1.7");
        let cat = doc.catalog().unwrap();
        assert!(cat.has(b"OutputIntents") && cat.has(b"Metadata"));
        assert_eq!(page_count(&pdf), Some(1));
        // page = 850/100*72 = 612 pt
        let pid = *doc.get_pages().values().next().unwrap();
        assert_eq!(display_box(&doc, pid), [0.0, 0.0, 612.0, 792.0]);
        // render mode 3
        let content = String::from_utf8_lossy(&doc.get_page_content(pid)).into_owned();
        assert!(content.contains("3 Tr"));
    }

    #[test]
    fn overlay_on_existing_pdf_keeps_content_and_rotation() {
        // source PDF: image page without text, rotated by 90°
        let spec = ImagePageSpec { image: PageImage::Raw(gray(100, 200)), dpi_x: 72.0, dpi_y: 72.0, rotate: 90, ocr: None };
        let meta = OutputMeta { output_type: OutputType::Pdf, ..Default::default() };
        let base = build_from_images(&[spec], &meta).unwrap();
        assert_eq!(extract_text(&base).map(|t| t.trim().to_string()), Some(String::new()));
        let doc = load_pdf(&base).unwrap();
        let pid = *doc.get_pages().values().next().unwrap();
        assert_eq!(page_rotation(&doc, pid), 90);
        assert!(analyze_page(&doc, pid).has_images);
        assert!(!analyze_page(&doc, pid).has_text);
        assert_eq!(analyze_page(&doc, pid).image_dpi.map(|d| d.round()), Some(72.0));
        // displayed 200x100 px
        let ocr = PageOcr { width: 200, height: 100, words: vec![word("Gedreht", 10.0, 10.0, 90.0, 30.0, 1)], text: String::new() };
        let edit = PageEdit { action: PageAction::Overlay { ocr, strip_old_ocr: false }, rotate_add: 0 };
        let out = build_from_pdf(&base, &[edit], &OutputMeta::default()).unwrap();
        assert_eq!(crate::text::post_process_text(extract_text(&out).as_deref()).as_deref(), Some("Gedreht"));
        let doc = load_pdf(&out).unwrap();
        let pid = *doc.get_pages().values().next().unwrap();
        assert!(analyze_page(&doc, pid).has_text && analyze_page(&doc, pid).has_images);
    }

    #[test]
    fn strip_invisible_only() {
        let c = b"BT /F1 12 Tf (visible) Tj ET q BT 3 Tr (hidden) Tj ET Q BT (again) Tj ET".to_vec();
        let s = String::from_utf8_lossy(&strip_text(&c, true)).into_owned();
        assert!(s.contains("visible") && s.contains("again") && !s.contains("hidden"));
        let s = String::from_utf8_lossy(&strip_text(&c, false)).into_owned();
        assert!(!s.contains("visible") && !s.contains("hidden"));
    }

    #[test]
    fn display_matrices() {
        let b = [10.0, 20.0, 110.0, 220.0]; // 100 x 200
        let apply = |m: [f64; 6], x: f64, y: f64| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        // origin of the display (bottom left) per rotation
        assert_eq!(apply(display_matrix(b, 0), 0.0, 0.0), (10.0, 20.0));
        // 90°: displayed 200 x 100; display bottom left = user space bottom right
        assert_eq!(apply(display_matrix(b, 90), 0.0, 0.0), (110.0, 20.0));
        assert_eq!(apply(display_matrix(b, 90), 0.0, 100.0), (10.0, 20.0));
        assert_eq!(apply(display_matrix(b, 180), 0.0, 0.0), (110.0, 220.0));
        assert_eq!(apply(display_matrix(b, 270), 0.0, 0.0), (10.0, 220.0));
        assert_eq!(display_size(b, 90), (200.0, 100.0));
    }

    #[test]
    fn xmp_dates() {
        assert_eq!(pdf_date_to_xmp("D:20240102030405+01'00'").as_deref(), Some("2024-01-02T03:04:05+01:00"));
        assert_eq!(pdf_date_to_xmp("D:2024").as_deref(), Some("2024-01-01T00:00:00"));
    }

    #[test]
    fn encrypted_detected() {
        let spec = ImagePageSpec { image: PageImage::Raw(gray(10, 10)), dpi_x: 72.0, dpi_y: 72.0, rotate: 0, ocr: None };
        let pdf = build_from_images(&[spec], &OutputMeta { output_type: OutputType::Pdf, ..Default::default() }).unwrap();
        let mut doc = load_pdf(&pdf).unwrap();
        doc.trailer.set("Encrypt", Dictionary::new());
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        assert!(matches!(load_pdf(&buf), Err(OcrError::EncryptedPdf)));
    }
}
