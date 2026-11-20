//! Small XML tree for eSCL responses. Namespaces are ignored (local names only):
//! devices use different prefixes (`scan:`, `pwg:`, none), the meaning depends on the name.
//! Limits against malicious responses: depth and node count; DOCTYPE is rejected.

use quick_xml::events::Event;

#[derive(Debug, Clone, Default)]
pub struct Node {
    pub name: String,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    /// First child with this name.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
    /// All children with this name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    /// First descendant (incl. self) with this name, document order.
    pub fn find(&self, name: &str) -> Option<&Node> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }
    /// All descendants with this name.
    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Node>) {
        if self.name == name {
            out.push(self);
        }
        for c in &self.children {
            c.find_all(name, out);
        }
    }
    /// Text of the first descendant `name` (trimmed).
    pub fn text_of(&self, name: &str) -> Option<String> {
        self.find(name).map(|n| n.text.trim().to_string()).filter(|s| !s.is_empty())
    }
}

const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 20_000;

/// XML → tree. `None` for broken XML or exceeded limits.
pub fn parse(data: &[u8]) -> Option<Node> {
    let mut r = quick_xml::Reader::from_reader(data);
    let mut buf = Vec::new();
    let mut stack: Vec<Node> = Vec::new();
    let mut nodes = 0usize;
    loop {
        let ev = r.read_event_into(&mut buf).ok()?;
        match ev {
            Event::Eof => break,
            Event::DocType(_) => return None,
            Event::Start(ref e) | Event::Empty(ref e) => {
                nodes += 1;
                if nodes > MAX_NODES || stack.len() >= MAX_DEPTH {
                    return None;
                }
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let node = Node { name, ..Default::default() };
                if matches!(ev, Event::Empty(_)) {
                    match stack.last_mut() {
                        Some(p) => p.children.push(node),
                        None => return Some(node),
                    }
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                let done = stack.pop()?;
                match stack.last_mut() {
                    Some(p) => p.children.push(done),
                    None => return Some(done),
                }
            }
            Event::Text(ref t) => {
                if let Some(cur) = stack.last_mut() {
                    let s = t
                        .xml10_content()
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| String::from_utf8_lossy(t.as_ref()).into_owned());
                    cur.text.push_str(&s);
                }
            }
            Event::CData(ref t) => {
                if let Some(cur) = stack.last_mut() {
                    cur.text.push_str(&String::from_utf8_lossy(t.as_ref()));
                }
            }
            Event::GeneralRef(ref g) => {
                if let Some(cur) = stack.last_mut() {
                    let s = if g.is_char_ref() {
                        g.resolve_char_ref().ok().flatten().map(|c| c.to_string())
                    } else {
                        quick_xml::escape::resolve_predefined_entity(&String::from_utf8_lossy(g.as_ref())).map(str::to_owned)
                    };
                    if let Some(s) = s {
                        cur.text.push_str(&s);
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    None
}

/// Escape text for XML (element content).
pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c => o.push(c),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_ignores_prefixes() {
        let n = parse(br#"<?xml version="1.0"?><scan:A xmlns:scan="x" xmlns:pwg="y"><pwg:B>1 &amp; 2</pwg:B><C/><B>3</B></scan:A>"#).unwrap();
        assert_eq!(n.name, "A");
        assert_eq!(n.text_of("B").as_deref(), Some("1 & 2"));
        assert_eq!(n.children_named("B").count(), 2);
        assert!(n.child("C").is_some());
    }

    #[test]
    fn rejects_doctype_and_garbage() {
        assert!(parse(br#"<!DOCTYPE x [<!ENTITY a "b">]><x>&a;</x>"#).is_none());
        assert!(parse(b"<a><b></a>").is_none());
        assert!(parse(b"").is_none());
    }
}
