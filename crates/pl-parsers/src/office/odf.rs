//! OpenDocument (ODT, ODS, ODP, ODG): `content.xml` + `meta.xml`, streamed with limits.

use crate::limits::TextSink;
use crate::xmlutil::{XEvent, XmlStream, attr};
use crate::zipread::ZipArchive;
use crate::{Deadline, Limits, ParseError, Result};

use super::{OfficeContent, push_line};

const NS_TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const NS_TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const NS_OFFICE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
const NS_META: &str = "urn:oasis:names:tc:opendocument:xmlns:meta:1.0";
const NS_DC: &str = "http://purl.org/dc/elements/1.1/";

/// Upper limit for `number-columns-repeated` / `number-rows-repeated` of non-empty content.
const MAX_REPEAT: usize = 100;

pub fn extract(
    data: &[u8],
    mime_type: &str,
    limits: &Limits,
    deadline: Deadline,
) -> Result<OfficeContent> {
    let zip = ZipArchive::open(data, limits)?;
    let content = zip.read("content.xml")?;
    let mut out = TextSink::new(limits.max_text_chars);
    let spreadsheet = mime_type == "application/vnd.oasis.opendocument.spreadsheet";
    content_text(&content, spreadsheet, limits, deadline, &mut out)?;
    let (metadata, created) = match zip.read_opt("meta.xml")? {
        Some(m) => meta(&m, mime_type, limits)?,
        None => (
            vec![("Content-Type".to_owned(), mime_type.to_owned())],
            None,
        ),
    };
    Ok(OfficeContent {
        text: out.buf,
        metadata,
        created,
    })
}

struct Cell {
    text: String,
    repeat: usize,
}

fn content_text(
    xml: &[u8],
    spreadsheet: bool,
    limits: &Limits,
    deadline: Deadline,
    out: &mut TextSink,
) -> Result<()> {
    let mut s = XmlStream::new(xml, limits.max_depth).with_deadline(deadline);
    let mut in_body = false;
    let mut paras: Vec<String> = Vec::new();
    let mut cells: Vec<Cell> = Vec::new();
    let mut rows: Vec<(Vec<Cell>, usize)> = Vec::new();
    let mut table_depth = 0usize;
    let mut skip = 0usize;
    while let Some(ev) = s.next()? {
        match &ev {
            XEvent::Start { ns, local, attrs } => {
                let ns = ns.as_deref();
                if ns == Some(NS_OFFICE) && local == "body" {
                    in_body = true;
                    continue;
                }
                if !in_body {
                    continue;
                }
                if skip > 0 {
                    skip += 1;
                    continue;
                }
                match (ns, local.as_str()) {
                    (Some(NS_TEXT), "tracked-changes") | (Some(NS_TEXT), "sequence-decls") => {
                        skip = 1
                    }
                    (Some(NS_TEXT), "p") | (Some(NS_TEXT), "h") => paras.push(String::new()),
                    (Some(NS_TEXT), "s") => {
                        let n: usize = attr(attrs, Some(NS_TEXT), "c")
                            .and_then(|c| c.parse().ok())
                            .unwrap_or(1);
                        if let Some(p) = paras.last_mut() {
                            p.extend(std::iter::repeat_n(' ', n.min(1000)));
                        }
                    }
                    (Some(NS_TEXT), "tab") => {
                        if let Some(p) = paras.last_mut() {
                            p.push('\t');
                        }
                    }
                    (Some(NS_TEXT), "line-break") => {
                        if let Some(p) = paras.last_mut() {
                            p.push('\n');
                        }
                    }
                    (Some(NS_TABLE), "table") => {
                        table_depth += 1;
                        if spreadsheet
                            && table_depth == 1
                            && let Some(name) = attr(attrs, Some(NS_TABLE), "name")
                        {
                            push_line(out, name)?;
                        }
                    }
                    (Some(NS_TABLE), "table-row") => {
                        let rep: usize = attr(attrs, Some(NS_TABLE), "number-rows-repeated")
                            .and_then(|c| c.parse().ok())
                            .unwrap_or(1);
                        rows.push((Vec::new(), rep));
                    }
                    (Some(NS_TABLE), "table-cell") | (Some(NS_TABLE), "covered-table-cell") => {
                        let rep: usize = attr(attrs, Some(NS_TABLE), "number-columns-repeated")
                            .and_then(|c| c.parse().ok())
                            .unwrap_or(1);
                        cells.push(Cell {
                            text: String::new(),
                            repeat: rep,
                        });
                    }
                    _ => {}
                }
            }
            XEvent::End { ns, local } => {
                let ns = ns.as_deref();
                if ns == Some(NS_OFFICE) && local == "body" {
                    in_body = false;
                    continue;
                }
                if !in_body {
                    continue;
                }
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                match (ns, local.as_str()) {
                    (Some(NS_TEXT), "p") | (Some(NS_TEXT), "h") => {
                        let line = paras.pop().unwrap_or_default();
                        if let Some(parent) = paras.last_mut() {
                            parent.push_str(&line);
                        } else if let Some(c) = cells.last_mut() {
                            if !c.text.is_empty() {
                                c.text.push(' ');
                            }
                            c.text.push_str(&line);
                        } else {
                            push_line(out, &line)?;
                        }
                    }
                    (Some(NS_TABLE), "table-cell") | (Some(NS_TABLE), "covered-table-cell") => {
                        if let Some(c) = cells.pop()
                            && let Some(r) = rows.last_mut()
                        {
                            r.0.push(c);
                        }
                    }
                    (Some(NS_TABLE), "table-row") => {
                        let (mut rc, rep) = rows.pop().unwrap_or_default();
                        while rc.last().is_some_and(|c| c.text.is_empty()) {
                            rc.pop();
                        }
                        if rc.is_empty() {
                            continue;
                        }
                        let mut parts: Vec<&str> = Vec::new();
                        for c in &rc {
                            for _ in 0..c.repeat.clamp(1, MAX_REPEAT) {
                                parts.push(&c.text);
                            }
                        }
                        let line = parts.join("\t");
                        for _ in 0..rep.clamp(1, MAX_REPEAT) {
                            if let Some(c) = cells.last_mut() {
                                if !c.text.is_empty() {
                                    c.text.push(' ');
                                }
                                c.text.push_str(&line);
                            } else {
                                if spreadsheet {
                                    out.push('\t')?;
                                }
                                push_line(out, &line)?;
                            }
                        }
                    }
                    (Some(NS_TABLE), "table") => {
                        table_depth = table_depth.saturating_sub(1);
                        if spreadsheet && table_depth == 0 {
                            out.push('\n')?;
                        }
                    }
                    _ => {}
                }
            }
            XEvent::Text(t) if in_body && skip == 0 => {
                if let Some(p) = paras.last_mut() {
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

#[allow(clippy::type_complexity)]
fn meta(
    xml: &[u8],
    mime_type: &str,
    limits: &Limits,
) -> Result<(Vec<(String, String)>, Option<String>)> {
    let mut s = XmlStream::new(xml, limits.max_depth);
    let mut md: Vec<(String, String)> = Vec::new();
    let mut created = None;
    let mut cur: Option<(String, String)> = None;
    let mut keywords: Vec<String> = Vec::new();
    while let Some(ev) = s.next()? {
        match ev {
            XEvent::Start { ns, local, attrs } => {
                let key = match (ns.as_deref(), local.as_str()) {
                    (Some(NS_META), "generator") => Some("generator"),
                    (Some(NS_DC), "title") => Some("dc:title"),
                    (Some(NS_DC), "subject") => Some("dc:subject"),
                    (Some(NS_DC), "description") => Some("dc:description"),
                    (Some(NS_DC), "creator") => Some("dc:creator"),
                    (Some(NS_DC), "language") => Some("dc:language"),
                    (Some(NS_META), "initial-creator") => Some("meta:initial-author"),
                    (Some(NS_META), "creation-date") => Some("dcterms:created"),
                    (Some(NS_DC), "date") => Some("dcterms:modified"),
                    (Some(NS_META), "print-date") => Some("meta:print-date"),
                    (Some(NS_META), "editing-cycles") => Some("meta:editing-cycles"),
                    (Some(NS_META), "keyword") => Some("meta:keyword"),
                    (Some(NS_META), "user-defined") => {
                        let name = attr(&attrs, Some(NS_META), "name").unwrap_or("").to_owned();
                        cur = Some((format!("custom:{name}"), String::new()));
                        None
                    }
                    (Some(NS_META), "document-statistic") => {
                        for a in &attrs {
                            if a.ns.as_deref() == Some(NS_META) {
                                md.push((format!("meta:{}", a.local), a.value.clone()));
                            }
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(k) = key {
                    cur = Some((k.to_owned(), String::new()));
                }
            }
            XEvent::Text(t) => {
                if let Some(c) = cur.as_mut()
                    && c.1.len() < 64 * 1024
                {
                    c.1.push_str(&t);
                }
            }
            XEvent::End { .. } => {
                if let Some((k, v)) = cur.take() {
                    let v = v.trim().to_owned();
                    if v.is_empty() {
                        continue;
                    }
                    if k == "meta:keyword" {
                        keywords.push(v);
                        continue;
                    }
                    if k == "dcterms:created" {
                        created = Some(v.clone());
                    }
                    md.push((k, v));
                }
            }
        }
    }
    if !keywords.is_empty() {
        md.push(("meta:keyword".into(), keywords.join(", ")));
    }
    md.push(("Content-Type".into(), mime_type.to_owned()));
    Ok((md, created))
}

const NS_DRAW: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
const NS_PRESENTATION: &str = "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0";

/// Pages (`draw:page`) of an ODP/ODG: title from the frame `presentation:class="title"`,
/// paragraphs, notes from `presentation:notes` (for the archive PDF).
pub(crate) fn odp_slides(data: &[u8], limits: &Limits, deadline: Deadline) -> Result<Vec<super::Slide>> {
    let zip = ZipArchive::open(data, limits)?;
    let content = zip.read("content.xml")?;
    let mut s = XmlStream::new(&content, limits.max_depth).with_deadline(deadline);
    let mut slides: Vec<super::Slide> = Vec::new();
    let mut in_page = false;
    let mut notes_depth = 0usize;
    let mut title_frames = 0usize;
    let mut frame_stack: Vec<bool> = Vec::new();
    let mut paras: Vec<String> = Vec::new();
    let mut total = 0usize;
    while let Some(ev) = s.next()? {
        match &ev {
            XEvent::Start { ns, local, attrs } => {
                let ns = ns.as_deref();
                match (ns, local.as_str()) {
                    (Some(NS_DRAW), "page") => {
                        in_page = true;
                        slides.push(super::Slide::default());
                    }
                    (Some(NS_PRESENTATION), "notes") if in_page => notes_depth += 1,
                    (Some(NS_DRAW), "frame") => {
                        let t = attr(attrs, Some(NS_PRESENTATION), "class") == Some("title");
                        frame_stack.push(t);
                        if t {
                            title_frames += 1;
                        }
                    }
                    (Some(NS_TEXT), "p") | (Some(NS_TEXT), "h") if in_page => paras.push(String::new()),
                    (Some(NS_TEXT), "s") => {
                        let n: usize = attr(attrs, Some(NS_TEXT), "c").and_then(|c| c.parse().ok()).unwrap_or(1);
                        if let Some(p) = paras.last_mut() {
                            p.extend(std::iter::repeat_n(' ', n.min(1000)));
                        }
                    }
                    (Some(NS_TEXT), "tab") => {
                        if let Some(p) = paras.last_mut() {
                            p.push('\t');
                        }
                    }
                    (Some(NS_TEXT), "line-break") => {
                        if let Some(p) = paras.last_mut() {
                            p.push('\n');
                        }
                    }
                    _ => {}
                }
            }
            XEvent::End { ns, local } => {
                let ns = ns.as_deref();
                match (ns, local.as_str()) {
                    (Some(NS_DRAW), "page") => in_page = false,
                    (Some(NS_PRESENTATION), "notes") if in_page => notes_depth = notes_depth.saturating_sub(1),
                    (Some(NS_DRAW), "frame") => {
                        if frame_stack.pop() == Some(true) {
                            title_frames = title_frames.saturating_sub(1);
                        }
                    }
                    (Some(NS_TEXT), "p") | (Some(NS_TEXT), "h") if in_page => {
                        let p = paras.pop().unwrap_or_default();
                        let p = p.trim();
                        if p.is_empty() {
                            continue;
                        }
                        // append nested paragraphs (lists in frames) to the outer one
                        if let Some(outer) = paras.last_mut() {
                            if !outer.is_empty() {
                                outer.push('\n');
                            }
                            outer.push_str(p);
                            continue;
                        }
                        let Some(slide) = slides.last_mut() else { continue };
                        if notes_depth > 0 {
                            slide.notes.push(p.to_owned());
                        } else if title_frames > 0 && slide.title.is_none() {
                            slide.title = Some(p.to_owned());
                        } else {
                            slide.lines.push(p.to_owned());
                        }
                    }
                    _ => {}
                }
            }
            XEvent::Text(t) => {
                if let Some(p) = paras.last_mut() {
                    total = total.saturating_add(t.len());
                    if total > limits.max_text_chars {
                        return Err(ParseError::LimitExceeded("slide text too long".into()));
                    }
                    p.push_str(t);
                }
            }
        }
    }
    Ok(slides)
}
