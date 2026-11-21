//! Small XML layer on top of quick-xml: namespace-resolved events with depth and
//! size limits, plus a mini DOM (for XMP).

use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

use crate::{Deadline, ParseError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XAttr {
    pub ns: Option<String>,
    pub local: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub enum XEvent {
    Start { ns: Option<String>, local: String, attrs: Vec<XAttr> },
    End { ns: Option<String>, local: String },
    Text(String),
}

pub fn attr<'a>(attrs: &'a [XAttr], ns: Option<&str>, local: &str) -> Option<&'a str> {
    attrs.iter().find(|a| a.local == local && a.ns.as_deref() == ns).map(|a| a.value.as_str())
}

/// Streaming reader. `max_depth` limits nesting, `max_events` the number of events.
pub struct XmlStream<'a> {
    reader: NsReader<&'a [u8]>,
    buf: Vec<u8>,
    depth: usize,
    max_depth: usize,
    events: u64,
    max_events: u64,
    deadline: Option<Deadline>,
    pending_end: Option<XEvent>,
    done: bool,
}

impl<'a> XmlStream<'a> {
    pub fn new(data: &'a [u8], max_depth: usize) -> Self {
        let mut reader = NsReader::from_reader(data);
        {
            let cfg = reader.config_mut();
            cfg.trim_text_start = false;
            cfg.trim_text_end = false;
            cfg.check_end_names = false;
            cfg.expand_empty_elements = false;
            cfg.allow_unmatched_ends = true;
        }
        Self {
            reader,
            buf: Vec::with_capacity(1024),
            depth: 0,
            max_depth,
            events: 0,
            max_events: 50_000_000,
            deadline: None,
            pending_end: None,
            done: false,
        }
    }

    pub fn with_deadline(mut self, d: Deadline) -> Self {
        self.deadline = Some(d);
        self
    }

    fn resolve(r: ResolveResult<'_>) -> Option<String> {
        match r {
            ResolveResult::Bound(ns) => Some(String::from_utf8_lossy(ns.as_ref()).into_owned()),
            _ => None,
        }
    }

    /// Next event; `Ok(None)` at the end.
    pub fn next(&mut self) -> Result<Option<XEvent>> {
        if let Some(e) = self.pending_end.take() {
            return Ok(Some(e));
        }
        loop {
            if self.done {
                return Ok(None);
            }
            self.events += 1;
            if self.events > self.max_events {
                return Err(ParseError::LimitExceeded("too many XML events".into()));
            }
            if self.events.is_multiple_of(4096)
                && let Some(d) = &self.deadline
            {
                d.check()?;
            }
            self.buf.clear();
            let (res, ev) = self.reader.read_resolved_event_into(&mut self.buf).map_err(|e| ParseError::Invalid(format!("XML: {e}")))?;
            let ns = Self::resolve(res);
            let empty_ev = matches!(ev, Event::Empty(_));
            match ev {
                Event::Start(e) | Event::Empty(e) => {
                    let empty = empty_ev;
                    let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    let mut attrs = Vec::new();
                    for a in e.attributes().with_checks(false).flatten() {
                        let key = a.key;
                        if key.as_ref() == b"xmlns" || key.as_ref().starts_with(b"xmlns:") {
                            continue;
                        }
                        let (r, l) = self.reader.resolver().resolve_attribute(key);
                        let ans = Self::resolve(r);
                        let value = a
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map(|c| c.into_owned())
                            .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
                        attrs.push(XAttr { ns: ans, local: String::from_utf8_lossy(l.as_ref()).into_owned(), value });
                    }
                    if empty {
                        self.pending_end = Some(XEvent::End { ns: ns.clone(), local: local.clone() });
                    } else {
                        self.depth += 1;
                        if self.depth > self.max_depth {
                            return Err(ParseError::LimitExceeded("XML nesting too deep".into()));
                        }
                    }
                    return Ok(Some(XEvent::Start { ns, local, attrs }));
                }
                Event::End(e) => {
                    self.depth = self.depth.saturating_sub(1);
                    let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    return Ok(Some(XEvent::End { ns, local }));
                }
                Event::Text(t) => {
                    let s = t.xml10_content().map(|c| c.into_owned()).unwrap_or_else(|_| String::from_utf8_lossy(t.as_ref()).into_owned());
                    if !s.is_empty() {
                        return Ok(Some(XEvent::Text(s)));
                    }
                }
                Event::CData(t) => {
                    let s = String::from_utf8_lossy(t.as_ref()).into_owned();
                    if !s.is_empty() {
                        return Ok(Some(XEvent::Text(s)));
                    }
                }
                Event::GeneralRef(r) => {
                    let resolved: Option<String> = if r.is_char_ref() {
                        r.resolve_char_ref().ok().flatten().map(|c| c.to_string())
                    } else {
                        let name = String::from_utf8_lossy(r.as_ref()).into_owned();
                        quick_xml::escape::resolve_predefined_entity(&name).map(str::to_owned)
                    };
                    if let Some(s) = resolved {
                        return Ok(Some(XEvent::Text(s)));
                    }
                }
                Event::Eof => {
                    self.done = true;
                    return Ok(None);
                }
                _ => {}
            }
        }
    }
}

/// Mini DOM element (lxml semantics: `text` = text before the first child element).
#[derive(Debug, Clone, Default)]
pub struct Elem {
    pub ns: Option<String>,
    pub local: String,
    pub attrs: Vec<XAttr>,
    /// `None` like lxml if there is no text before the first child.
    pub text: Option<String>,
    pub children: Vec<Elem>,
}

impl Elem {
    pub fn is(&self, ns: &str, local: &str) -> bool {
        self.ns.as_deref() == Some(ns) && self.local == local
    }
    pub fn find(&self, ns: &str, local: &str) -> Option<&Elem> {
        self.children.iter().find(|c| c.is(ns, local))
    }
    /// First element `ns:local` in the subtree (incl. self), document order.
    pub fn find_descendant(&self, ns: &str, local: &str) -> Option<&Elem> {
        if self.is(ns, local) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find_descendant(ns, local))
    }
}

/// Builds a DOM; `max_nodes` limits the number of elements.
pub fn parse_dom(data: &[u8], max_depth: usize, max_nodes: usize) -> Result<Elem> {
    let mut s = XmlStream::new(data, max_depth);
    let mut stack: Vec<Elem> = Vec::new();
    let mut root: Option<Elem> = None;
    let mut nodes = 0usize;
    while let Some(ev) = s.next()? {
        match ev {
            XEvent::Start { ns, local, attrs } => {
                nodes += 1;
                if nodes > max_nodes {
                    return Err(ParseError::LimitExceeded("too many XML nodes".into()));
                }
                stack.push(Elem { ns, local, attrs, text: None, children: Vec::new() });
            }
            XEvent::End { .. } => {
                if let Some(done) = stack.pop() {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(done),
                        None => {
                            if root.is_none() {
                                root = Some(done);
                            }
                        }
                    }
                }
            }
            XEvent::Text(t) => {
                if let Some(cur) = stack.last_mut()
                    && cur.children.is_empty()
                {
                    cur.text.get_or_insert_with(String::new).push_str(&t);
                }
            }
        }
    }
    // Unclosed elements (broken XML) -> error like lxml (strict)
    if !stack.is_empty() {
        return Err(ParseError::Invalid("unclosed XML elements".into()));
    }
    root.ok_or_else(|| ParseError::Invalid("no XML root element".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dom_text_and_entities() {
        let d = parse_dom(br#"<a xmlns="urn:x" xmlns:p="urn:p" p:k="v&amp;w"> t&lt;1 <b/>tail</a>"#, 10, 100).unwrap();
        assert_eq!(d.ns.as_deref(), Some("urn:x"));
        assert_eq!(d.text.as_deref(), Some(" t<1 "));
        assert_eq!(d.attrs[0].ns.as_deref(), Some("urn:p"));
        assert_eq!(d.attrs[0].value, "v&w");
        assert_eq!(d.children.len(), 1);
    }

    #[test]
    fn depth_limit() {
        let deep = "<a>".repeat(50) + &"</a>".repeat(50);
        assert!(matches!(parse_dom(deep.as_bytes(), 20, 1000), Err(ParseError::LimitExceeded(_))));
    }
}
