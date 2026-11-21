// upstream: src/documents/double_sided.py@v3.4.2 (CollatePlugin.run: pikepdf part)
//
// `pdf2.pages.reverse(); for i, page in enumerate(pdf2.pages): pdf1.pages.insert(2*i+1, page)` with
// lopdf. The page trees of both files are flattened (inheritable attributes Resources,
// MediaBox, CropBox, Rotate are copied into the pages).

use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::stages::StageError;

/// Text from upstream when the second file has more pages.
pub const TOO_MANY_PAGES: &str = "This second file (even numbered pages) contains more pages than the first/odd numbered one. This means the two uploaded files don't belong to the same double-sided scan. Please retry, starting with the odd numbered pages again.";

const INHERITABLE: &[&[u8]] = &[b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

fn err(e: impl std::fmt::Display) -> StageError {
    StageError::Other(e.to_string())
}

/// Copy inherited attributes into the page.
fn materialize_inherited(doc: &mut Document, page: ObjectId) -> Result<(), StageError> {
    let mut inherited: Vec<(Vec<u8>, Object)> = Vec::new();
    let mut parent = doc.get_dictionary(page).map_err(err)?.get(b"Parent").and_then(Object::as_reference).ok();
    let mut depth = 0;
    while let Some(pid) = parent {
        depth += 1;
        if depth > 64 {
            break;
        }
        let Ok(pd) = doc.get_dictionary(pid) else { break };
        for key in INHERITABLE {
            if let Ok(v) = pd.get(key)
                && !inherited.iter().any(|(k, _)| k.as_slice() == *key)
            {
                inherited.push((key.to_vec(), v.clone()));
            }
        }
        parent = pd.get(b"Parent").and_then(Object::as_reference).ok();
    }
    let pd = doc.get_dictionary_mut(page).map_err(err)?;
    for (k, v) in inherited {
        if pd.get(&k).is_err() {
            pd.set(k, v);
        }
    }
    Ok(())
}

fn pages_root(doc: &Document) -> Result<ObjectId, StageError> {
    doc.catalog().map_err(err)?.get(b"Pages").and_then(Object::as_reference).map_err(err)
}

/// `collate`: result to `out`.
pub fn collate_pdfs(odd: &Path, even: &Path, out: &Path) -> Result<(), StageError> {
    let mut d1 = Document::load(odd).map_err(err)?;
    let mut d2 = Document::load(even).map_err(err)?;
    let p1: Vec<ObjectId> = d1.get_pages().values().copied().collect();
    for p in &p1 {
        materialize_inherited(&mut d1, *p)?;
    }
    d2.renumber_objects_with(d1.max_id + 1);
    let mut p2: Vec<ObjectId> = d2.get_pages().values().copied().collect();
    for p in &p2 {
        materialize_inherited(&mut d2, *p)?;
    }
    p2.reverse();
    // pikepdf: insert(2*i+1) – IndexError as soon as i > len(pdf1) - 1
    let mut order = p1.clone();
    for (i, page) in p2.iter().enumerate() {
        let idx = 2 * i + 1;
        if idx > order.len() {
            return Err(StageError::Other(TOO_MANY_PAGES.into()));
        }
        order.insert(idx, *page);
    }
    let root2 = pages_root(&d2).ok();
    let catalog2 = d2.trailer.get(b"Root").and_then(Object::as_reference).ok();
    for (id, obj) in std::mem::take(&mut d2.objects) {
        if Some(id) == root2 || Some(id) == catalog2 {
            continue;
        }
        d1.objects.insert(id, obj);
    }
    d1.max_id = d1.max_id.max(d2.max_id);
    let root = pages_root(&d1)?;
    for p in &order {
        if let Ok(pd) = d1.get_dictionary_mut(*p) {
            pd.set("Parent", Object::Reference(root));
        }
    }
    let kids: Vec<Object> = order.iter().map(|p| Object::Reference(*p)).collect();
    let count = kids.len() as i64;
    let rd: &mut Dictionary = d1.get_dictionary_mut(root).map_err(err)?;
    rd.set("Kids", Object::Array(kids));
    rd.set("Count", Object::Integer(count));
    for key in INHERITABLE {
        rd.remove(key);
    }
    d1.prune_objects();
    d1.save(out).map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};

    fn make(path: &Path, labels: &[&str]) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier"});
        let resources_id = doc.add_object(dictionary! {"Font" => dictionary! {"F1" => font_id}});
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
        doc.save(path).unwrap();
    }

    #[test]
    fn collates_reverse_interleaved() {
        let t = tempfile::tempdir().unwrap();
        let (a, b, o) = (t.path().join("a.pdf"), t.path().join("b.pdf"), t.path().join("o.pdf"));
        make(&a, &["P1", "P3", "P5"]);
        make(&b, &["P6", "P4", "P2"]);
        collate_pdfs(&a, &b, &o).unwrap();
        let d = Document::load(&o).unwrap();
        let mut texts = Vec::new();
        for n in d.get_pages().keys() {
            texts.push(d.extract_text(&[*n]).unwrap_or_default().trim().to_string());
        }
        assert_eq!(texts, ["P1", "P2", "P3", "P4", "P5", "P6"]);
    }

    #[test]
    fn too_many_even_pages() {
        let t = tempfile::tempdir().unwrap();
        let (a, b, o) = (t.path().join("a.pdf"), t.path().join("b.pdf"), t.path().join("o.pdf"));
        make(&a, &["P1"]);
        make(&b, &["P2", "P3"]);
        assert_eq!(collate_pdfs(&a, &b, &o).unwrap_err(), StageError::Other(TOO_MANY_PAGES.into()));
    }
}
