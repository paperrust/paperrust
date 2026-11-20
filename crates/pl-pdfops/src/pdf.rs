//! lopdf basics with pikepdf/qpdf semantics.
//!
//! upstream: pikepdf 10.13 / qpdf 12 (`Pdf.open`, `Pdf.new`, `pages.append/extend`, `del pages[i]`,
//! `Page.rotate(angle, relative=True)`, `remove_unreferenced_resources`, `save`).
//! Verified empirically with `reference/venv` (tests/parity/pl-pdfops).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId};

use crate::{PdfOpsError, Result};

/// Inheritable page attributes (ISO 32000-1, table 30).
pub const INHERITABLE: &[&[u8]] = &[b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

fn pdf_err(label: &str, e: impl std::fmt::Display) -> PdfOpsError {
    PdfOpsError::Pdf(format!("{label}: {e}"))
}

/// `pikepdf.open(path, password=...)`.
///
/// - Not readable → `Pdf` (`PdfError`).
/// - Encrypted and password missing/wrong → `Password` (`{path}: invalid password`).
/// - Encrypted with an empty user password → opened (like qpdf), `was_encrypted()`.
pub fn load(path: &Path, password: Option<&str>) -> Result<Document> {
    let data = std::fs::read(path)?;
    load_bytes(&data, &path.display().to_string(), password)
}

/// Like [`load`] for bytes; `label` appears in error texts (upstream: the path).
pub fn load_bytes(data: &[u8], label: &str, password: Option<&str>) -> Result<Document> {
    // qpdf: header `%PDF-` within the first 1024 bytes; offsets count from there.
    let data = match crate::repair::header_offset(data) {
        Some(off) if off > 0 => &data[off..],
        _ => data,
    };
    let password = password.filter(|p| !p.is_empty());
    let opts = LoadOptions { password: password.map(str::to_owned), ..Default::default() };
    match Document::load_mem_with_options(data, opts) {
        Ok(doc) => {
            // lopdf tries the empty password first; qpdf only checks the one given
            // (wrong password for a file without a user password → PasswordError).
            if let Some(pw) = password
                && doc.was_encrypted()
                && !password_matches(&doc, pw)
            {
                return Err(PdfOpsError::Password(format!("{label}: invalid password")));
            }
            if doc.is_encrypted() {
                // lopdf loads encrypted files without a matching password as empty.
                if let Ok(enc) = doc.get_encrypted()
                    && enc.get(b"Filter").and_then(Object::as_name).is_ok_and(|f| f != b"Standard")
                {
                    return Err(PdfOpsError::UnsupportedEncryption(format!(
                        "{label}: unsupported encryption filter (only the standard security handler is supported)"
                    )));
                }
                return Err(PdfOpsError::Password(format!("{label}: invalid password")));
            }
            if doc.catalog().is_err() {
                return Err(pdf_err(label, "unable to find trailer dictionary while recovering damaged file"));
            }
            Ok(doc)
        }
        Err(lopdf::Error::InvalidPassword) | Err(lopdf::Error::Decryption(lopdf::encryption::DecryptionError::IncorrectPassword)) => {
            Err(PdfOpsError::Password(format!("{label}: invalid password")))
        }
        Err(lopdf::Error::Decryption(
            e @ (lopdf::encryption::DecryptionError::UnsupportedEncryption
            | lopdf::encryption::DecryptionError::UnsupportedVersion
            | lopdf::encryption::DecryptionError::UnsupportedRevision),
        )) => Err(PdfOpsError::UnsupportedEncryption(format!("{label}: {e}"))),
        Err(e) => Err(pdf_err(label, e)),
    }
}

/// Does `pw` (as user or owner password) match the original `/Encrypt` of a
/// document loaded decrypted?
fn password_matches(doc: &Document, pw: &str) -> bool {
    let Some(state) = &doc.encryption_state else { return false };
    let Ok(enc) = state.encode() else { return false };
    let mut tmp = Document::new();
    let id = tmp.add_object(enc);
    tmp.trailer.set("Encrypt", Object::Reference(id));
    if let Ok(file_id) = doc.trailer.get(b"ID") {
        tmp.trailer.set("ID", file_id.clone());
    }
    tmp.authenticate_password(pw).is_ok()
}

/// Pages in document order.
pub fn page_ids(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

/// Page tree root (`/Root /Pages`).
pub fn pages_root(doc: &Document) -> Result<ObjectId> {
    doc.catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|e| PdfOpsError::Pdf(format!("page tree not found: {e}")))
}

/// Attribute of the page, otherwise from the nearest ancestor in the page tree.
pub fn inherited<'a>(doc: &'a Document, page: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut cur = doc.get_dictionary(page).ok()?;
    for _ in 0..64 {
        if let Ok(v) = cur.get(key) {
            return Some(v);
        }
        let parent = cur.get(b"Parent").and_then(Object::as_reference).ok()?;
        cur = doc.get_dictionary(parent).ok()?;
    }
    None
}

fn as_int(doc: &Document, o: &Object) -> Option<i64> {
    match doc.dereference(o).ok()?.1 {
        Object::Integer(i) => Some(*i),
        Object::Real(r) => Some(*r as i64),
        _ => None,
    }
}

/// Effective `/Rotate` (incl. inheritance), as qpdf reads it (not a multiple of 90 → 0).
pub fn effective_rotation(doc: &Document, page: ObjectId) -> i64 {
    let v = inherited(doc, page, b"Rotate").and_then(|o| as_int(doc, o)).unwrap_or(0);
    // qpdf `getIntValueAsInt`: clamped to the `int` range
    let v = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    if v % 90 != 0 { 0 } else { v }
}

/// `QPDFPageObjectHelper::rotate(angle, relative)` (pikepdf `Page.rotate`).
///
/// Empirical (pikepdf 10.13): `angle % 90 != 0` → `RuntimeError("QPDF::rotatePage called with an
/// angle that is not a multiple of 90")`; relative: old value (inherited, invalid → 0) + angle,
/// then `(new + 360) % 360` with C remainder (e.g. 0 − 450 → −90). The result is always directly in
/// the page.
///
/// Security review final W-N1: `angle` must fit into a C `int` (pikepdf/pybind11 rejects larger
/// values); computation is done in `i64`, i.e. without overflow (the old value is clamped to `int`).
pub fn rotate_page(doc: &mut Document, page: ObjectId, angle: i64, relative: bool) -> Result<()> {
    if i32::try_from(angle).is_err() {
        return Err(PdfOpsError::Runtime("rotation angle out of range".into()));
    }
    if angle % 90 != 0 {
        return Err(PdfOpsError::Runtime("QPDF::rotatePage called with an angle that is not a multiple of 90".into()));
    }
    let mut new = angle;
    if relative {
        new += effective_rotation(doc, page);
    }
    new = (new + 360) % 360;
    let d = doc.get_dictionary_mut(page).map_err(|e| PdfOpsError::Pdf(e.to_string()))?;
    d.set("Rotate", Object::Integer(new));
    Ok(())
}

/// Take inherited attributes into the page (qpdf `pushInheritedAttributesToPage`).
pub fn materialize_inherited(doc: &mut Document, page: ObjectId) -> Result<()> {
    let mut found: Vec<(&[u8], Object)> = Vec::new();
    {
        let d = doc.get_dictionary(page).map_err(|e| PdfOpsError::Pdf(e.to_string()))?;
        let mut parent = d.get(b"Parent").and_then(Object::as_reference).ok();
        let mut depth = 0;
        while let Some(pid) = parent {
            depth += 1;
            if depth > 64 {
                break;
            }
            let Ok(pd) = doc.get_dictionary(pid) else { break };
            for key in INHERITABLE {
                if d.get(key).is_err()
                    && !found.iter().any(|(k, _)| k == key)
                    && let Ok(v) = pd.get(key)
                {
                    found.push((key, v.clone()));
                }
            }
            parent = pd.get(b"Parent").and_then(Object::as_reference).ok();
        }
    }
    let d = doc.get_dictionary_mut(page).map_err(|e| PdfOpsError::Pdf(e.to_string()))?;
    for (k, v) in found {
        d.set(k.to_vec(), v);
    }
    Ok(())
}

/// Remove a page from the page tree (pikepdf `del pdf.pages[i]`): only from `/Kids` of the
/// parent node, adjust `/Count` of the ancestors. The page object itself stays as long as it is
/// still referenced (e.g. bookmarks), like qpdf.
pub fn remove_page_from_tree(doc: &mut Document, page: ObjectId) -> Result<()> {
    // Saving inherited attributes beforehand in case the page is still shown elsewhere is unnecessary:
    // qpdf leaves the object unchanged.
    let parent = doc
        .get_dictionary(page)
        .and_then(|d| d.get(b"Parent"))
        .and_then(Object::as_reference)
        .map_err(|e| PdfOpsError::Pdf(e.to_string()))?;
    {
        let pd = doc.get_dictionary_mut(parent).map_err(|e| PdfOpsError::Pdf(e.to_string()))?;
        if let Ok(Object::Array(kids)) = pd.get_mut(b"Kids")
            && let Some(pos) = kids.iter().position(|k| k.as_reference().ok() == Some(page))
        {
            kids.remove(pos);
        }
    }
    let mut cur = Some(parent);
    let mut depth = 0;
    while let Some(id) = cur {
        depth += 1;
        if depth > 64 {
            break;
        }
        let Ok(d) = doc.get_dictionary_mut(id) else { break };
        if let Ok(c) = d.get(b"Count").and_then(Object::as_i64) {
            d.set("Count", c - 1);
        }
        cur = d.get(b"Parent").and_then(Object::as_reference).ok();
    }
    Ok(())
}

/// Empty document like `pikepdf.new()`: PDF 1.3, only catalog and empty page tree, no info.
pub fn new_document() -> (Document, ObjectId) {
    let mut doc = Document::with_version("1.3");
    let pages_id = doc.new_object_id();
    let mut pages = Dictionary::new();
    pages.set("Count", 0);
    pages.set("Kids", Vec::<Object>::new());
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let mut cat = Dictionary::new();
    cat.set("Pages", Object::Reference(pages_id));
    cat.set("Type", Object::Name(b"Catalog".to_vec()));
    let cat_id = doc.add_object(cat);
    doc.trailer.set("Root", Object::Reference(cat_id));
    (doc, pages_id)
}

/// Copies pages from foreign documents into a target document (qpdf `copyForeignObject` +
/// `addPage`): one object cache per source (shared resources are copied once),
/// inherited attributes end up in the page, `/Parent` is not followed, page tree nodes
/// (`/Type /Pages`) are never copied (→ `null`). The same page twice → shallow copy
/// (qpdf: "duplicate page" → `shallowCopy`).
pub struct PageCopier {
    pub doc: Document,
    pages_id: ObjectId,
    caches: HashMap<usize, HashMap<ObjectId, ObjectId>>,
    in_tree: HashSet<ObjectId>,
}

impl PageCopier {
    pub fn new() -> Self {
        let (doc, pages_id) = new_document();
        PageCopier { doc, pages_id, caches: HashMap::new(), in_tree: HashSet::new() }
    }

    /// Number of pages in the target.
    pub fn page_count(&self) -> usize {
        self.in_tree_len()
    }

    fn in_tree_len(&self) -> usize {
        self.doc
            .get_dictionary(self.pages_id)
            .and_then(|d| d.get(b"Kids"))
            .and_then(Object::as_array)
            .map(Vec::len)
            .unwrap_or(0)
    }

    /// Append page `page` from `src` (source `source_key`, e.g. index of the opened file).
    /// Returns the target page ID.
    pub fn append(&mut self, source_key: usize, src: &Document, page: ObjectId) -> Result<ObjectId> {
        // Page with inherited attributes (without modifying the source).
        let mut page_dict = src.get_dictionary(page).map_err(|e| PdfOpsError::Pdf(e.to_string()))?.clone();
        for key in INHERITABLE {
            if page_dict.get(key).is_err()
                && let Some(v) = inherited(src, page, key)
            {
                page_dict.set(key.to_vec(), v.clone());
            }
        }
        page_dict.remove(b"Parent");
        let cache = self.caches.entry(source_key).or_default();
        let mut target = match cache.get(&page) {
            Some(id) => *id,
            None => {
                let id = self.doc.new_object_id();
                cache.insert(page, id);
                let copied = copy_dict(&mut self.doc, cache, src, &page_dict);
                self.doc.objects.insert(id, Object::Dictionary(copied));
                id
            }
        };
        if self.in_tree.contains(&target) {
            // Duplicate: shallow copy of the page dictionary.
            let d = self.doc.get_dictionary(target).map_err(|e| PdfOpsError::Pdf(e.to_string()))?.clone();
            target = self.doc.add_object(d);
        }
        {
            let d = self.doc.get_dictionary_mut(target).map_err(|e| PdfOpsError::Pdf(e.to_string()))?;
            d.set("Parent", Object::Reference(self.pages_id));
            if d.get(b"Type").is_err() {
                d.set("Type", Object::Name(b"Page".to_vec()));
            }
        }
        let pages = self.doc.get_dictionary_mut(self.pages_id).map_err(|e| PdfOpsError::Pdf(e.to_string()))?;
        if let Ok(Object::Array(kids)) = pages.get_mut(b"Kids") {
            kids.push(Object::Reference(target));
            let n = kids.len() as i64;
            pages.set("Count", n);
        }
        self.in_tree.insert(target);
        Ok(target)
    }

    pub fn finish(self) -> Document {
        self.doc
    }
}

impl Default for PageCopier {
    fn default() -> Self {
        Self::new()
    }
}

fn is_pages_node(src: &Document, id: ObjectId) -> bool {
    src.get_dictionary(id).ok().and_then(|d| d.get(b"Type").ok()).and_then(|t| t.as_name().ok()) == Some(b"Pages".as_slice())
}

fn copy_ref(dst: &mut Document, cache: &mut HashMap<ObjectId, ObjectId>, src: &Document, id: ObjectId) -> Object {
    if let Some(t) = cache.get(&id) {
        return Object::Reference(*t);
    }
    if is_pages_node(src, id) {
        return Object::Null;
    }
    let Ok(obj) = src.get_object(id) else { return Object::Null };
    let new_id = dst.new_object_id();
    cache.insert(id, new_id);
    // Placeholder so that cycles point to the new ID.
    dst.objects.insert(new_id, Object::Null);
    let copied = match obj {
        Object::Dictionary(d) => {
            let mut d = d.clone();
            if d.get(b"Type").and_then(Object::as_name).ok() == Some(b"Page".as_slice()) {
                // Another page (e.g. via /P of an annotation): do not follow /Parent.
                d.remove(b"Parent");
            }
            Object::Dictionary(copy_dict(dst, cache, src, &d))
        }
        other => copy_obj(dst, cache, src, other),
    };
    dst.objects.insert(new_id, copied);
    Object::Reference(new_id)
}

fn copy_dict(dst: &mut Document, cache: &mut HashMap<ObjectId, ObjectId>, src: &Document, d: &Dictionary) -> Dictionary {
    let mut out = Dictionary::new();
    for (k, v) in d.iter() {
        out.set(k.clone(), copy_obj(dst, cache, src, v));
    }
    out
}

fn copy_obj(dst: &mut Document, cache: &mut HashMap<ObjectId, ObjectId>, src: &Document, o: &Object) -> Object {
    match o {
        Object::Reference(id) => copy_ref(dst, cache, src, *id),
        Object::Array(a) => Object::Array(a.iter().map(|x| copy_obj(dst, cache, src, x)).collect()),
        Object::Dictionary(d) => Object::Dictionary(copy_dict(dst, cache, src, d)),
        Object::Stream(s) => {
            let mut s2 = s.clone();
            s2.dict = copy_dict(dst, cache, src, &s.dict);
            Object::Stream(s2)
        }
        other => other.clone(),
    }
}

// ----------------------------------------------------------------------------------------------
// remove_unreferenced_resources
// ----------------------------------------------------------------------------------------------

const RESOURCE_TYPES: &[&[u8]] = &[b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"XObject", b"Font", b"Properties"];

/// All name operands of a content stream; `None` if it cannot be decomposed (then
/// qpdf leaves the resources alone).
fn content_names(content: &[u8]) -> Option<BTreeSet<Vec<u8>>> {
    let c = lopdf::content::Content::decode(content).ok()?;
    let mut names = BTreeSet::new();
    fn walk(o: &Object, names: &mut BTreeSet<Vec<u8>>) {
        match o {
            Object::Name(n) => {
                names.insert(n.clone());
            }
            Object::Array(a) => a.iter().for_each(|x| walk(x, names)),
            Object::Dictionary(d) => d.iter().for_each(|(_, v)| walk(v, names)),
            _ => {}
        }
    }
    for op in &c.operations {
        for operand in &op.operands {
            walk(operand, &mut names);
        }
    }
    Some(names)
}

fn stream_content(doc: &Document, id: ObjectId) -> Option<Vec<u8>> {
    let s = doc.get_object(id).ok()?.as_stream().ok()?;
    if s.dict.get(b"Filter").is_ok() { s.decompressed_content().ok() } else { Some(s.content.clone()) }
}

/// Make the resource dictionary of a page/form direct (not shared) and
/// remove unused entries. `owner` = object that carries `/Resources`.
fn prune_resources(doc: &mut Document, owner: ObjectId, names: &BTreeSet<Vec<u8>>, visited: &mut HashSet<ObjectId>) {
    let res = {
        let Ok(o) = doc.get_object(owner) else { return };
        let dict = match o {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => return,
        };
        let Ok(r) = dict.get(b"Resources") else { return };
        match doc.dereference(r) {
            Ok((_, Object::Dictionary(d))) => d.clone(),
            _ => return,
        }
    };
    let mut new_res = Dictionary::new();
    let mut forms: Vec<ObjectId> = Vec::new();
    for (k, v) in res.iter() {
        if !RESOURCE_TYPES.contains(&k.as_slice()) {
            new_res.set(k.clone(), v.clone());
            continue;
        }
        let sub = match doc.dereference(v) {
            Ok((_, Object::Dictionary(d))) => d.clone(),
            _ => {
                new_res.set(k.clone(), v.clone());
                continue;
            }
        };
        let mut kept = Dictionary::new();
        for (name, val) in sub.iter() {
            if names.contains(name) {
                kept.set(name.clone(), val.clone());
                if k.as_slice() == b"XObject"
                    && let Object::Reference(id) = val
                {
                    forms.push(*id);
                }
            }
        }
        new_res.set(k.clone(), Object::Dictionary(kept));
    }
    if let Ok(o) = doc.get_object_mut(owner) {
        match o {
            Object::Dictionary(d) => d.set("Resources", Object::Dictionary(new_res)),
            Object::Stream(s) => s.dict.set("Resources", Object::Dictionary(new_res)),
            _ => {}
        }
    }
    // Form XObjects with their own resources recursively.
    for f in forms {
        if !visited.insert(f) {
            continue;
        }
        let is_form = doc
            .get_object(f)
            .ok()
            .and_then(|o| o.as_stream().ok())
            .is_some_and(|s| s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Form".as_slice()) && s.dict.get(b"Resources").is_ok());
        if !is_form {
            continue;
        }
        if let Some(names) = stream_content(doc, f).and_then(|c| content_names(&c)) {
            prune_resources(doc, f, &names, visited);
        }
    }
}

/// `Pdf.remove_unreferenced_resources()` (qpdf `removeUnreferencedResources`): per page remove the
/// resources that do not occur in the content stream (form XObjects recursively).
/// Conservative: every name operand counts as a reference; content that cannot be decomposed → unchanged.
pub fn remove_unreferenced_resources(doc: &mut Document) {
    let pages = page_ids(doc);
    let mut visited = HashSet::new();
    for page in pages {
        // bring inherited resources into the page first (otherwise the parent node would be modified)
        if let Some(r) = inherited(doc, page, b"Resources").cloned()
            && let Ok(d) = doc.get_dictionary_mut(page)
            && d.get(b"Resources").is_err()
        {
            d.set("Resources", r);
        }
        let content = doc.get_page_content(page);
        let Some(names) = content_names(&content) else { continue };
        // Annotations with an appearance have their own resources; those stay untouched.
        prune_resources(doc, page, &names, &mut visited);
    }
}

/// Saving like `Pdf.save(dst)` (without encryption): reachable objects only;
/// from PDF 1.5 with object/cross-reference streams (smaller files, like qpdf "preserve").
pub fn save(doc: &mut Document, dst: &Path) -> Result<()> {
    let bytes = save_to_vec(doc)?;
    std::fs::write(dst, bytes)?;
    Ok(())
}

pub fn save_to_vec(doc: &mut Document) -> Result<Vec<u8>> {
    doc.prune_objects();
    let mut buf = Vec::new();
    let modern = version_tuple(&doc.version) >= (1, 5);
    if !modern {
        // otherwise lopdf raises the version to 1.5 for the cross-reference stream
        doc.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceTable;
    }
    let r = if modern { doc.save_modern(&mut buf) } else { doc.save_to(&mut buf) };
    r.map_err(|e| PdfOpsError::Pdf(format!("unable to write PDF: {e}")))?;
    Ok(buf)
}

/// "1.7" → (1, 7); unreadable → (1, 3).
pub fn version_tuple(v: &str) -> (u32, u32) {
    let mut it = v.trim().split('.');
    let a = it.next().and_then(|x| x.parse().ok()).unwrap_or(1);
    let b = it.next().and_then(|x| x.parse().ok()).unwrap_or(3);
    (a, b)
}

#[cfg(test)]
pub(crate) mod testpdf {
    //! Small test PDFs (one text line per page).
    use super::*;
    use lopdf::{Stream, dictionary};

    pub fn make(labels: &[&str]) -> Document {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier"});
        let unused_id = doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica"});
        let resources_id = doc.add_object(dictionary! {"Font" => dictionary! {"F1" => font_id, "F9" => unused_id}});
        let mut kids = Vec::new();
        for l in labels {
            let content = format!("BT /F1 24 Tf 72 700 Td ({l}) Tj ET");
            let cid = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
            let pid = doc.add_object(dictionary! {"Type" => "Page", "Parent" => pages_id, "Contents" => cid});
            kids.push(Object::Reference(pid));
        }
        let n = kids.len() as i64;
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => n,
                "Resources" => resources_id, "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()]}),
        );
        let cat = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", cat);
        let info = doc.add_object(dictionary! {"Title" => Object::string_literal("Testtitel")});
        doc.trailer.set("Info", info);
        doc
    }

    pub fn write(path: &Path, labels: &[&str]) {
        let mut d = make(labels);
        d.save(path).unwrap();
    }

    pub fn texts(path: &Path) -> Vec<String> {
        let d = Document::load(path).unwrap();
        d.get_pages().keys().map(|n| d.extract_text(&[*n]).unwrap_or_default().trim().to_string()).collect()
    }

    pub fn rotations(path: &Path) -> Vec<i64> {
        let d = Document::load(path).unwrap();
        page_ids(&d).into_iter().map(|p| inherited(&d, p, b"Rotate").and_then(|o| o.as_i64().ok()).unwrap_or(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_semantics_like_qpdf() {
        let mut d = testpdf::make(&["A"]);
        let p = page_ids(&d)[0];
        let cases = [(90, 0), (-90, 180), (450, 0), (-450, 180), (180, 90), (0, 270), (360, 270)];
        for (a, want) in cases {
            d.get_dictionary_mut(p).unwrap().set("Rotate", 270);
            rotate_page(&mut d, p, a, true).unwrap();
            assert_eq!(d.get_dictionary(p).unwrap().get(b"Rotate").unwrap().as_i64().unwrap(), want, "angle {a}");
        }
        let e = rotate_page(&mut d, p, 45, true).unwrap_err();
        assert_eq!(e.to_string(), "QPDF::rotatePage called with an angle that is not a multiple of 90");
        d.get_dictionary_mut(p).unwrap().set("Rotate", 45);
        rotate_page(&mut d, p, 90, true).unwrap();
        assert_eq!(d.get_dictionary(p).unwrap().get(b"Rotate").unwrap().as_i64().unwrap(), 90);
        // inherited
        d.get_dictionary_mut(p).unwrap().remove(b"Rotate");
        let root = pages_root(&d).unwrap();
        d.get_dictionary_mut(root).unwrap().set("Rotate", 90);
        rotate_page(&mut d, p, 90, true).unwrap();
        assert_eq!(d.get_dictionary(p).unwrap().get(b"Rotate").unwrap().as_i64().unwrap(), 180);
        // 0 - 450 → -90 (C-Rest)
        d.get_dictionary_mut(p).unwrap().set("Rotate", 0);
        rotate_page(&mut d, p, -450, true).unwrap();
        assert_eq!(d.get_dictionary(p).unwrap().get(b"Rotate").unwrap().as_i64().unwrap(), -90);
    }

    /// W-N1: no overflow with extreme angles or `/Rotate` values in the PDF.
    #[test]
    fn rotate_extreme_values_no_overflow() {
        let mut d = testpdf::make(&["A"]);
        let p = page_ids(&d)[0];
        for a in [i64::MAX - 7, i64::MIN, 90 * (1 << 40), -(90 * (1 << 40))] {
            assert!(rotate_page(&mut d, p, a, true).is_err(), "angle {a}");
        }
        // largest multiple of 90 in the int range plus a large old value
        let big = i64::from(i32::MAX) / 90 * 90;
        d.get_dictionary_mut(p).unwrap().set("Rotate", i64::MAX - 7);
        rotate_page(&mut d, p, big, true).unwrap();
        d.get_dictionary_mut(p).unwrap().set("Rotate", big);
        rotate_page(&mut d, p, big, true).unwrap();
        let r = d.get_dictionary(p).unwrap().get(b"Rotate").unwrap().as_i64().unwrap();
        assert!((-359..360).contains(&r) && r % 90 == 0, "{r}");
        rotate_page(&mut d, p, -big, false).unwrap();
    }

    #[test]
    fn copier_pushes_inherited_and_shallow_copies_duplicates() {
        let src = testpdf::make(&["A", "B"]);
        let ps = page_ids(&src);
        let mut c = PageCopier::new();
        let a1 = c.append(0, &src, ps[0]).unwrap();
        let a2 = c.append(0, &src, ps[0]).unwrap();
        assert_ne!(a1, a2);
        c.append(0, &src, ps[1]).unwrap();
        let mut doc = c.finish();
        assert_eq!(doc.version, "1.3");
        assert!(doc.trailer.get(b"Info").is_err());
        let d = doc.get_dictionary(a1).unwrap();
        assert!(d.get(b"MediaBox").is_ok() && d.get(b"Resources").is_ok());
        let t = tempfile::tempdir().unwrap();
        let out = t.path().join("o.pdf");
        save(&mut doc, &out).unwrap();
        assert_eq!(testpdf::texts(&out), ["A", "A", "B"]);
    }

    #[test]
    fn unreferenced_resources_removed() {
        let mut d = testpdf::make(&["A"]);
        remove_unreferenced_resources(&mut d);
        let p = page_ids(&d)[0];
        let res = d.get_dictionary(p).unwrap().get(b"Resources").unwrap().as_dict().unwrap();
        let fonts = res.get(b"Font").unwrap().as_dict().unwrap();
        assert!(fonts.get(b"F1").is_ok());
        assert!(fonts.get(b"F9").is_err());
    }

    #[test]
    fn remove_page_keeps_tree_consistent() {
        let mut d = testpdf::make(&["A", "B", "C"]);
        let ps = page_ids(&d);
        remove_page_from_tree(&mut d, ps[1]).unwrap();
        assert_eq!(page_ids(&d), vec![ps[0], ps[2]]);
        let root = pages_root(&d).unwrap();
        assert_eq!(d.get_dictionary(root).unwrap().get(b"Count").unwrap().as_i64().unwrap(), 2);
    }

    #[test]
    fn load_errors() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("x.pdf");
        std::fs::write(&p, b"not a pdf at all").unwrap();
        assert_eq!(load(&p, None).unwrap_err().kind(), "PdfError");
    }
}
