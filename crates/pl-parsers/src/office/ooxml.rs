//! OOXML (DOCX, XLSX, PPTX/PPSX): ZIP + XML, streamed with limits.

use crate::limits::TextSink;
use crate::xmlutil::{XEvent, XmlStream, attr};
use crate::zipread::ZipArchive;
use crate::{Deadline, Limits, ParseError, Result};

use super::{OfficeContent, push_line};

const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_S: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const NS_P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const NS_CP: &str = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
const NS_DCTERMS: &str = "http://purl.org/dc/terms/";
const NS_EP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties";
const REL_NOTES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide";

pub fn extract(data: &[u8], mime_type: &str, limits: &Limits, deadline: Deadline) -> Result<OfficeContent> {
    let zip = ZipArchive::open(data, limits)?;
    let mut out = TextSink::new(limits.max_text_chars);
    match mime_type {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => docx_text(&zip, limits, deadline, &mut out)?,
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => xlsx_text(&zip, limits, deadline, &mut out)?,
        _ => pptx_text(&zip, limits, deadline, &mut out)?,
    }
    let (metadata, created) = core_metadata(&zip, mime_type, limits)?;
    Ok(OfficeContent { text: out.buf, metadata, created })
}

/// Resolve a path relative to a part (`xl/workbook.xml` + `worksheets/s1.xml`).
fn resolve(base: &str, target: &str) -> String {
    if let Some(t) = target.strip_prefix('/') {
        return t.to_owned();
    }
    let mut parts: Vec<&str> = base.split('/').collect();
    parts.pop();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn rels_path(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// (Id, Type, Target) of a `.rels` file.
fn read_rels(zip: &ZipArchive, part: &str, limits: &Limits) -> Result<Vec<(String, String, String)>> {
    let Some(data) = zip.read_opt(&rels_path(part))? else {
        return Ok(Vec::new());
    };
    let mut s = XmlStream::new(&data, limits.max_depth);
    let mut out = Vec::new();
    while let Some(ev) = s.next()? {
        if let XEvent::Start { ns, local, attrs } = &ev
            && ns.as_deref() == Some(NS_REL)
            && local == "Relationship"
        {
            out.push((
                attr(attrs, None, "Id").unwrap_or("").to_owned(),
                attr(attrs, None, "Type").unwrap_or("").to_owned(),
                attr(attrs, None, "Target").unwrap_or("").to_owned(),
            ));
        }
    }
    Ok(out)
}

fn main_part(zip: &ZipArchive, limits: &Limits, default: &str) -> Result<String> {
    for (_, ty, target) in read_rels(zip, "", limits).unwrap_or_default() {
        if ty.ends_with("/officeDocument") {
            return Ok(resolve("", &target));
        }
    }
    let _ = limits;
    if zip.has(default) { Ok(default.to_owned()) } else { Err(ParseError::Invalid(format!("OOXML main part {default} missing"))) }
}

// ---------------------------------------------------------------------------
// DOCX
// ---------------------------------------------------------------------------

struct WordState {
    paras: Vec<String>,
    /// open tables: rows → cells
    tables: Vec<Vec<Vec<String>>>,
    cells: Vec<String>,
    in_t: bool,
    skip: usize,
}

fn word_part_text(xml: &[u8], limits: &Limits, deadline: Deadline, out: &mut TextSink) -> Result<()> {
    let mut s = XmlStream::new(xml, limits.max_depth).with_deadline(deadline);
    let mut st = WordState { paras: Vec::new(), tables: Vec::new(), cells: Vec::new(), in_t: false, skip: 0 };
    let emit = |st: &mut WordState, out: &mut TextSink, line: String| -> Result<()> {
        if let Some(cell) = st.cells.last_mut() {
            if !cell.is_empty() {
                cell.push(' ');
            }
            cell.push_str(&line);
            Ok(())
        } else if let Some(p) = st.paras.last_mut() {
            p.push_str(&line);
            Ok(())
        } else {
            push_line(out, &line)
        }
    };
    while let Some(ev) = s.next()? {
        match &ev {
            XEvent::Start { ns, local, .. } if ns.as_deref() == Some(NS_W) => {
                if st.skip > 0 {
                    if matches!(local.as_str(), "del" | "instrText" | "delText" | "moveFrom") {
                        st.skip += 1;
                    }
                    continue;
                }
                match local.as_str() {
                    "p" => st.paras.push(String::new()),
                    "t" => st.in_t = true,
                    "tab" | "ptab" => {
                        if let Some(p) = st.paras.last_mut() {
                            p.push('\t');
                        }
                    }
                    "br" | "cr" => {
                        if let Some(p) = st.paras.last_mut() {
                            p.push('\n');
                        }
                    }
                    "noBreakHyphen" => {
                        if let Some(p) = st.paras.last_mut() {
                            p.push('-');
                        }
                    }
                    "del" | "instrText" | "delText" | "moveFrom" => st.skip = 1,
                    "tbl" => st.tables.push(Vec::new()),
                    "tr" => {
                        if let Some(t) = st.tables.last_mut() {
                            t.push(Vec::new());
                        }
                    }
                    "tc" => st.cells.push(String::new()),
                    _ => {}
                }
            }
            XEvent::End { ns, local } if ns.as_deref() == Some(NS_W) => {
                if st.skip > 0 {
                    if matches!(local.as_str(), "del" | "instrText" | "delText" | "moveFrom") {
                        st.skip -= 1;
                    }
                    continue;
                }
                match local.as_str() {
                    "t" => st.in_t = false,
                    "p" => {
                        let line = st.paras.pop().unwrap_or_default();
                        emit(&mut st, out, line)?;
                    }
                    "tc" => {
                        let cell = st.cells.pop().unwrap_or_default();
                        if let Some(row) = st.tables.last_mut().and_then(|t| t.last_mut()) {
                            row.push(cell);
                        }
                    }
                    "tr" => {
                        let row = st.tables.last_mut().and_then(|t| t.pop()).unwrap_or_default();
                        let line = row.join("\t");
                        emit(&mut st, out, line)?;
                    }
                    "tbl" => {
                        st.tables.pop();
                    }
                    _ => {}
                }
            }
            XEvent::Text(t) if st.in_t && st.skip == 0 => {
                if let Some(p) = st.paras.last_mut() {
                    if p.len() + t.len() > limits.max_text_chars {
                        return Err(ParseError::LimitExceeded("paragraph too long".into()));
                    }
                    p.push_str(t);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn docx_text(zip: &ZipArchive, limits: &Limits, deadline: Deadline, out: &mut TextSink) -> Result<()> {
    let main = main_part(zip, limits, "word/document.xml")?;
    let rels = read_rels(zip, &main, limits)?;
    let by_type = |suffix: &str| -> Vec<String> {
        rels.iter().filter(|(_, t, _)| t.ends_with(suffix)).map(|(_, _, target)| resolve(&main, target)).collect()
    };
    for h in by_type("/header") {
        if let Some(x) = zip.read_opt(&h)? {
            word_part_text(&x, limits, deadline, out)?;
        }
    }
    let body = zip.read(&main)?;
    word_part_text(&body, limits, deadline, out)?;
    for kind in ["/footnotes", "/endnotes", "/footer"] {
        for h in by_type(kind) {
            if let Some(x) = zip.read_opt(&h)? {
                word_part_text(&x, limits, deadline, out)?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// XLSX
// ---------------------------------------------------------------------------

fn shared_strings(xml: &[u8], limits: &Limits, deadline: Deadline) -> Result<Vec<String>> {
    let mut s = XmlStream::new(xml, limits.max_depth).with_deadline(deadline);
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut in_t = false;
    let mut in_rph = 0usize;
    let mut total = 0usize;
    while let Some(ev) = s.next()? {
        match &ev {
            XEvent::Start { ns, local, .. } if ns.as_deref() == Some(NS_S) => match local.as_str() {
                "si" => cur = Some(String::new()),
                "t" => in_t = true,
                "rPh" => in_rph += 1,
                _ => {}
            },
            XEvent::End { ns, local } if ns.as_deref() == Some(NS_S) => match local.as_str() {
                "si" => out.push(cur.take().unwrap_or_default()),
                "t" => in_t = false,
                "rPh" => in_rph = in_rph.saturating_sub(1),
                _ => {}
            },
            XEvent::Text(t) if in_t && in_rph == 0 => {
                total += t.len();
                if total > limits.max_text_chars {
                    return Err(ParseError::LimitExceeded("shared strings too large".into()));
                }
                if let Some(c) = cur.as_mut() {
                    c.push_str(t);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Cell formats from `styles.xml`: format number per style index (`cellXfs`) and custom formats.
#[derive(Debug, Default, Clone)]
pub(crate) struct CellStyles {
    pub xf_numfmt: Vec<u32>,
    pub custom: Vec<(u32, String)>,
    pub date1904: bool,
}

impl CellStyles {
    /// Displayed text of a number with style index `s` (like POI `DataFormatter`).
    pub fn display(&self, v: f64, s: Option<usize>) -> String {
        let id = s.and_then(|i| self.xf_numfmt.get(i)).copied().unwrap_or(0);
        super::numfmt::format_cell(v, id, &self.custom, self.date1904)
    }
}

fn cell_styles(xml: &[u8], limits: &Limits, deadline: Deadline) -> Result<CellStyles> {
    let mut s = XmlStream::new(xml, limits.max_depth).with_deadline(deadline);
    let mut st = CellStyles::default();
    let mut in_xfs = false;
    while let Some(ev) = s.next()? {
        match &ev {
            XEvent::Start { ns, local, attrs } if ns.as_deref() == Some(NS_S) => match local.as_str() {
                "numFmt" => {
                    if let (Some(id), Some(code)) =
                        (attr(attrs, None, "numFmtId").and_then(|v| v.trim().parse().ok()), attr(attrs, None, "formatCode"))
                    {
                        st.custom.push((id, code.to_owned()));
                    }
                }
                "cellXfs" => in_xfs = true,
                "xf" if in_xfs => {
                    st.xf_numfmt.push(attr(attrs, None, "numFmtId").and_then(|v| v.trim().parse().ok()).unwrap_or(0));
                }
                _ => {}
            },
            XEvent::End { ns, local } if ns.as_deref() == Some(NS_S) && local == "cellXfs" => in_xfs = false,
            _ => {}
        }
    }
    Ok(st)
}

fn sheet_text(xml: &[u8], sst: &[String], styles: &CellStyles, limits: &Limits, deadline: Deadline, out: &mut TextSink) -> Result<()> {
    let mut s = XmlStream::new(xml, limits.max_depth).with_deadline(deadline);
    let mut row: Vec<String> = Vec::new();
    let mut in_row = false;
    let mut cell_type = String::new();
    let mut cell_style: Option<usize> = None;
    let mut val: Option<String> = None;
    let mut in_v = false;
    let mut in_is_t = false;
    let mut inline = String::new();
    while let Some(ev) = s.next()? {
        match &ev {
            XEvent::Start { ns, local, attrs } if ns.as_deref() == Some(NS_S) => match local.as_str() {
                "row" => {
                    in_row = true;
                    row.clear();
                }
                "c" => {
                    cell_type = attr(attrs, None, "t").unwrap_or("n").to_owned();
                    cell_style = attr(attrs, None, "s").and_then(|v| v.trim().parse().ok());
                    val = None;
                    inline.clear();
                }
                "v" => {
                    in_v = true;
                    val = Some(String::new());
                }
                "t" => in_is_t = true,
                _ => {}
            },
            XEvent::End { ns, local } if ns.as_deref() == Some(NS_S) => match local.as_str() {
                "v" => in_v = false,
                "t" => in_is_t = false,
                "c" => {
                    let text = match cell_type.as_str() {
                        "s" => val.as_deref().and_then(|v| v.trim().parse::<usize>().ok()).and_then(|i| sst.get(i).cloned()),
                        "inlineStr" => Some(inline.clone()),
                        "b" => val.as_deref().map(|v| if v.trim() == "1" { "TRUE".into() } else { "FALSE".into() }),
                        // Number: displayed text according to cell format (date, percent, …) like Tika/POI
                        "n" => val.as_deref().map(|v| match v.trim().parse::<f64>() {
                            Ok(x) => styles.display(x, cell_style),
                            Err(_) => v.to_owned(),
                        }),
                        _ => val.clone(),
                    };
                    if let Some(t) = text
                        && !t.is_empty()
                    {
                        row.push(t);
                    }
                }
                "row" => {
                    in_row = false;
                    if !row.is_empty() {
                        out.push('\t')?;
                        push_line(out, &row.join("\t"))?;
                    }
                }
                _ => {}
            },
            XEvent::Text(t) => {
                if in_v {
                    if let Some(v) = val.as_mut() {
                        v.push_str(t);
                    }
                } else if in_is_t && in_row {
                    inline.push_str(t);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn xlsx_text(zip: &ZipArchive, limits: &Limits, deadline: Deadline, out: &mut TextSink) -> Result<()> {
    let wb = main_part(zip, limits, "xl/workbook.xml")?;
    let rels = read_rels(zip, &wb, limits)?;
    let sst = match rels.iter().find(|(_, t, _)| t.ends_with("/sharedStrings")) {
        Some((_, _, target)) => match zip.read_opt(&resolve(&wb, target))? {
            Some(x) => shared_strings(&x, limits, deadline)?,
            None => Vec::new(),
        },
        None => Vec::new(),
    };
    let mut styles = match rels.iter().find(|(_, t, _)| t.ends_with("/styles")) {
        Some((_, _, target)) => match zip.read_opt(&resolve(&wb, target))? {
            Some(x) => cell_styles(&x, limits, deadline)?,
            None => CellStyles::default(),
        },
        None => CellStyles::default(),
    };
    let wbx = zip.read(&wb)?;
    let mut s = XmlStream::new(&wbx, limits.max_depth);
    let mut sheets: Vec<(String, String)> = Vec::new();
    while let Some(ev) = s.next()? {
        if let XEvent::Start { ns, local, attrs } = &ev
            && ns.as_deref() == Some(NS_S)
        {
            if local == "sheet" {
                sheets.push((attr(attrs, None, "name").unwrap_or("").to_owned(), attr(attrs, Some(NS_R), "id").unwrap_or("").to_owned()));
            } else if local == "workbookPr" {
                styles.date1904 = matches!(attr(attrs, None, "date1904"), Some("1" | "true"));
            }
        }
    }
    for (name, rid) in sheets {
        let Some((_, _, target)) = rels.iter().find(|(id, _, _)| *id == rid) else {
            continue;
        };
        let path = resolve(&wb, target);
        let Some(x) = zip.read_opt(&path)? else {
            continue;
        };
        push_line(out, &name)?;
        sheet_text(&x, &sst, &styles, limits, deadline, out)?;
        out.push('\n')?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// PPTX
// ---------------------------------------------------------------------------

fn drawing_text(xml: &[u8], limits: &Limits, deadline: Deadline, out: &mut TextSink) -> Result<()> {
    let mut s = XmlStream::new(xml, limits.max_depth).with_deadline(deadline);
    let mut para: Option<String> = None;
    let mut in_t = false;
    while let Some(ev) = s.next()? {
        match &ev {
            XEvent::Start { ns, local, .. } if ns.as_deref() == Some(NS_A) => match local.as_str() {
                "p" => para = Some(String::new()),
                "t" => in_t = true,
                "br" => {
                    if let Some(p) = para.as_mut() {
                        p.push('\n');
                    }
                }
                _ => {}
            },
            XEvent::End { ns, local } if ns.as_deref() == Some(NS_A) => match local.as_str() {
                "t" => in_t = false,
                "p" => {
                    let p = para.take().unwrap_or_default();
                    if !p.is_empty() {
                        push_line(out, &p)?;
                    }
                }
                _ => {}
            },
            XEvent::Text(t) if in_t => {
                if let Some(p) = para.as_mut() {
                    if p.len() + t.len() > limits.max_text_chars {
                        return Err(ParseError::LimitExceeded("paragraph too long".into()));
                    }
                    p.push_str(t);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn pptx_text(zip: &ZipArchive, limits: &Limits, deadline: Deadline, out: &mut TextSink) -> Result<()> {
    let pres = main_part(zip, limits, "ppt/presentation.xml")?;
    let rels = read_rels(zip, &pres, limits)?;
    let px = zip.read(&pres)?;
    let mut s = XmlStream::new(&px, limits.max_depth);
    let mut ids = Vec::new();
    while let Some(ev) = s.next()? {
        if let XEvent::Start { ns, local, attrs } = &ev
            && ns.as_deref() == Some(NS_P)
            && local == "sldId"
            && let Some(id) = attr(attrs, Some(NS_R), "id")
        {
            ids.push(id.to_owned());
        }
    }
    for rid in ids {
        deadline.check()?;
        let Some((_, _, target)) = rels.iter().find(|(id, _, _)| *id == rid) else {
            continue;
        };
        let slide = resolve(&pres, target);
        let Some(x) = zip.read_opt(&slide)? else {
            continue;
        };
        drawing_text(&x, limits, deadline, out)?;
        for (_, ty, t) in read_rels(zip, &slide, limits)? {
            if ty == REL_NOTES
                && let Some(n) = zip.read_opt(&resolve(&slide, &t))?
            {
                drawing_text(&n, limits, deadline, out)?;
            }
        }
        out.push('\n')?;
    }
    Ok(())
}

/// Title (placeholder `title`/`ctrTitle`) and remaining paragraphs of a slide or notes page.
fn slide_paragraphs(xml: &[u8], limits: &Limits, deadline: Deadline) -> Result<(Option<String>, Vec<String>)> {
    let mut s = XmlStream::new(xml, limits.max_depth).with_deadline(deadline);
    let mut title: Option<String> = None;
    let mut lines = Vec::new();
    let mut shape_is_title = false;
    let mut para: Option<String> = None;
    let mut in_t = false;
    let mut total = 0usize;
    while let Some(ev) = s.next()? {
        match &ev {
            XEvent::Start { ns, local, attrs } => match (ns.as_deref(), local.as_str()) {
                (Some(NS_P), "sp") => shape_is_title = false,
                (Some(NS_P), "ph") => {
                    shape_is_title = matches!(attr(attrs, None, "type"), Some("title") | Some("ctrTitle"));
                }
                (Some(NS_A), "p") => para = Some(String::new()),
                (Some(NS_A), "t") => in_t = true,
                (Some(NS_A), "br") => {
                    if let Some(p) = para.as_mut() {
                        p.push('\n');
                    }
                }
                _ => {}
            },
            XEvent::End { ns, local } if ns.as_deref() == Some(NS_A) => match local.as_str() {
                "t" => in_t = false,
                "p" => {
                    let p = para.take().unwrap_or_default();
                    let p = p.trim();
                    if !p.is_empty() {
                        if shape_is_title {
                            match title.as_mut() {
                                Some(t) => {
                                    t.push(' ');
                                    t.push_str(p);
                                }
                                None => title = Some(p.to_owned()),
                            }
                        } else {
                            lines.push(p.to_owned());
                        }
                    }
                }
                _ => {}
            },
            XEvent::Text(t) if in_t => {
                if let Some(p) = para.as_mut() {
                    total = total.saturating_add(t.len());
                    if total > limits.max_text_chars {
                        return Err(ParseError::LimitExceeded("slide text too long".into()));
                    }
                    p.push_str(t);
                }
            }
            _ => {}
        }
    }
    Ok((title, lines))
}

/// Slides of a PPTX/PPSX in presentation order (for the archive PDF).
pub(crate) fn pptx_slides(data: &[u8], limits: &Limits, deadline: Deadline) -> Result<Vec<super::Slide>> {
    let zip = ZipArchive::open(data, limits)?;
    let pres = main_part(&zip, limits, "ppt/presentation.xml")?;
    let rels = read_rels(&zip, &pres, limits)?;
    let px = zip.read(&pres)?;
    let mut s = XmlStream::new(&px, limits.max_depth);
    let mut ids = Vec::new();
    while let Some(ev) = s.next()? {
        if let XEvent::Start { ns, local, attrs } = &ev
            && ns.as_deref() == Some(NS_P)
            && local == "sldId"
            && let Some(id) = attr(attrs, Some(NS_R), "id")
        {
            ids.push(id.to_owned());
        }
    }
    let mut out = Vec::new();
    for rid in ids {
        deadline.check()?;
        let Some((_, _, target)) = rels.iter().find(|(id, _, _)| *id == rid) else {
            continue;
        };
        let slide = resolve(&pres, target);
        let Some(x) = zip.read_opt(&slide)? else {
            continue;
        };
        let (title, lines) = slide_paragraphs(&x, limits, deadline)?;
        let mut notes = Vec::new();
        for (_, ty, t) in read_rels(&zip, &slide, limits)? {
            if ty == REL_NOTES
                && let Some(n) = zip.read_opt(&resolve(&slide, &t))?
            {
                // Notes page: the slide placeholder carries no text, only the note text counts
                let (_, l) = slide_paragraphs(&n, limits, deadline)?;
                notes.extend(l.into_iter().filter(|l| !l.chars().all(|c| c.is_ascii_digit())));
            }
        }
        out.push(super::Slide { title, lines, notes });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Metadata (docProps/core.xml, docProps/app.xml) with Tika keys
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
fn core_metadata(zip: &ZipArchive, mime_type: &str, limits: &Limits) -> Result<(Vec<(String, String)>, Option<String>)> {
    let mut md: Vec<(String, String)> = Vec::new();
    let mut created = None;
    let core_map: &[(&str, &str, &str)] = &[
        (NS_DC, "title", "dc:title"),
        (NS_DC, "subject", "dc:subject"),
        (NS_DC, "creator", "dc:creator"),
        (NS_CP, "keywords", "meta:keyword"),
        (NS_DC, "description", "dc:description"),
        (NS_CP, "lastModifiedBy", "meta:last-author"),
        (NS_CP, "revision", "cp:revision"),
        (NS_CP, "category", "cp:category"),
        (NS_CP, "contentStatus", "cp:contentStatus"),
        (NS_DC, "language", "dc:language"),
        (NS_CP, "lastPrinted", "meta:print-date"),
        (NS_DCTERMS, "created", "dcterms:created"),
        (NS_DCTERMS, "modified", "dcterms:modified"),
        // Tika `MetadataExtractor.extractMetadata(CoreProperties)`
        (NS_DC, "identifier", "dc:identifier"),
        (NS_CP, "version", "cp:version"),
    ];
    let app_map: &[(&str, &str)] = &[
        ("Application", "extended-properties:Application"),
        ("AppVersion", "extended-properties:AppVersion"),
        ("Company", "extended-properties:Company"),
        ("Manager", "extended-properties:Manager"),
        ("Notes", "extended-properties:Notes"),
        ("PresentationFormat", "extended-properties:PresentationFormat"),
        ("Template", "extended-properties:Template"),
        ("TotalTime", "extended-properties:TotalTime"),
        ("HiddenSlides", "extended-properties:HiddenSlides"),
        ("DocSecurity", "extended-properties:DocSecurity"),
        ("Pages", "meta:page-count"),
        ("Slides", "meta:slide-count"),
        ("Words", "meta:word-count"),
        ("Characters", "meta:character-count"),
        ("CharactersWithSpaces", "meta:character-count-with-spaces"),
        ("Lines", "meta:line-count"),
        ("Paragraphs", "meta:paragraph-count"),
    ];
    let core_path = read_rels(zip, "", limits)
        .unwrap_or_default()
        .into_iter()
        .find(|(_, t, _)| t.ends_with("/core-properties"))
        .map(|(_, _, t)| resolve("", &t))
        .unwrap_or_else(|| "docProps/core.xml".into());
    if let Some(x) = zip.read_opt(&core_path)? {
        let vals = simple_children(&x, limits)?;
        for (ns, local, key) in core_map {
            if let Some((_, _, v)) = vals.iter().find(|(n, l, _)| n == ns && l == local) {
                md.push(((*key).to_owned(), v.clone()));
                if *key == "dcterms:created" {
                    created = Some(v.clone());
                }
            }
        }
    }
    if let Some(x) = zip.read_opt("docProps/app.xml")? {
        let vals = simple_children(&x, limits)?;
        let num = |local: &str| vals.iter().find(|(n, l, _)| n == NS_EP && l == local).and_then(|(_, _, v)| v.trim().parse::<i64>().ok());
        for (local, key) in app_map {
            if let Some((_, _, v)) = vals.iter().find(|(n, l, _)| n == NS_EP && l == local) {
                // Tika `addProperty(int)`: counter only if > 0
                let counter = matches!(
                    *local,
                    "Pages" | "Slides" | "Words" | "Characters" | "CharactersWithSpaces" | "Lines" | "Paragraphs" | "HiddenSlides"
                );
                if counter && !v.trim().parse::<i64>().is_ok_and(|n| n > 0) {
                    continue;
                }
                md.push(((*key).to_owned(), v.clone()));
                match *local {
                    // Tika: company also as dc:publisher
                    "Company" => md.push(("dc:publisher".into(), v.clone())),
                    "DocSecurity" => md.push(("extended-properties:DocSecurityString".into(), doc_security_string(v).into())),
                    _ => {}
                }
            }
        }
        // Tika: xmpTPg:NPages = pages, otherwise slides
        if let Some(n) = num("Pages").filter(|n| *n > 0).or_else(|| num("Slides").filter(|n| *n > 0)) {
            md.push(("xmpTPg:NPages".into(), n.to_string()));
        }
    }
    // Custom properties → `custom:<Name>` (Tika)
    let custom_path = read_rels(zip, "", limits)
        .unwrap_or_default()
        .into_iter()
        .find(|(_, t, _)| t.ends_with("/custom-properties"))
        .map(|(_, _, t)| resolve("", &t))
        .unwrap_or_else(|| "docProps/custom.xml".into());
    if let Some(x) = zip.read_opt(&custom_path)? {
        md.extend(custom_properties(&x, limits)?);
    }
    md.push(("Content-Type".into(), mime_type.to_owned()));
    Ok((md, created))
}

/// Tika `OfficeOpenXMLExtended.DOC_SECURITY_STRING`.
fn doc_security_string(v: &str) -> &'static str {
    match v.trim().parse::<i64>() {
        Ok(0) => "None",
        Ok(1) => "PasswordProtected",
        Ok(2) => "ReadOnlyRecommended",
        Ok(4) => "ReadOnlyEnforced",
        Ok(8) => "LockedForAnnotations",
        _ => "Unknown",
    }
}

/// `docProps/custom.xml`: `<property name="X"><vt:lpwstr>Wert</vt:lpwstr></property>` →
/// (`custom:X`, value). Boolean and date values as text as in the file.
fn custom_properties(xml: &[u8], limits: &Limits) -> Result<Vec<(String, String)>> {
    let mut s = XmlStream::new(xml, limits.max_depth);
    let mut out = Vec::new();
    let mut cur: Option<(String, String)> = None;
    let mut depth = 0usize;
    while let Some(ev) = s.next()? {
        match ev {
            XEvent::Start { local, attrs, .. } => {
                depth += 1;
                if depth == 2 && local == "property" {
                    cur = attr(&attrs, None, "name").map(|n| (format!("custom:{n}"), String::new()));
                }
            }
            XEvent::End { .. } => {
                if depth == 2
                    && let Some((k, v)) = cur.take()
                {
                    let v = v.trim().to_owned();
                    if !v.is_empty() && out.len() < 1000 {
                        out.push((k, v));
                    }
                }
                depth = depth.saturating_sub(1);
            }
            XEvent::Text(t) => {
                if let Some(c) = cur.as_mut()
                    && c.1.len() < 64 * 1024
                {
                    c.1.push_str(&t);
                }
            }
        }
    }
    Ok(out)
}

/// (ns, local, text) of the direct children of the root element.
fn simple_children(xml: &[u8], limits: &Limits) -> Result<Vec<(String, String, String)>> {
    let mut s = XmlStream::new(xml, limits.max_depth);
    let mut depth = 0usize;
    let mut out = Vec::new();
    let mut cur: Option<(String, String, String)> = None;
    while let Some(ev) = s.next()? {
        match ev {
            XEvent::Start { ns, local, .. } => {
                depth += 1;
                if depth == 2 {
                    cur = Some((ns.unwrap_or_default(), local, String::new()));
                }
            }
            XEvent::End { .. } => {
                if depth == 2
                    && let Some((n, l, t)) = cur.take()
                {
                    let t = t.trim().to_owned();
                    if !t.is_empty() {
                        out.push((n, l, t));
                    }
                }
                depth = depth.saturating_sub(1);
            }
            XEvent::Text(t) => {
                if let Some(c) = cur.as_mut()
                    && c.2.len() < 64 * 1024
                {
                    c.2.push_str(&t);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(resolve("xl/workbook.xml", "worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve("ppt/slides/slide1.xml", "../notesSlides/n1.xml"), "ppt/notesSlides/n1.xml");
        assert_eq!(resolve("", "word/document.xml"), "word/document.xml");
        assert_eq!(resolve("x/y.xml", "/abs/z.xml"), "abs/z.xml");
        assert_eq!(rels_path("word/document.xml"), "word/_rels/document.xml.rels");
        assert_eq!(rels_path(""), "_rels/.rels");
    }
}
