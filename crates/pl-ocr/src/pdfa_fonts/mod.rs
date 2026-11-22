//! Substitute fonts for non-embedded fonts in the PDF/A archive (gap E3).
//!
//! upstream: Ghostscript (`-dPDFA`, via OCRmyPDF) embeds a substitute font from its font directory
//! for non-embedded fonts (URW clones of the standard fonts), so that
//! the file becomes PDF/A-compliant.
//!
//! PaperRust: simple, non-symbolic fonts (`Type1`, `MMType1`, `TrueType` without
//! font file) get a **Liberation** font as an embedded TrueType subset – Sans for
//! Helvetica/Arial and unknown, Serif for Times, Mono for Courier, style (bold/italic) from
//! name or font descriptor. Liberation is metrically identical to Arial/Helvetica, Times and
//! Courier; the widths of the embedded glyphs are additionally set to the `/Widths` of the PDF
//! (PDF/A 6.2.11.5), so text and positions stay unchanged. The encoding is written as
//! `WinAnsiEncoding` + `Differences` with the same glyph names (6.2.11.6), so text
//! extraction does not change.
//!
//! Searched in `PAPERRUST_PDFA_FONT_DIR`, then in the usual Linux folders
//! (`/usr/share/fonts/liberation` – present in the Docker image – among others). Without font files it stays
//! at the previous behavior (PDF/A marking omitted, warning). Not replaced are
//! symbolic fonts (Symbol, ZapfDingbats, Wingdings …), CID fonts (`Type0`) and
//! fonts with glyph names outside the standard encodings.

mod enc;
pub mod ttf;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use self::ttf::Ttf;

/// Font family of the substitute font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Sans,
    Serif,
    Mono,
}

/// Desired substitute font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Face {
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
}

impl Face {
    /// File name of the Liberation font.
    pub fn file_name(&self) -> String {
        let fam = match self.family {
            Family::Sans => "Sans",
            Family::Serif => "Serif",
            Family::Mono => "Mono",
        };
        let style = match (self.bold, self.italic) {
            (false, false) => "Regular",
            (true, false) => "Bold",
            (false, true) => "Italic",
            (true, true) => "BoldItalic",
        };
        format!("Liberation{fam}-{style}.ttf")
    }
}

/// Folders in which Liberation is searched.
pub fn font_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = pl_envlive::var("PAPERRUST_PDFA_FONT_DIR").filter(|d| !d.trim().is_empty()) {
        v.push(PathBuf::from(d.trim()));
    }
    for d in [
        "/usr/share/fonts/liberation",
        "/usr/share/fonts/truetype/liberation",
        "/usr/share/fonts/truetype/liberation2",
        "/usr/share/fonts/liberation-sans",
        "/usr/share/fonts/liberation-serif",
        "/usr/share/fonts/liberation-mono",
        "/usr/share/fonts/TTF",
        "/usr/local/share/fonts/liberation",
        // FreeBSD x11-fonts/liberation-fonts-ttf (capital L)
        "/usr/local/share/fonts/Liberation",
    ] {
        v.push(PathBuf::from(d));
    }
    if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("fonts"))) {
        v.push(exe);
    }
    v
}

/// Loads the font file from the [`font_dirs`].
pub fn load_from_dirs(face: Face) -> Option<Vec<u8>> {
    let name = face.file_name();
    font_dirs().iter().map(|d| d.join(&name)).find(|p| p.is_file()).and_then(|p| std::fs::read(&p).ok())
}

fn name_of(o: &Object) -> Option<&[u8]> {
    o.as_name().ok()
}

fn dict_ref<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    match doc.dereference(o).ok()?.1 {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

fn num(doc: &Document, o: &Object) -> Option<f64> {
    match doc.dereference(o).ok()?.1 {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)),
        _ => None,
    }
}

/// Font name without subset tag (`ABCDEF+`).
fn base_name(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b).into_owned();
    match s.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|c| c.is_ascii_uppercase()) => rest.to_owned(),
        _ => s,
    }
}

/// Family and style from name and font descriptor; `None` for symbolic fonts.
pub fn choose_face(base: &str, flags: i64, italic_angle: f64, weight: Option<f64>) -> Option<Face> {
    let l = base.to_ascii_lowercase().replace([' ', '-', ',', '_'], "");
    if ["symbol", "dingbat", "wingding", "webding", "marlett"].iter().any(|s| l.contains(s)) {
        return None;
    }
    // Flags: bit 3 (4) symbolic, bit 6 (32) non-symbolic
    if flags & 4 != 0 && flags & 32 == 0 {
        return None;
    }
    let family = if l.contains("courier") || l.contains("mono") || l.contains("consol") || flags & 1 != 0 {
        Family::Mono
    } else if l.contains("times")
        || l.contains("serif") && !l.contains("sansserif")
        || l.contains("roman")
        || l.contains("georgia")
        || l.contains("garamond")
        || l.contains("book")
        || (flags & 2 != 0 && !l.contains("sans"))
    {
        Family::Serif
    } else {
        Family::Sans
    };
    let bold = ["bold", "black", "heavy", "semibold", "demi"].iter().any(|s| l.contains(s)) || weight.is_some_and(|w| w >= 600.0);
    let italic = l.contains("italic") || l.contains("oblique") || italic_angle.abs() > 0.5 || flags & 64 != 0;
    Some(Face { family, bold, italic })
}

fn agl_unicode(name: &str) -> Option<u32> {
    if let Ok(i) = enc::AGL.binary_search_by(|(n, _)| (*n).cmp(name)) {
        return Some(u32::from(enc::AGL[i].1));
    }
    name.strip_prefix("uni").filter(|h| h.len() == 4).and_then(|h| u32::from_str_radix(h, 16).ok())
}

/// AGL name to a Unicode character (for `Differences`), from the standard table only.
fn agl_name(u: u32) -> Option<&'static str> {
    enc::AGL.iter().find(|(_, c)| u32::from(*c) == u).map(|(n, _)| *n)
}

fn base_table(n: &[u8]) -> Option<&'static [&'static str; 256]> {
    match n {
        b"WinAnsiEncoding" => Some(&enc::WIN_ANSI),
        b"MacRomanEncoding" => Some(&enc::MAC_ROMAN),
        b"StandardEncoding" => Some(&enc::STANDARD),
        _ => None,
    }
}

/// Effective glyph names per code (base encoding + `Differences`). `None` if the encoding
/// cannot be mapped (unknown base, names outside the AGL table).
fn effective_names(doc: &Document, font: &Dictionary) -> Option<Vec<Option<String>>> {
    let mut names: Vec<Option<String>> = enc::STANDARD.iter().map(|n| (!n.is_empty()).then(|| (*n).to_owned())).collect();
    let set_base = |names: &mut Vec<Option<String>>, t: &[&str; 256]| {
        for (slot, n) in names.iter_mut().zip(t.iter()) {
            *slot = (!n.is_empty()).then(|| (*n).to_owned());
        }
    };
    match font.get(b"Encoding").ok().map(|o| doc.dereference(o).map(|(_, o)| o).unwrap_or(o)) {
        None => {}
        Some(Object::Name(n)) => set_base(&mut names, base_table(n)?),
        Some(Object::Dictionary(d)) => {
            if let Ok(b) = d.get(b"BaseEncoding") {
                set_base(&mut names, base_table(name_of(b)?)?);
            }
            if let Ok(diff) = d.get(b"Differences") {
                let arr = doc.dereference(diff).ok()?.1.as_array().ok()?;
                let mut code = 0usize;
                for o in arr {
                    match o {
                        Object::Integer(i) => code = usize::try_from(*i).ok()?,
                        Object::Name(n) => {
                            let s = String::from_utf8_lossy(n).into_owned();
                            if code < 256 {
                                // Unknown names (e.g. "g123") cannot be mapped.
                                let u = agl_unicode(&s)?;
                                names[code] = Some(agl_name(u).map(str::to_owned).unwrap_or(s));
                            }
                            code += 1;
                        }
                        _ => return None,
                    }
                }
            }
        }
        _ => return None,
    }
    Some(names)
}

/// Identifier for the subset (6 capital letters, derived from the name).
fn subset_tag(base: &str, face: Face) -> String {
    let mut h: u32 = 0x811C_9DC5;
    for b in base.bytes().chain(face.file_name().bytes()) {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    (0..6).map(|i| char::from(b'A' + ((h >> (i * 5)) % 26) as u8)).collect()
}

/// Replaces non-embedded simple fonts with embedded Liberation subsets.
/// `load`: returns the font file of a [`Face`] (default: [`load_from_dirs`]).
/// Returns: messages for the log (one per replaced font).
pub fn embed_missing_with(doc: &mut Document, load: &dyn Fn(Face) -> Option<Vec<u8>>) -> Vec<String> {
    let mut msgs = Vec::new();
    let candidates: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, o)| {
            let d = o.as_dict().ok()?;
            if !matches!(d.get(b"Type").ok().and_then(name_of), Some(b"Font")) {
                return None;
            }
            if !matches!(d.get(b"Subtype").ok().and_then(name_of), Some(b"Type1" | b"MMType1" | b"TrueType")) {
                return None;
            }
            let embedded = d
                .get(b"FontDescriptor")
                .ok()
                .and_then(|o| dict_ref(doc, o))
                .is_some_and(|fd| fd.has(b"FontFile") || fd.has(b"FontFile2") || fd.has(b"FontFile3"));
            (!embedded).then_some(*id)
        })
        .collect();
    if candidates.is_empty() {
        return msgs;
    }
    let mut cache: HashMap<Face, Option<Vec<u8>>> = HashMap::new();
    for id in candidates {
        match embed_one(doc, id, load, &mut cache) {
            Some(m) => msgs.push(m),
            None => tracing::debug!("PDF/A: no substitute font embedded for font object {id:?}"),
        }
    }
    msgs
}

/// [`embed_missing_with`] with the font folders.
pub fn embed_missing(doc: &mut Document) -> Vec<String> {
    embed_missing_with(doc, &load_from_dirs)
}

fn embed_one(
    doc: &mut Document,
    id: ObjectId,
    load: &dyn Fn(Face) -> Option<Vec<u8>>,
    cache: &mut HashMap<Face, Option<Vec<u8>>>,
) -> Option<String> {
    let font = doc.get_dictionary(id).ok()?.clone();
    let base = base_name(font.get(b"BaseFont").ok().and_then(name_of).unwrap_or(b"Unknown"));
    let fd = font.get(b"FontDescriptor").ok().and_then(|o| dict_ref(doc, o)).cloned();
    let flags = fd.as_ref().and_then(|d| d.get(b"Flags").ok()).and_then(|o| num(doc, o)).map_or(32, |f| f as i64);
    let angle = fd.as_ref().and_then(|d| d.get(b"ItalicAngle").ok()).and_then(|o| num(doc, o)).unwrap_or(0.0);
    let weight = fd.as_ref().and_then(|d| d.get(b"FontWeight").ok()).and_then(|o| num(doc, o));
    let face = choose_face(&base, flags, angle, weight)?;
    let names = effective_names(doc, &font)?;
    let data = cache.entry(face).or_insert_with(|| load(face)).as_ref()?;
    let ttf = Ttf::parse(data)?;
    let upem = f64::from(ttf.units_per_em);
    // Existing widths
    let first = font.get(b"FirstChar").ok().and_then(|o| num(doc, o)).map(|v| v as i64);
    let widths: Vec<f64> = font
        .get(b"Widths")
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_array().ok())
        .map(|a| a.iter().map(|w| num(doc, w).unwrap_or(0.0)).collect())
        .unwrap_or_default();
    let orig_width = |code: usize| -> Option<f64> {
        let f = first?;
        let i = usize::try_from(code as i64 - f).ok()?;
        widths.get(i).copied()
    };
    let mut keep = BTreeSet::new();
    let mut gid_width: BTreeMap<u16, u16> = BTreeMap::new();
    let mut new_widths: Vec<(usize, f64)> = Vec::new();
    for (code, n) in names.iter().enumerate() {
        let Some(n) = n else { continue };
        let Some(u) = agl_unicode(n) else { continue };
        let g = ttf.glyph(u);
        if g == 0 {
            continue;
        }
        keep.insert(g);
        let w = match orig_width(code) {
            Some(w) if w > 0.0 => {
                gid_width.entry(g).or_insert((w * upem / 1000.0).round().clamp(0.0, 65535.0) as u16);
                w
            }
            _ => (f64::from(ttf.advance(g)) * 1000.0 / upem).round(),
        };
        new_widths.push((code, w));
    }
    if keep.is_empty() {
        return None;
    }
    let sub = ttf.subset(&keep, &gid_width)?;
    let fc = new_widths.first().map_or(0, |(c, _)| *c);
    let lc = new_widths.last().map_or(0, |(c, _)| *c);
    let mut warr: Vec<Object> = (fc..=lc).map(|c| Object::Integer(orig_width(c).unwrap_or(0.0).round() as i64)).collect();
    for (c, w) in &new_widths {
        warr[c - fc] = Object::Integer(*w as i64);
    }
    // encoding: WinAnsi + differences with the same names
    let mut diffs: Vec<Object> = Vec::new();
    let mut last: Option<usize> = None;
    for (code, n) in names.iter().enumerate() {
        let Some(n) = n else { continue };
        if enc::WIN_ANSI[code] == n.as_str() {
            continue;
        }
        if last.map(|l| l + 1) != Some(code) {
            diffs.push(Object::Integer(code as i64));
        }
        diffs.push(Object::Name(n.as_bytes().to_vec()));
        last = Some(code);
    }
    let encoding = if diffs.is_empty() {
        Object::Name(b"WinAnsiEncoding".to_vec())
    } else {
        let mut e = Dictionary::new();
        e.set("Type", Object::Name(b"Encoding".to_vec()));
        e.set("BaseEncoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        e.set("Differences", diffs);
        Object::Dictionary(e)
    };
    let tag = subset_tag(&base, face);
    let font_name = format!("{tag}+{}", base.replace(' ', ""));
    let scale = |v: i16| Object::Integer((f64::from(v) * 1000.0 / upem).round() as i64);
    let len1 = sub.len() as i64;
    let mut ff = Stream::new(Dictionary::new(), sub);
    ff.dict.set("Length1", len1);
    let _ = ff.compress();
    let ff_id = doc.add_object(ff);
    let mut flags_new = 32i64;
    if face.italic {
        flags_new |= 64;
    }
    if face.family == Family::Serif {
        flags_new |= 2;
    }
    if face.family == Family::Mono {
        flags_new |= 1;
    }
    let mut desc = Dictionary::new();
    desc.set("Type", Object::Name(b"FontDescriptor".to_vec()));
    desc.set("FontName", Object::Name(font_name.as_bytes().to_vec()));
    desc.set("Flags", flags_new);
    desc.set("FontBBox", ttf.bbox.iter().map(|v| scale(*v)).collect::<Vec<_>>());
    desc.set("ItalicAngle", Object::Real(ttf.italic_angle as f32));
    desc.set("Ascent", scale(ttf.ascent));
    desc.set("Descent", scale(ttf.descent));
    desc.set("CapHeight", scale(ttf.cap_height));
    desc.set("StemV", if face.bold { 140 } else { 80 });
    desc.set("FontFile2", Object::Reference(ff_id));
    let desc_id = doc.add_object(desc);
    let f = doc.get_dictionary_mut(id).ok()?;
    f.set("Subtype", Object::Name(b"TrueType".to_vec()));
    f.set("BaseFont", Object::Name(font_name.as_bytes().to_vec()));
    f.set("FirstChar", fc as i64);
    f.set("LastChar", lc as i64);
    f.set("Widths", warr);
    f.set("Encoding", encoding);
    f.set("FontDescriptor", Object::Reference(desc_id));
    Some(format!("embedded substitute font {} for {base}", face.file_name()))
}

/// For tests/diagnostics: path of the substitute font, if present.
pub fn find_font_file(face: Face) -> Option<PathBuf> {
    let name = face.file_name();
    font_dirs().into_iter().map(|d| d.join(&name)).find(|p: &PathBuf| Path::is_file(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_from_names() {
        let f = |n: &str| choose_face(n, 32, 0.0, None);
        assert_eq!(f("Helvetica"), Some(Face { family: Family::Sans, bold: false, italic: false }));
        assert_eq!(f("ABCDEF+Arial,BoldItalic").map(|x| (x.bold, x.italic)), Some((true, true)));
        assert_eq!(f("Times-Roman").map(|x| x.family), Some(Family::Serif));
        assert_eq!(f("TimesNewRomanPS-BoldMT").map(|x| (x.family, x.bold)), Some((Family::Serif, true)));
        assert_eq!(f("Courier-Oblique").map(|x| (x.family, x.italic)), Some((Family::Mono, true)));
        assert_eq!(f("Verdana").map(|x| x.family), Some(Family::Sans));
        assert_eq!(f("Symbol"), None);
        assert_eq!(f("ZapfDingbats"), None);
        assert_eq!(choose_face("Foo", 4, 0.0, None), None);
        assert_eq!(choose_face("Foo", 32 | 64, 0.0, Some(700.0)).map(|x| (x.bold, x.italic)), Some((true, true)));
        assert_eq!(Face { family: Family::Serif, bold: true, italic: true }.file_name(), "LiberationSerif-BoldItalic.ttf");
        assert_eq!(base_name(b"ABCDEF+Helvetica"), "Helvetica");
        assert_eq!(base_name(b"Abc+X"), "Abc+X");
    }

    #[test]
    fn agl_tables() {
        assert_eq!(agl_unicode("A"), Some(0x41));
        assert_eq!(agl_unicode("quoteright"), Some(0x2019));
        assert_eq!(agl_unicode("uni20AC"), Some(0x20AC));
        assert_eq!(agl_unicode("g123"), None);
        assert_eq!(agl_name(0x20AC), Some("Euro"));
        assert_eq!(enc::STANDARD[0x27], "quoteright");
        assert_eq!(enc::WIN_ANSI[0x27], "quotesingle");
        assert_eq!(enc::WIN_ANSI[0x80], "Euro");
    }
}
