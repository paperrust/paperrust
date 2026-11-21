//! PDF/A without Ghostscript: cleanup ([`normalize`]) and self-check ([`check`]).
//!
//! upstream generates PDF/A with Ghostscript (`-dPDFA=N -dPDFACompatibilityPolicy=1`, OCRmyPDF
//! `generate_pdfa`). Here the lopdf document is cleaned according to the core rules of ISO 19005-1/-2/-3 (level B);
//! what cannot be fixed (e.g. non-embedded fonts without a font file)
//! is reported by [`check`]. veraPDF is not available locally – the check covers the most important rules
//! (list in docs/contract/pl-ocr.md §9).
//!
//! Like Ghostscript with `PDFACompatibilityPolicy=1`, disallowed parts are removed
//! (JavaScript, forbidden actions, additional actions, embedded files for PDF/A-1/2, transparency
//! for PDF/A-1 …) instead of aborting the conversion.

use std::collections::{HashMap, HashSet};

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::pdfimage::{self, ColorSpace};

/// Result of the cleanup (for the log).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NormalizeReport {
    pub fixes: Vec<String>,
}

impl NormalizeReport {
    fn add(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.fixes.contains(&s) {
            self.fixes.push(s);
        }
    }
}

/// Violation of a PDF/A rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Rule (ISO 19005-2 section or short identifier).
    pub rule: &'static str,
    pub detail: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.rule, self.detail)
    }
}

fn name(o: &Object) -> Option<&[u8]> {
    o.as_name().ok()
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

fn dict_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    match deref(doc, o) {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

/// Forbidden action types.
fn forbidden_action(part: u8, s: &[u8]) -> bool {
    let common: &[&[u8]] = &[b"Launch", b"Sound", b"Movie", b"ResetForm", b"ImportData", b"JavaScript"];
    let p2: &[&[u8]] = &[b"Hide", b"SetOCGState", b"Rendition", b"Trans", b"GoTo3DView"];
    // PDF 1.2 legacy (PDF/A-1 6.6.1)
    let p1: &[&[u8]] = &[b"set-state", b"no-op", b"Hide"];
    common.contains(&s) || (part >= 2 && p2.contains(&s)) || (part == 1 && p1.contains(&s))
}

fn action_forbidden(doc: &Document, part: u8, a: &Object) -> bool {
    let Some(d) = dict_of(doc, a) else { return false };
    match d.get(b"S").ok().and_then(name) {
        Some(b"Named") => {
            let n = d.get(b"N").ok().and_then(name).unwrap_or(b"");
            !matches!(n, b"NextPage" | b"PrevPage" | b"FirstPage" | b"LastPage")
        }
        Some(s) => forbidden_action(part, s),
        None => false,
    }
}

const ANNOTS_P1: &[&[u8]] = &[
    b"Text",
    b"Link",
    b"FreeText",
    b"Line",
    b"Square",
    b"Circle",
    b"Highlight",
    b"Underline",
    b"Squiggly",
    b"StrikeOut",
    b"Stamp",
    b"Ink",
    b"Popup",
    b"Widget",
    b"PrinterMark",
    b"TrapNet",
];

fn annot_allowed(part: u8, subtype: &[u8]) -> bool {
    match part {
        1 => ANNOTS_P1.contains(&subtype),
        _ => !matches!(subtype, b"3D" | b"Sound" | b"Screen" | b"Movie" | b"RichMedia") && (part == 3 || subtype != b"FileAttachment"),
    }
}

const BLEND_MODES: &[&[u8]] = &[
    b"Normal",
    b"Compatible",
    b"Multiply",
    b"Screen",
    b"Overlay",
    b"Darken",
    b"Lighten",
    b"ColorDodge",
    b"ColorBurn",
    b"HardLight",
    b"SoftLight",
    b"Difference",
    b"Exclusion",
    b"Hue",
    b"Saturation",
    b"Color",
    b"Luminosity",
];

const INTENTS: &[&[u8]] = &[b"RelativeColorimetric", b"AbsoluteColorimetric", b"Perceptual", b"Saturation"];

fn is_extgstate(d: &Dictionary) -> bool {
    matches!(d.get(b"Type").ok().and_then(name), Some(b"ExtGState"))
}

fn rect_area(doc: &Document, d: &Dictionary) -> f64 {
    let Some(a) = d.get(b"Rect").ok().map(|o| deref(doc, o)).and_then(|o| o.as_array().ok()) else { return 0.0 };
    let v: Vec<f64> = a
        .iter()
        .filter_map(|o| match deref(doc, o) {
            Object::Integer(i) => Some(*i as f64),
            Object::Real(r) => Some(f64::from(*r)),
            _ => None,
        })
        .collect();
    if v.len() != 4 {
        return 0.0;
    }
    ((v[2] - v[0]) * (v[3] - v[1])).abs()
}

fn rect_wh(doc: &Document, d: &Dictionary) -> (f64, f64) {
    let a: Vec<f64> = d
        .get(b"Rect")
        .ok()
        .map(|o| deref(doc, o))
        .and_then(|o| o.as_array().ok())
        .map(|a| {
            a.iter()
                .filter_map(|o| match deref(doc, o) {
                    Object::Integer(i) => Some(*i as f64),
                    Object::Real(r) => Some(f64::from(*r)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if a.len() != 4 { (0.0, 0.0) } else { ((a[2] - a[0]).abs(), (a[3] - a[1]).abs()) }
}

// ----------------------------------------------------------------------------------------------
// Cleanup
// ----------------------------------------------------------------------------------------------

/// Clean up the document for PDF/A `part` (before XMP/OutputIntent in `pdfwrite::finish`).
pub fn normalize(doc: &mut Document, part: u8) -> NormalizeReport {
    let mut r = NormalizeReport::default();
    catalog_fixes(doc, part, &mut r);
    remove_forbidden_actions(doc, part, &mut r);
    stream_fixes(doc, part, &mut r);
    annotation_fixes(doc, part, &mut r);
    font_fixes(doc, &mut r);
    // Gap E3: replace non-embedded fonts with embedded substitute fonts
    for m in crate::pdfa_fonts::embed_missing(doc) {
        r.add(m);
    }
    if part == 1 {
        flatten_soft_masks(doc, &mut r);
    }
    r
}

fn catalog_fixes(doc: &mut Document, part: u8, r: &mut NormalizeReport) {
    let names_ref = doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).and_then(|o| o.as_reference().ok());
    let acro_ref = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()).and_then(|o| o.as_reference().ok());
    let oc_ref = doc.catalog().ok().and_then(|c| c.get(b"OCProperties").ok()).and_then(|o| o.as_reference().ok());
    let drop_files = part < 3;
    let fix_names = |d: &mut Dictionary, r: &mut NormalizeReport| {
        if d.remove(b"JavaScript").is_some() {
            r.add("removed document JavaScript");
        }
        if drop_files && d.remove(b"EmbeddedFiles").is_some() {
            r.add("removed embedded files (not allowed in PDF/A-1/-2)");
        }
    };
    let fix_acro = |d: &mut Dictionary, r: &mut NormalizeReport| {
        if d.remove(b"NeedAppearances").is_some() {
            r.add("removed AcroForm NeedAppearances");
        }
        if d.remove(b"XFA").is_some() {
            r.add("removed XFA form data");
        }
    };
    let fix_oc = |d: &mut Dictionary, r: &mut NormalizeReport| {
        let fix_cfg = |c: &mut Dictionary| {
            if !c.has(b"Name") {
                c.set("Name", Object::String(b"Default".to_vec(), lopdf::StringFormat::Literal));
            }
            c.remove(b"AS");
        };
        if let Ok(Object::Dictionary(dd)) = d.get_mut(b"D") {
            fix_cfg(dd);
        }
        if let Ok(Object::Array(cfgs)) = d.get_mut(b"Configs") {
            for c in cfgs.iter_mut() {
                if let Object::Dictionary(c) = c {
                    fix_cfg(c);
                }
            }
        }
        r.add("optional content configurations named, AS removed");
    };
    if let Some(id) = names_ref
        && let Ok(d) = doc.get_dictionary_mut(id)
    {
        fix_names(d, r);
    }
    if let Some(id) = acro_ref
        && let Ok(d) = doc.get_dictionary_mut(id)
    {
        fix_acro(d, r);
    }
    if part >= 2
        && let Some(id) = oc_ref
        && let Ok(d) = doc.get_dictionary_mut(id)
    {
        fix_oc(d, r);
    }
    let Ok(cat) = doc.catalog_mut() else { return };
    if cat.remove(b"AA").is_some() {
        r.add("removed catalog additional actions");
    }
    for k in [b"Requirements".as_slice(), b"Perms"] {
        cat.remove(k);
    }
    if let Ok(Object::Dictionary(n)) = cat.get_mut(b"Names") {
        fix_names(n, r);
    }
    if let Ok(Object::Dictionary(a)) = cat.get_mut(b"AcroForm") {
        fix_acro(a, r);
    }
    if part == 1 {
        if cat.remove(b"OCProperties").is_some() {
            r.add("removed optional content (not allowed in PDF/A-1)");
        }
    } else if let Ok(Object::Dictionary(oc)) = cat.get_mut(b"OCProperties") {
        fix_oc(oc, r);
    }
}

fn remove_forbidden_actions(doc: &mut Document, part: u8, r: &mut NormalizeReport) {
    // 1. collect action objects with a forbidden type
    let bad: HashSet<ObjectId> = doc.objects.iter().filter(|(_, o)| action_forbidden(doc, part, o)).map(|(id, _)| *id).collect();
    // 2. remove references to them and direct forbidden actions, AA everywhere gone
    let bad_ref = bad.clone();
    let check = move |o: &Object| -> bool {
        match o {
            Object::Reference(id) => bad_ref.contains(id),
            Object::Dictionary(d) => match d.get(b"S").ok().and_then(name) {
                Some(b"Named") => !matches!(d.get(b"N").ok().and_then(name), Some(b"NextPage" | b"PrevPage" | b"FirstPage" | b"LastPage")),
                Some(s) => forbidden_action(part, s),
                None => false,
            },
            _ => false,
        }
    };
    let removed = std::cell::Cell::new(0usize);
    doc.traverse_objects(|o| {
        let d = match o {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &mut s.dict,
            _ => return,
        };
        if d.remove(b"AA").is_some() {
            removed.set(removed.get() + 1);
        }
        for k in [b"A".as_slice(), b"OpenAction"] {
            if d.get(k).is_ok_and(&check) {
                d.remove(k);
                removed.set(removed.get() + 1);
            }
        }
        if let Ok(next) = d.get_mut(b"Next") {
            match next {
                Object::Array(a) => {
                    let n = a.len();
                    a.retain(|x| !check(x));
                    if a.len() != n {
                        removed.set(removed.get() + 1);
                    }
                }
                other if check(other) => {
                    *other = Object::Null;
                    removed.set(removed.get() + 1);
                }
                _ => {}
            }
        }
    });
    if removed.get() > 0 {
        r.add(format!("removed {} forbidden or additional action(s)", removed.get()));
    }
}

fn stream_fixes(doc: &mut Document, part: u8, r: &mut NormalizeReport) {
    let ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
    for id in ids {
        let Some(obj) = doc.objects.get_mut(&id) else { continue };
        match obj {
            Object::Stream(s) => {
                for k in [b"F".as_slice(), b"FFilter", b"FDecodeParms"] {
                    if s.dict.remove(k).is_some() {
                        r.add("removed external stream references (F/FFilter/FDecodeParms)");
                    }
                }
                let filters = pdfimage::filters(&s.dict);
                if filters.iter().any(|f| f.as_slice() == b"LZWDecode")
                    && let Ok(plain) = s.decompressed_content_with_limit(pdfimage::MAX_DECODED)
                {
                    s.dict.remove(b"Filter");
                    s.dict.remove(b"DecodeParms");
                    s.set_content(pdfimage::zlib(&plain, 6));
                    s.dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
                    s.allows_compression = false;
                    r.add("re-encoded LZW streams with Flate");
                }
                let subtype = s.dict.get(b"Subtype").ok().and_then(name).map(<[u8]>::to_vec);
                match subtype.as_deref() {
                    Some(b"Image") => {
                        for k in [b"Alternates".as_slice(), b"OPI"] {
                            if s.dict.remove(k).is_some() {
                                r.add("removed image Alternates/OPI");
                            }
                        }
                        if matches!(s.dict.get(b"Interpolate"), Ok(Object::Boolean(true))) {
                            s.dict.set("Interpolate", false);
                            r.add("image Interpolate set to false");
                        }
                        if let Ok(ri) = s.dict.get(b"Intent")
                            && !name(ri).is_some_and(|n| INTENTS.contains(&n))
                        {
                            s.dict.set("Intent", Object::Name(b"RelativeColorimetric".to_vec()));
                        }
                    }
                    Some(b"Form") => {
                        for k in [b"OPI".as_slice(), b"Ref", b"PS", b"Subtype2"] {
                            if s.dict.remove(k).is_some() {
                                r.add("removed form XObject OPI/Ref/PostScript");
                            }
                        }
                        if part == 1 && remove_transparency_group(&mut s.dict) {
                            r.add("removed transparency groups (PDF/A-1)");
                        }
                    }
                    _ => {}
                }
            }
            Object::Dictionary(d) => {
                if matches!(d.get(b"Type").ok().and_then(name), Some(b"Page")) && part == 1 && remove_transparency_group(d) {
                    r.add("removed transparency groups (PDF/A-1)");
                }
                if is_extgstate(d) || d.has(b"TR") || d.has(b"BM") || d.has(b"ca") || d.has(b"CA") {
                    fix_extgstate(d, part, r);
                }
            }
            _ => {}
        }
    }
    // ExtGState as direct dictionaries in resources
    let res_ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
    for id in res_ids {
        let Some(obj) = doc.objects.get_mut(&id) else { continue };
        let d = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &mut s.dict,
            _ => continue,
        };
        let Some(res) = (match d.get_mut(b"Resources") {
            Ok(Object::Dictionary(res)) => Some(res),
            _ => None,
        }) else {
            continue;
        };
        if let Ok(Object::Dictionary(gs)) = res.get_mut(b"ExtGState") {
            for (_, v) in gs.iter_mut() {
                if let Object::Dictionary(g) = v {
                    fix_extgstate(g, part, r);
                }
            }
        }
    }
    // ExtGState collections as separate objects
    let gs_dicts: Vec<ObjectId> = doc
        .objects
        .values()
        .filter_map(|o| match o {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        })
        .filter_map(|d| match d.get(b"Resources") {
            Ok(Object::Dictionary(res)) => res.get(b"ExtGState").ok().and_then(|o| o.as_reference().ok()),
            Ok(Object::Reference(_)) => None,
            _ => None,
        })
        .collect();
    let res_refs: Vec<ObjectId> = doc
        .objects
        .values()
        .filter_map(|o| match o {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        })
        .filter_map(|d| d.get(b"Resources").ok().and_then(|o| o.as_reference().ok()))
        .collect();
    let mut more = gs_dicts;
    for rid in res_refs {
        if let Ok(res) = doc.get_dictionary(rid)
            && let Ok(Object::Reference(g)) = res.get(b"ExtGState")
        {
            more.push(*g);
        }
        if let Ok(res) = doc.get_dictionary_mut(rid)
            && let Ok(Object::Dictionary(gs)) = res.get_mut(b"ExtGState")
        {
            for (_, v) in gs.iter_mut() {
                if let Object::Dictionary(g) = v {
                    fix_extgstate(g, part, r);
                }
            }
        }
    }
    for gid in more {
        if let Ok(gs) = doc.get_dictionary_mut(gid) {
            for (_, v) in gs.iter_mut() {
                if let Object::Dictionary(g) = v {
                    fix_extgstate(g, part, r);
                }
            }
        }
    }
}

fn remove_transparency_group(d: &mut Dictionary) -> bool {
    let is_t = matches!(d.get(b"Group"), Ok(Object::Dictionary(g)) if matches!(g.get(b"S").ok().and_then(name), Some(b"Transparency")));
    if is_t {
        d.remove(b"Group");
    }
    is_t
}

fn fix_extgstate(d: &mut Dictionary, part: u8, r: &mut NormalizeReport) {
    if d.remove(b"TR").is_some() {
        r.add("removed transfer functions (TR)");
    }
    if let Ok(tr2) = d.get(b"TR2")
        && name(tr2) != Some(b"Default")
    {
        d.set("TR2", Object::Name(b"Default".to_vec()));
        r.add("transfer functions TR2 set to Default");
    }
    if d.remove(b"HTP").is_some() {
        r.add("removed HTP");
    }
    if let Ok(ri) = d.get(b"RI")
        && !name(ri).is_some_and(|n| INTENTS.contains(&n))
    {
        d.set("RI", Object::Name(b"RelativeColorimetric".to_vec()));
    }
    let bm_ok = |o: &Object| -> bool {
        match o {
            Object::Name(n) => if part == 1 { matches!(n.as_slice(), b"Normal" | b"Compatible") } else { BLEND_MODES.contains(&n.as_slice()) },
            Object::Array(a) => a.iter().all(|x| matches!(x, Object::Name(n) if if part == 1 { matches!(n.as_slice(), b"Normal" | b"Compatible") } else { BLEND_MODES.contains(&n.as_slice()) })),
            _ => false,
        }
    };
    if let Ok(bm) = d.get(b"BM")
        && !bm_ok(bm)
    {
        d.set("BM", Object::Name(b"Normal".to_vec()));
        r.add("blend modes normalized");
    }
    if part == 1 {
        if let Ok(sm) = d.get(b"SMask")
            && name(sm) != Some(b"None")
        {
            d.set("SMask", Object::Name(b"None".to_vec()));
            r.add("removed soft masks from graphics states (PDF/A-1)");
        }
        for k in [b"CA".as_slice(), b"ca"] {
            let v = d.get(k).ok().and_then(|o| match o {
                Object::Integer(i) => Some(*i as f64),
                Object::Real(r) => Some(f64::from(*r)),
                _ => None,
            });
            if v.is_some_and(|v| (v - 1.0).abs() > 1e-6) {
                d.set(k.to_vec(), Object::Real(1.0));
                r.add("constant opacity set to 1.0 (PDF/A-1)");
            }
        }
    }
}

fn empty_appearance(doc: &mut Document, w: f64, h: f64) -> ObjectId {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"XObject".to_vec()));
    d.set("Subtype", Object::Name(b"Form".to_vec()));
    d.set("BBox", vec![Object::Integer(0), Object::Integer(0), Object::Real(w as f32), Object::Real(h as f32)]);
    doc.add_object(Stream::new(d, Vec::new()))
}

fn annotation_fixes(doc: &mut Document, part: u8, r: &mut NormalizeReport) {
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    for pid in pages {
        // read the Annots array (direct or reference)
        let annots: Vec<Object> = match doc.get_dictionary(pid).ok().and_then(|d| d.get(b"Annots").ok()) {
            Some(o) => match deref(doc, o) {
                Object::Array(a) => a.clone(),
                _ => continue,
            },
            None => continue,
        };
        let mut keep = Vec::with_capacity(annots.len());
        for a in annots {
            let Some(ad) = dict_of(doc, &a).cloned() else { continue };
            let subtype = ad.get(b"Subtype").ok().and_then(name).unwrap_or(b"").to_vec();
            if !annot_allowed(part, &subtype) {
                r.add(format!("removed {} annotation(s)", String::from_utf8_lossy(&subtype)));
                continue;
            }
            let mut nd = ad.clone();
            // flags: Print on, Hidden/Invisible/NoView/ToggleNoView off
            let f = nd.get(b"F").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0);
            let nf = (f | 4) & !(1 | 2 | 32 | 256);
            if nf != f || !nd.has(b"F") {
                nd.set("F", nf);
                r.add("annotation flags set to printable");
            }
            if part == 1 && nd.has(b"CA") {
                nd.remove(b"CA");
                r.add("removed annotation opacity (PDF/A-1)");
            }
            // appearance: N only
            let ap = nd.get(b"AP").ok().map(|o| deref(doc, o).clone());
            match ap {
                Some(Object::Dictionary(mut apd)) => {
                    if apd.remove(b"D").is_some() | apd.remove(b"R").is_some() {
                        r.add("annotation appearances reduced to normal appearance");
                    }
                    if apd.has(b"N") {
                        nd.set("AP", apd);
                    } else if part >= 2 {
                        let (w, h) = rect_wh(doc, &nd);
                        let ap_id = empty_appearance(doc, w, h);
                        let mut apd = Dictionary::new();
                        apd.set("N", Object::Reference(ap_id));
                        nd.set("AP", apd);
                    }
                }
                _ if part >= 2 && !matches!(subtype.as_slice(), b"Popup" | b"Link") && rect_area(doc, &nd) > 0.0 => {
                    let (w, h) = rect_wh(doc, &nd);
                    let ap_id = empty_appearance(doc, w, h);
                    let mut apd = Dictionary::new();
                    apd.set("N", Object::Reference(ap_id));
                    nd.set("AP", apd);
                    r.add("added empty appearance streams to annotations without appearance");
                }
                _ => {}
            }
            match a {
                Object::Reference(id) => {
                    doc.objects.insert(id, Object::Dictionary(nd));
                    keep.push(Object::Reference(id));
                }
                _ => keep.push(Object::Dictionary(nd)),
            }
        }
        if let Ok(d) = doc.get_dictionary_mut(pid) {
            if keep.is_empty() {
                d.remove(b"Annots");
            } else {
                d.set("Annots", keep);
            }
        }
    }
    if part == 3 {
        embedded_files_pdfa3(doc, r);
    }
}

/// PDF/A-3: embedded files need AFRelationship, UF, Subtype (MIME) and a reference from /AF.
fn embedded_files_pdfa3(doc: &mut Document, r: &mut NormalizeReport) {
    let specs: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, o)| match o {
            Object::Dictionary(d) if d.has(b"EF") => Some(*id),
            _ => None,
        })
        .collect();
    if specs.is_empty() {
        return;
    }
    let mut ef_streams = Vec::new();
    for id in &specs {
        if let Ok(d) = doc.get_dictionary_mut(*id) {
            if !d.has(b"AFRelationship") {
                d.set("AFRelationship", Object::Name(b"Unspecified".to_vec()));
            }
            if !d.has(b"UF")
                && let Ok(f) = d.get(b"F").cloned()
            {
                d.set("UF", f);
            }
            if let Ok(Object::Dictionary(ef)) = d.get(b"EF") {
                for (_, v) in ef.iter() {
                    if let Object::Reference(s) = v {
                        ef_streams.push(*s);
                    }
                }
            }
        }
    }
    for s in ef_streams {
        if let Ok(Object::Stream(st)) = doc.get_object_mut(s)
            && !st.dict.has(b"Subtype")
        {
            st.dict.set("Subtype", Object::Name(b"application#2Foctet-stream".to_vec()));
        }
    }
    if let Ok(cat) = doc.catalog_mut() {
        let mut af: Vec<Object> = match cat.get(b"AF") {
            Ok(Object::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        for id in specs {
            if !af.contains(&Object::Reference(id)) {
                af.push(Object::Reference(id));
            }
        }
        cat.set("AF", af);
    }
    r.add("embedded files marked with AFRelationship (PDF/A-3)");
}

/// Replace a non-embedded "GlyphLessFont" (text layers of other OCR programs or older
/// PaperRust archives) with the embedded version.
fn font_fixes(doc: &mut Document, r: &mut NormalizeReport) {
    let cid_fonts: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, o)| {
            let d = o.as_dict().ok()?;
            let is = matches!(d.get(b"Subtype").ok().and_then(name), Some(b"CIDFontType2"))
                && d.get(b"BaseFont").ok().and_then(name).is_some_and(|n| n.ends_with(b"GlyphLessFont"));
            let fd = d.get(b"FontDescriptor").ok().and_then(|o| dict_of(doc, o))?;
            let embedded = fd.has(b"FontFile2") || fd.has(b"FontFile3") || fd.has(b"FontFile");
            (is && !embedded).then_some(*id)
        })
        .collect();
    if cid_fonts.is_empty() {
        return;
    }
    let ttf = crate::glyphless::glyphless_ttf();
    let mut ff = Dictionary::new();
    ff.set("Length1", ttf.len() as i64);
    let ff_id = doc.add_object(Stream::new(ff, ttf));
    let c2g = doc.add_object(Stream::new(Dictionary::new(), crate::glyphless::cid_to_gid_map(u16::MAX)));
    for id in cid_fonts {
        let fd_ref = doc.get_dictionary(id).ok().and_then(|d| d.get(b"FontDescriptor").ok()).cloned();
        match fd_ref {
            Some(Object::Reference(fid)) => {
                if let Ok(fd) = doc.get_dictionary_mut(fid) {
                    fd.set("FontFile2", Object::Reference(ff_id));
                }
            }
            Some(Object::Dictionary(mut fd)) => {
                fd.set("FontFile2", Object::Reference(ff_id));
                if let Ok(d) = doc.get_dictionary_mut(id) {
                    d.set("FontDescriptor", fd);
                }
            }
            _ => continue,
        }
        if let Ok(d) = doc.get_dictionary_mut(id) {
            d.set("CIDToGIDMap", Object::Reference(c2g));
            d.set("DW", i64::from(crate::glyphless::ADVANCE));
            d.remove(b"W");
        }
        r.add("embedded GlyphLessFont for existing invisible text layer");
    }
}

/// PDF/A-1: blend images with `/SMask` onto white (Ghostscript flattens transparency likewise).
fn flatten_soft_masks(doc: &mut Document, r: &mut NormalizeReport) {
    let (ids, _) = pdfimage::image_ids(doc);
    for id in ids {
        let Ok(Object::Stream(s)) = doc.get_object(id) else { continue };
        let Some(mask_ref) = s.dict.get(b"SMask").ok().and_then(|o| o.as_reference().ok()) else { continue };
        let Some(px) = pdfimage::decode(doc, s) else { continue };
        let Ok(Object::Stream(ms)) = doc.get_object(mask_ref) else { continue };
        let Some(mask) = pdfimage::decode(doc, ms) else { continue };
        let Some(px) = pdfimage::expand_indexed(&px) else { continue };
        if mask.bpc != 8 || mask.comps() != 1 || px.bpc != 8 || px.cs.device().is_none() {
            continue;
        }
        let n = px.comps();
        let (w, h) = (px.width as usize, px.height as usize);
        let (mw, mh) = (mask.width as usize, mask.height as usize);
        let cmyk = px.cs.device() == Some(ColorSpace::Cmyk);
        let mut out = px.data.clone();
        for y in 0..h {
            let my = y * mh / h;
            for x in 0..w {
                let mx = x * mw / w;
                let a = u32::from(mask.data[my * mw + mx]);
                for c in 0..n {
                    let v = u32::from(out[(y * w + x) * n + c]);
                    // white: 255 (gray/RGB) or 0 (CMYK)
                    let white = if cmyk { 0 } else { 255 };
                    out[(y * w + x) * n + c] = ((v * a + white * (255 - a) + 127) / 255) as u8;
                }
            }
        }
        let flat = pdfimage::Pixels { data: out, ..px.clone() };
        let mut enc = if flat.was_jpeg {
            pdfimage::encode_jpeg(&flat, crate::color::RECODE_JPEG_QUALITY).unwrap_or_else(|| pdfimage::encode_flate(&flat, 6))
        } else {
            pdfimage::encode_flate(&flat, 6)
        };
        if let Ok(Object::Stream(s)) = doc.get_object(id)
            && let Ok(cs) = s.dict.get(b"ColorSpace")
            && !matches!(flat.cs, ColorSpace::Indexed { .. })
            && matches!(pdfimage::parse_colorspace(doc, cs), ColorSpace::Icc { .. })
        {
            enc.cs = cs.clone();
        }
        if let Ok(Object::Stream(s)) = doc.get_object_mut(id) {
            pdfimage::apply(s, enc);
            s.dict.remove(b"SMask");
            r.add("flattened image soft masks onto white (PDF/A-1)");
        }
    }
    // Remaining SMask references (not decodable) stay → the check reports them.
}

// ----------------------------------------------------------------------------------------------
// Check
// ----------------------------------------------------------------------------------------------

/// Use of device color spaces and fonts (from content streams).
#[derive(Debug, Default)]
struct Usage {
    rgb: bool,
    cmyk: bool,
    /// Font objects that are used visibly (Tr ≠ 3).
    visible_fonts: HashSet<ObjectId>,
    /// direct (non-referenced) fonts with visible use
    visible_direct_font: bool,
    inline_cmyk: bool,
    inline_rgb: bool,
}

fn res_dict<'a>(doc: &'a Document, o: Option<&'a Object>) -> Option<&'a Dictionary> {
    o.and_then(|o| dict_of(doc, o))
}

fn page_res(doc: &Document, page: ObjectId) -> Option<&Dictionary> {
    let mut id = page;
    for _ in 0..64 {
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(r) = d.get(b"Resources") {
            return dict_of(doc, r);
        }
        id = d.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn scan_ops(doc: &Document, content: &[u8], res: Option<&Dictionary>, u: &mut Usage) {
    let Ok(c) = Content::decode(content) else { return };
    let mut tr = 0i64;
    let mut font: Option<Vec<u8>> = None;
    let mut stack: Vec<(i64, Option<Vec<u8>>)> = Vec::new();
    for op in &c.operations {
        match op.operator.as_str() {
            "q" => stack.push((tr, font.clone())),
            "Q" => {
                if let Some((t, f)) = stack.pop() {
                    tr = t;
                    font = f;
                }
            }
            "Tr" => tr = op.operands.first().and_then(|o| o.as_i64().ok()).unwrap_or(0),
            "Tf" => font = op.operands.first().and_then(|o| o.as_name().ok()).map(<[u8]>::to_vec),
            "Tj" | "TJ" | "'" | "\"" if tr != 3 => {
                if let Some(f) = &font {
                    match res.and_then(|r| r.get(b"Font").ok()).and_then(|o| dict_of(doc, o)).and_then(|fd| fd.get(f).ok()) {
                        Some(Object::Reference(id)) => {
                            u.visible_fonts.insert(*id);
                        }
                        Some(_) => u.visible_direct_font = true,
                        None => {}
                    }
                }
            }
            "rg" | "RG" => u.rgb = true,
            "k" | "K" => u.cmyk = true,
            "cs" | "CS" => match op.operands.first().and_then(|o| o.as_name().ok()) {
                Some(b"DeviceRGB") => u.rgb = true,
                Some(b"DeviceCMYK") => u.cmyk = true,
                _ => {}
            },
            "BI" => {
                if let Some(Object::Stream(s)) = op.operands.first() {
                    let cs = s.dict.get(b"CS").or_else(|_| s.dict.get(b"ColorSpace")).ok().and_then(name);
                    match cs {
                        Some(b"RGB" | b"DeviceRGB") => u.inline_rgb = true,
                        Some(b"CMYK" | b"DeviceCMYK") => u.inline_cmyk = true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

/// Device color spaces in a color space object (incl. Indexed base, alternates).
fn cs_usage(doc: &Document, o: &Object, u: &mut Usage, depth: u8) {
    if depth > 8 {
        return;
    }
    match deref(doc, o) {
        Object::Name(n) => match n.as_slice() {
            b"DeviceRGB" | b"RGB" => u.rgb = true,
            b"DeviceCMYK" | b"CMYK" => u.cmyk = true,
            _ => {}
        },
        Object::Array(a) => match a.first().and_then(name) {
            Some(b"ICCBased") => {}
            Some(b"Indexed" | b"I") => {
                if let Some(b) = a.get(1) {
                    cs_usage(doc, b, u, depth + 1);
                }
            }
            Some(b"Separation") => {
                if let Some(b) = a.get(2) {
                    cs_usage(doc, b, u, depth + 1);
                }
            }
            Some(b"DeviceN") => {
                if let Some(b) = a.get(2) {
                    cs_usage(doc, b, u, depth + 1);
                }
            }
            Some(b"Pattern") => {
                if let Some(b) = a.get(1) {
                    cs_usage(doc, b, u, depth + 1);
                }
            }
            _ => {}
        },
        _ => {}
    }
}

fn font_embedded(doc: &Document, f: &Dictionary) -> bool {
    match f.get(b"Subtype").ok().and_then(name) {
        Some(b"Type3") => true,
        Some(b"Type0") => f
            .get(b"DescendantFonts")
            .ok()
            .map(|o| deref(doc, o))
            .and_then(|o| o.as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| dict_of(doc, o))
            .is_some_and(|d| font_embedded(doc, d)),
        _ => f
            .get(b"FontDescriptor")
            .ok()
            .and_then(|o| dict_of(doc, o))
            .is_some_and(|fd| fd.has(b"FontFile") || fd.has(b"FontFile2") || fd.has(b"FontFile3")),
    }
}

fn icc_header_ok(p: &[u8], part: u8) -> Result<(u8, [u8; 4]), String> {
    if p.len() < 132 || &p[36..40] != b"acsp" {
        return Err("ICC profile header invalid".into());
    }
    let size = u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as usize;
    if size != p.len() {
        return Err(format!("ICC profile size {size} != stream length {}", p.len()));
    }
    let major = p[8];
    if part == 1 && major >= 4 {
        return Err("ICC profile version ≥ 4 not allowed in PDF/A-1".into());
    }
    if major > 4 {
        return Err("ICC profile version > 4".into());
    }
    let class: [u8; 4] = [p[12], p[13], p[14], p[15]];
    let cs: [u8; 4] = [p[16], p[17], p[18], p[19]];
    if !matches!(&class, b"mntr" | b"prtr") {
        return Err(format!("output intent profile class {} not allowed", String::from_utf8_lossy(&class)));
    }
    let n = match &cs {
        b"RGB " => 3,
        b"CMYK" => 4,
        b"GRAY" => 1,
        _ => return Err(format!("output intent profile colour space {}", String::from_utf8_lossy(&cs))),
    };
    Ok((n, cs))
}

/// Simple XMP reader: text of an element (without namespace check) or attribute value.
fn xmp_value(xmp: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    if let Some(i) = xmp.find(&open) {
        let rest = &xmp[i + open.len()..];
        let end = rest.find(&format!("</{tag}>"))?;
        let inner = &rest[..end];
        // rdf:Alt/Seq/li → first entry
        if let Some(li) = inner.find("<rdf:li") {
            let after = &inner[li..];
            let s = after.find('>')? + 1;
            let e = after.find("</rdf:li>")?;
            return Some(xml_unescape(&after[s..e]));
        }
        return Some(xml_unescape(inner.trim()));
    }
    let attr = format!("{tag}=\"");
    let i = xmp.find(&attr)?;
    let rest = &xmp[i + attr.len()..];
    Some(xml_unescape(&rest[..rest.find('"')?]))
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn pdf_text(o: &Object) -> Option<String> {
    let Object::String(b, _) = o else { return None };
    if b.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = b[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
        return Some(String::from_utf16_lossy(&u));
    }
    Some(b.iter().map(|&c| c as char).collect())
}

/// Point in time (seconds since epoch, for comparison only) from ISO 8601 or PDF date.
fn instant(iso: &str) -> Option<i64> {
    let s = iso.trim();
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp());
    dt.or_else(|| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok().map(|d| d.and_utc().timestamp()))
}

/// PDF/A check of a finished PDF (bytes) against the core rules. Empty list = no violations
/// found (not a complete veraPDF check).
pub fn check(data: &[u8], part: u8) -> Vec<Violation> {
    let mut v = Vec::new();
    let mut add = |rule: &'static str, detail: String| {
        if !v.iter().any(|x: &Violation| x.rule == rule && x.detail == detail) {
            v.push(Violation { rule, detail });
        }
    };
    // 6.1.2 file header
    let header_ok = data.starts_with(b"%PDF-1.");
    let version = data.get(7).copied().unwrap_or(b'0');
    if !header_ok {
        add("6.1.2", "file header is not %PDF-1.n".into());
    } else if part == 1 && version > b'4' {
        add("6.1.2", format!("PDF version 1.{} not allowed in PDF/A-1", version as char));
    }
    let second = data.iter().position(|&b| b == b'\n').map(|i| &data[i + 1..]).unwrap_or(&[]);
    if !(second.first() == Some(&b'%') && second.iter().skip(1).take(4).all(|&b| b > 127)) {
        add("6.1.2", "binary comment after header missing".into());
    }
    if part == 1 && (contains(data, b"/ObjStm") || contains(data, b"/XRef")) {
        add("6.1.4", "object or cross-reference streams not allowed in PDF/A-1".into());
    }
    let doc = match Document::load_mem(data) {
        Ok(d) => d,
        Err(e) => {
            add("parse", format!("PDF cannot be parsed: {e}"));
            return v;
        }
    };
    // 6.1.3 Trailer
    if doc.trailer.has(b"Encrypt") {
        add("6.1.3", "file is encrypted".into());
    }
    if !matches!(doc.trailer.get(b"ID"), Ok(Object::Array(a)) if a.len() == 2) {
        add("6.1.3", "trailer ID missing".into());
    }
    let Ok(cat) = doc.catalog() else {
        add("parse", "no catalog".into());
        return v;
    };
    // 6.6.2 metadata
    let xmp = cat.get(b"Metadata").ok().and_then(|o| deref(&doc, o).as_stream().ok());
    match xmp {
        None => add("6.6.2.1", "catalog has no XMP metadata stream".into()),
        Some(s) => {
            if part == 1 && s.dict.has(b"Filter") {
                add("6.7.2", "metadata stream must not be filtered in PDF/A-1".into());
            }
            let raw = s.decompressed_content().unwrap_or_default();
            let x = String::from_utf8_lossy(&raw).into_owned();
            if !x.contains("<?xpacket begin=") {
                add("6.6.2.1", "XMP packet header missing".into());
            }
            match xmp_value(&x, "pdfaid:part") {
                Some(p) if p.trim() == part.to_string() => {}
                other => add("6.6.4", format!("pdfaid:part is {other:?}, expected {part}")),
            }
            match xmp_value(&x, "pdfaid:conformance").as_deref().map(str::trim) {
                Some("B" | "A") => {}
                Some("U") if part >= 2 => {}
                other => add("6.6.4", format!("pdfaid:conformance is {other:?}")),
            }
            // 6.6.3 Info ↔ XMP
            if let Some(info) = doc.trailer.get(b"Info").ok().and_then(|o| dict_of(&doc, o)) {
                let pairs: [(&[u8], &str); 6] = [
                    (b"Title", "dc:title"),
                    (b"Author", "dc:creator"),
                    (b"Subject", "dc:description"),
                    (b"Keywords", "pdf:Keywords"),
                    (b"Creator", "xmp:CreatorTool"),
                    (b"Producer", "pdf:Producer"),
                ];
                for (k, tag) in pairs {
                    if let Some(val) = info.get(k).ok().and_then(|o| pdf_text(deref(&doc, o))) {
                        match xmp_value(&x, tag) {
                            Some(xv) if xv == val || (tag == "dc:title" || tag == "dc:description") && xv.trim() == val.trim() => {}
                            other => add("6.6.3", format!("Info {} = {val:?} but XMP {tag} = {other:?}", String::from_utf8_lossy(k))),
                        }
                    }
                }
                for (k, tag) in [(b"CreationDate".as_slice(), "xmp:CreateDate"), (b"ModDate", "xmp:ModifyDate")] {
                    if let Some(val) = info.get(k).ok().and_then(|o| pdf_text(deref(&doc, o))) {
                        let a = crate::pdfwrite::pdf_date_to_xmp(&val).and_then(|d| instant(&d));
                        let b = xmp_value(&x, tag).and_then(|d| instant(&d));
                        if a.is_none() || a != b {
                            add("6.6.3", format!("Info {} does not match XMP {tag}", String::from_utf8_lossy(k)));
                        }
                    }
                }
            }
        }
    }
    // 6.2.3 OutputIntent
    let mut intent_n = None;
    let intents = cat.get(b"OutputIntents").ok().map(|o| deref(&doc, o)).and_then(|o| o.as_array().ok());
    let mut profiles = HashSet::new();
    for oi in intents.map(|a| a.as_slice()).unwrap_or(&[]) {
        let Some(d) = dict_of(&doc, oi) else { continue };
        if !matches!(d.get(b"S").ok().and_then(name), Some(b"GTS_PDFA1")) {
            continue;
        }
        match d.get(b"DestOutputProfile") {
            Ok(Object::Reference(pid)) => {
                profiles.insert(*pid);
                match doc.get_object(*pid).ok().and_then(|o| o.as_stream().ok()).map(|s| s.decompressed_content().unwrap_or_default()) {
                    Some(p) => match icc_header_ok(&p, part) {
                        Ok((n, _)) => {
                            intent_n = Some(n);
                            let dn = doc
                                .get_object(*pid)
                                .ok()
                                .and_then(|o| o.as_stream().ok())
                                .and_then(|s| s.dict.get(b"N").ok())
                                .and_then(|o| o.as_i64().ok());
                            if dn != Some(i64::from(n)) {
                                add("6.2.3", "ICC profile /N does not match profile colour space".into());
                            }
                        }
                        Err(e) => add("6.2.3", e),
                    },
                    None => add("6.2.3", "DestOutputProfile is not a stream".into()),
                }
            }
            _ => add("6.2.3", "GTS_PDFA1 output intent without DestOutputProfile".into()),
        }
    }
    if profiles.len() > 1 {
        add("6.2.3", "output intents use different profiles".into());
    }
    // content and resources
    let mut u = Usage::default();
    for pid in doc.get_pages().values() {
        let res = page_res(&doc, *pid);
        scan_ops(&doc, &doc.get_page_content(*pid), res, &mut u);
        if let Ok(pd) = doc.get_dictionary(*pid) {
            if part == 1
                && matches!(
                    pd.get(b"Group").ok().and_then(|o| dict_of(&doc, o)).and_then(|g| g.get(b"S").ok()).and_then(name),
                    Some(b"Transparency")
                )
            {
                add("6.4", "page transparency group not allowed in PDF/A-1".into());
            }
            if pd.has(b"AA") {
                add("6.6.2", "page additional actions".into());
            }
            // Annotations
            if let Some(annots) = pd.get(b"Annots").ok().map(|o| deref(&doc, o)).and_then(|o| o.as_array().ok()) {
                for a in annots {
                    let Some(ad) = dict_of(&doc, a) else { continue };
                    let st = ad.get(b"Subtype").ok().and_then(name).unwrap_or(b"");
                    if !annot_allowed(part, st) {
                        add("6.3.1", format!("annotation type {} not allowed", String::from_utf8_lossy(st)));
                    }
                    let f = ad.get(b"F").ok().and_then(|o| o.as_i64().ok());
                    match f {
                        Some(f) if f & 4 != 0 && f & (1 | 2 | 32) == 0 && (part == 1 || f & 256 == 0) => {}
                        _ if st == b"Popup" && part >= 2 => {}
                        _ => add("6.3.2", format!("annotation {} flags not printable", String::from_utf8_lossy(st))),
                    }
                    if part == 1 && ad.get(b"CA").ok().and_then(|o| o.as_float().ok()).is_some_and(|c| (c - 1.0).abs() > 1e-6) {
                        add("6.5.3", "annotation opacity".into());
                    }
                    if ad.has(b"AA") {
                        add("6.6.2", "annotation additional actions".into());
                    }
                    match ad.get(b"AP").ok().and_then(|o| dict_of(&doc, o)) {
                        Some(ap) => {
                            if ap.has(b"D") || ap.has(b"R") || !ap.has(b"N") {
                                add("6.3.3", "appearance dictionary must contain only N".into());
                            }
                        }
                        None if part >= 2 && !matches!(st, b"Popup" | b"Link") && rect_area(&doc, ad) > 0.0 => {
                            add("6.3.3", format!("annotation {} without appearance stream", String::from_utf8_lossy(st)));
                        }
                        None => {}
                    }
                }
            }
        }
    }
    let mut unembedded: Vec<(ObjectId, String)> = Vec::new();
    for (oid, o) in &doc.objects {
        let (d, stream) = match o {
            Object::Dictionary(d) => (d, None),
            Object::Stream(s) => (&s.dict, Some(s)),
            _ => continue,
        };
        // Actions
        if action_forbidden(&doc, part, o) {
            let s = d.get(b"S").ok().and_then(name).unwrap_or(b"");
            add("6.6.1", format!("forbidden action {}", String::from_utf8_lossy(s)));
        }
        for k in [b"A".as_slice(), b"OpenAction"] {
            if let Ok(a) = d.get(k)
                && action_forbidden(&doc, part, a)
            {
                add("6.6.1", "forbidden action referenced".into());
            }
        }
        // resource color spaces
        if let Ok(res) = d.get(b"Resources").map(|o| deref(&doc, o))
            && let Some(cs) = res.as_dict().ok().and_then(|r| r.get(b"ColorSpace").ok()).and_then(|o| dict_of(&doc, o))
        {
            for (_, c) in cs.iter() {
                cs_usage(&doc, c, &mut u, 0);
            }
        }
        if d.get(b"ShadingType").is_ok()
            && let Ok(c) = d.get(b"ColorSpace")
        {
            cs_usage(&doc, c, &mut u, 0);
        }
        if let Ok(g) = d.get(b"Group").map(|o| deref(&doc, o))
            && let Some(gd) = g.as_dict().ok()
            && let Ok(c) = gd.get(b"CS")
        {
            cs_usage(&doc, c, &mut u, 0);
        }
        // Fonts
        if matches!(d.get(b"Type").ok().and_then(name), Some(b"Font"))
            && !matches!(d.get(b"Subtype").ok().and_then(name), Some(b"CIDFontType0" | b"CIDFontType2"))
        {
            if !font_embedded(&doc, d) {
                let bf = d.get(b"BaseFont").ok().and_then(name).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
                unembedded.push((*oid, bf));
            }
            if let Some(desc) = d
                .get(b"DescendantFonts")
                .ok()
                .map(|o| deref(&doc, o))
                .and_then(|o| o.as_array().ok())
                .and_then(|a| a.first())
                .and_then(|o| dict_of(&doc, o))
                && matches!(desc.get(b"Subtype").ok().and_then(name), Some(b"CIDFontType2"))
            {
                if !desc.has(b"CIDToGIDMap") {
                    add("6.2.11.3.2", "CIDFontType2 without CIDToGIDMap".into());
                }
                // width matching for the own GlyphLessFont
                if let Some(ff) = desc
                    .get(b"FontDescriptor")
                    .ok()
                    .and_then(|o| dict_of(&doc, o))
                    .and_then(|fd| fd.get(b"FontFile2").ok())
                    .and_then(|o| deref(&doc, o).as_stream().ok())
                    && let Ok(ttf) = ff.decompressed_content()
                    && crate::glyphless::read::table(&ttf, b"head").is_some()
                    && crate::glyphless::read::num_glyphs(&ttf) == Some(2)
                {
                    let upem = f64::from(crate::glyphless::read::units_per_em(&ttf).unwrap_or(1000).max(1));
                    let adv = f64::from(crate::glyphless::read::advance(&ttf, 1).unwrap_or(0)) * 1000.0 / upem;
                    let dw = desc.get(b"DW").ok().and_then(|o| o.as_float().ok()).unwrap_or(1000.0);
                    if (f64::from(dw) - adv).abs() > 1.0 && !desc.has(b"W") {
                        add("6.2.11.5", "glyph widths inconsistent with embedded font program".into());
                    }
                }
            }
        }
        // Streams
        if let Some(s) = stream {
            for k in [b"F".as_slice(), b"FFilter", b"FDecodeParms"] {
                if s.dict.has(k) {
                    add("6.1.7", "stream refers to external file".into());
                }
            }
            let f = pdfimage::filters(&s.dict);
            if f.iter().any(|x| x.as_slice() == b"LZWDecode") {
                add("6.1.7", "LZWDecode filter".into());
            }
            if part == 1 && f.iter().any(|x| x.as_slice() == b"JPXDecode") {
                add("6.2.8", "JPEG 2000 not allowed in PDF/A-1".into());
            }
            match s.dict.get(b"Subtype").ok().and_then(name) {
                Some(b"Image") => {
                    if let Ok(c) = s.dict.get(b"ColorSpace") {
                        cs_usage(&doc, c, &mut u, 0);
                    }
                    if matches!(s.dict.get(b"Interpolate"), Ok(Object::Boolean(true))) {
                        add("6.2.9", "image Interpolate is true".into());
                    }
                    if s.dict.has(b"Alternates") || s.dict.has(b"OPI") {
                        add("6.2.9", "image Alternates/OPI".into());
                    }
                    if part == 1 && s.dict.get(b"SMask").is_ok_and(|m| name(m) != Some(b"None")) {
                        add("6.4", "image soft mask not allowed in PDF/A-1".into());
                    }
                }
                Some(b"Form") => {
                    if s.dict.has(b"OPI")
                        || s.dict.has(b"Ref")
                        || s.dict.has(b"PS")
                        || matches!(s.dict.get(b"Subtype2").ok().and_then(name), Some(b"PS"))
                    {
                        add("6.2.10", "form XObject with OPI/Ref/PostScript".into());
                    }
                    if part == 1
                        && matches!(
                            s.dict.get(b"Group").ok().and_then(|o| dict_of(&doc, o)).and_then(|g| g.get(b"S").ok()).and_then(name),
                            Some(b"Transparency")
                        )
                    {
                        add("6.4", "form transparency group not allowed in PDF/A-1".into());
                    }
                    let res = res_dict(&doc, s.dict.get(b"Resources").ok());
                    if let Ok(c) = s.decompressed_content() {
                        scan_ops(&doc, &c, res, &mut u);
                    }
                }
                Some(b"PS") => add("6.2.10", "PostScript XObject".into()),
                _ => {}
            }
            if matches!(s.dict.get(b"PatternType"), Ok(Object::Integer(1))) {
                let res = res_dict(&doc, s.dict.get(b"Resources").ok());
                if let Ok(c) = s.decompressed_content() {
                    scan_ops(&doc, &c, res, &mut u);
                }
            }
        }
        // ExtGState
        if is_extgstate(d) {
            if d.has(b"TR") {
                add("6.2.5", "ExtGState TR".into());
            }
            if d.get(b"TR2").is_ok_and(|t| name(t) != Some(b"Default")) {
                add("6.2.5", "ExtGState TR2 other than Default".into());
            }
            if d.has(b"HTP") {
                add("6.2.5", "ExtGState HTP".into());
            }
            if part == 1 {
                if d.get(b"SMask").is_ok_and(|m| name(m) != Some(b"None")) {
                    add("6.4", "ExtGState soft mask not allowed in PDF/A-1".into());
                }
                for k in [b"CA".as_slice(), b"ca"] {
                    if d.get(k).ok().and_then(|o| o.as_float().ok()).is_some_and(|c| (c - 1.0).abs() > 1e-6) {
                        add("6.4", "constant opacity not allowed in PDF/A-1".into());
                    }
                }
                if d.get(b"BM").ok().and_then(name).is_some_and(|b| !matches!(b, b"Normal" | b"Compatible")) {
                    add("6.4", "blend mode not allowed in PDF/A-1".into());
                }
            }
        }
        // File specifications
        if d.has(b"EF") {
            if part < 3 {
                add("6.8", "embedded file not allowed (PDF/A-1/-2: only PDF/A files; not verifiable)".into());
            } else if !d.has(b"AFRelationship") {
                add("6.8", "embedded file without AFRelationship".into());
            }
        }
    }
    // Fonts. PDF/A-2/3 6.2.11.4.1: only fonts used invisibly (Tr 3) need not be
    // embedded; PDF/A-1 does not have the exception.
    for (id, bf) in unembedded {
        let visible = u.visible_fonts.contains(&id) || u.visible_direct_font;
        if part == 1 || visible {
            add("6.2.11.4", format!("font {bf} is not embedded"));
        }
    }
    // device color spaces against OutputIntent
    let mut add2 = |rule: &'static str, detail: String| {
        if !v.iter().any(|x: &Violation| x.rule == rule && x.detail == detail) {
            v.push(Violation { rule, detail });
        }
    };
    if (u.rgb || u.inline_rgb) && intent_n != Some(3) {
        add2("6.2.4.3", "DeviceRGB used without RGB output intent".into());
    }
    if (u.cmyk || u.inline_cmyk) && intent_n != Some(4) {
        add2("6.2.4.3", "DeviceCMYK used without CMYK output intent".into());
    }
    if intent_n.is_none() {
        add2("6.2.3", "no PDF/A output intent".into());
    }
    // Catalog
    if cat.get(b"Names").ok().and_then(|o| dict_of(&doc, o)).is_some_and(|n| n.has(b"JavaScript")) {
        add2("6.6.1", "document JavaScript".into());
    }
    if cat.has(b"AA") {
        add2("6.6.2", "catalog additional actions".into());
    }
    if let Some(af) = cat.get(b"AcroForm").ok().and_then(|o| dict_of(&doc, o)) {
        if matches!(af.get(b"NeedAppearances"), Ok(Object::Boolean(true))) {
            add2("6.4.1", "AcroForm NeedAppearances true".into());
        }
        if af.has(b"XFA") {
            add2("6.4.2", "XFA forms not allowed".into());
        }
    }
    if part == 1 && cat.has(b"OCProperties") {
        add2("6.1.13", "optional content not allowed in PDF/A-1".into());
    }
    if part == 1 && cat.get(b"Names").ok().and_then(|o| dict_of(&doc, o)).is_some_and(|n| n.has(b"EmbeddedFiles")) {
        add2("6.1.11", "embedded files not allowed in PDF/A-1".into());
    }
    if part == 1 {
        limits_p1(&doc, &mut v);
    }
    v
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// PDF/A-1 6.1.12 implementation limits.
fn limits_p1(doc: &Document, v: &mut Vec<Violation>) {
    let mut bad: HashMap<&'static str, usize> = HashMap::new();
    fn walk(o: &Object, bad: &mut HashMap<&'static str, usize>) {
        match o {
            Object::Integer(i) if *i > i64::from(i32::MAX) || *i < i64::from(i32::MIN) => {
                *bad.entry("integer out of range").or_default() += 1
            }
            Object::Real(r) if r.abs() > 32767.0 => *bad.entry("real out of range").or_default() += 1,
            Object::String(s, _) if s.len() > 65535 => *bad.entry("string longer than 65535 bytes").or_default() += 1,
            Object::Name(n) if n.len() > 127 => *bad.entry("name longer than 127 bytes").or_default() += 1,
            Object::Array(a) => {
                if a.len() > 8191 {
                    *bad.entry("array with more than 8191 elements").or_default() += 1;
                }
                for x in a {
                    walk(x, bad);
                }
            }
            Object::Dictionary(d) => {
                if d.len() > 4095 {
                    *bad.entry("dictionary with more than 4095 entries").or_default() += 1;
                }
                for (k, x) in d.iter() {
                    if k.len() > 127 {
                        *bad.entry("name longer than 127 bytes").or_default() += 1;
                    }
                    walk(x, bad);
                }
            }
            Object::Stream(s) => walk(&Object::Dictionary(s.dict.clone()), bad),
            _ => {}
        }
    }
    for o in doc.objects.values() {
        walk(o, &mut bad);
    }
    for (k, n) in bad {
        v.push(Violation { rule: "6.1.12", detail: format!("{k} ({n}×)") });
    }
}

/// Violations that the cleanup cannot fix and that rule out a PDF/A claim.
pub fn summarize(v: &[Violation]) -> String {
    v.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
}
