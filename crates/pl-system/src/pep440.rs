//! PEP 440 version parsing and ordering as in `packaging.version.Version`.
//!
//! upstream: packaging 26.0 `packaging/version.py` (`VERSION_PATTERN`, `_cmpkey`), used by
//! src/documents/views.py@v3.4.2 (`RemoteVersionView`).

use std::cmp::Ordering;

/// Pre-release kind after normalisation (`alpha`→`a`, `beta`→`b`, `c|pre|preview`→`rc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PreKind {
    A,
    B,
    Rc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalPart {
    Num(u128),
    Str(String),
}

/// A parsed version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    epoch: u128,
    release: Vec<u128>,
    pre: Option<(PreKind, u128)>,
    post: Option<u128>,
    dev: Option<u128>,
    local: Option<Vec<LocalPart>>,
}

/// `packaging.version.InvalidVersion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidVersion(pub String);

impl std::fmt::Display for InvalidVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid version: {:?}", self.0)
    }
}

struct Cursor<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn eat_sep(&mut self) -> bool {
        if matches!(self.peek(), Some(b'.' | b'_' | b'-')) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn num(&mut self) -> Option<u128> {
        let start = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        if start == self.i {
            return None;
        }
        std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
    }
    /// Case-insensitive keyword match at the cursor (input is already lowercased).
    fn word(&mut self, words: &[&str]) -> Option<&'static str> {
        // longest match first
        let mut best: Option<&str> = None;
        for w in words {
            if self.s[self.i..].starts_with(w.as_bytes()) && best.is_none_or(|b| w.len() > b.len()) {
                best = Some(w);
            }
        }
        let b = best?;
        self.i += b.len();
        // map to a 'static canonical form
        Some(match b {
            "alpha" | "a" => "a",
            "beta" | "b" => "b",
            "rc" | "c" | "pre" | "preview" => "rc",
            "post" | "rev" | "r" => "post",
            "dev" => "dev",
            _ => "",
        })
    }
}

impl Version {
    /// `packaging.version.parse` (strict PEP 440, surrounding whitespace allowed).
    pub fn parse(input: &str) -> Result<Self, InvalidVersion> {
        let err = || InvalidVersion(input.to_string());
        let lower = input.trim_matches(|c: char| c.is_whitespace()).to_ascii_lowercase();
        let mut c = Cursor { s: lower.as_bytes(), i: 0 };
        if c.peek() == Some(b'v') {
            c.i += 1;
        }
        // epoch
        let mut epoch = 0;
        let save = c.i;
        if let Some(n) = c.num() {
            if c.peek() == Some(b'!') {
                c.i += 1;
                epoch = n;
            } else {
                c.i = save;
            }
        }
        // release
        let mut release = vec![c.num().ok_or_else(err)?];
        while c.peek() == Some(b'.') && matches!(c.s.get(c.i + 1), Some(b'0'..=b'9')) {
            c.i += 1;
            release.push(c.num().ok_or_else(err)?);
        }
        // pre
        let mut pre = None;
        let save = c.i;
        c.eat_sep();
        match c.word(&["alpha", "a", "beta", "b", "preview", "pre", "rc", "c"]) {
            Some(kind) => {
                let save2 = c.i;
                c.eat_sep();
                let n = match c.num() {
                    Some(n) => n,
                    None => {
                        c.i = save2;
                        0
                    }
                };
                let k = match kind {
                    "a" => PreKind::A,
                    "b" => PreKind::B,
                    _ => PreKind::Rc,
                };
                pre = Some((k, n));
            }
            None => c.i = save,
        }
        // post
        let mut post = None;
        let save = c.i;
        if c.peek() == Some(b'-') && matches!(c.s.get(c.i + 1), Some(b'0'..=b'9')) {
            c.i += 1;
            post = c.num();
        } else {
            c.eat_sep();
            if c.word(&["post", "rev", "r"]).is_some() {
                let save2 = c.i;
                c.eat_sep();
                post = Some(match c.num() {
                    Some(n) => n,
                    None => {
                        c.i = save2;
                        0
                    }
                });
            } else {
                c.i = save;
            }
        }
        // dev
        let mut dev = None;
        let save = c.i;
        c.eat_sep();
        if c.word(&["dev"]).is_some() {
            let save2 = c.i;
            c.eat_sep();
            dev = Some(match c.num() {
                Some(n) => n,
                None => {
                    c.i = save2;
                    0
                }
            });
        } else {
            c.i = save;
        }
        // local
        let mut local = None;
        if c.peek() == Some(b'+') {
            c.i += 1;
            let rest = std::str::from_utf8(&c.s[c.i..]).map_err(|_| err())?;
            let mut parts = Vec::new();
            for p in rest.split(['.', '_', '-']) {
                if p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric()) {
                    return Err(err());
                }
                parts.push(match p.parse::<u128>() {
                    Ok(n) if p.bytes().all(|b| b.is_ascii_digit()) => LocalPart::Num(n),
                    _ => LocalPart::Str(p.to_string()),
                });
            }
            c.i = c.s.len();
            local = Some(parts);
        }
        if c.i != c.s.len() {
            return Err(err());
        }
        Ok(Version { epoch, release, pre, post, dev, local })
    }

    fn release_key(&self) -> &[u128] {
        let mut end = self.release.len();
        while end > 0 && self.release[end - 1] == 0 {
            end -= 1;
        }
        &self.release[..end]
    }
}

/// Sort key element with -inf / +inf like packaging's `NegativeInfinity` / `Infinity`.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Bound<T: Ord> {
    NegInf,
    Val(T),
    Inf,
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        fn pre_key(v: &Version) -> Bound<(PreKind, u128)> {
            match (v.pre, v.post, v.dev) {
                (None, None, Some(_)) => Bound::NegInf,
                (None, _, _) => Bound::Inf,
                (Some(p), _, _) => Bound::Val(p),
            }
        }
        fn post_key(v: &Version) -> Bound<u128> {
            v.post.map_or(Bound::NegInf, Bound::Val)
        }
        fn dev_key(v: &Version) -> Bound<u128> {
            v.dev.map_or(Bound::Inf, Bound::Val)
        }
        fn local_cmp(a: &Option<Vec<LocalPart>>, b: &Option<Vec<LocalPart>>) -> Ordering {
            match (a, b) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
                (Some(a), Some(b)) => {
                    // ints sort above strings; strings compare lexically
                    for (x, y) in a.iter().zip(b.iter()) {
                        let o = match (x, y) {
                            (LocalPart::Num(x), LocalPart::Num(y)) => x.cmp(y),
                            (LocalPart::Num(_), LocalPart::Str(_)) => Ordering::Greater,
                            (LocalPart::Str(_), LocalPart::Num(_)) => Ordering::Less,
                            (LocalPart::Str(x), LocalPart::Str(y)) => x.cmp(y),
                        };
                        if o != Ordering::Equal {
                            return o;
                        }
                    }
                    a.len().cmp(&b.len())
                }
            }
        }
        self.epoch
            .cmp(&other.epoch)
            .then_with(|| self.release_key().cmp(other.release_key()))
            .then_with(|| pre_key(self).cmp(&pre_key(other)))
            .then_with(|| post_key(self).cmp(&post_key(other)))
            .then_with(|| dev_key(self).cmp(&dev_key(other)))
            .then_with(|| local_cmp(&self.local, &other.local))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_and_orders() {
        assert!(v("3.4.3") > v("3.4.2"));
        assert!(v("1.6.0") < v("3.4.2"));
        assert_eq!(v("3.4.2").cmp(&v("3.4.2.0")), Ordering::Equal);
        assert!(v("3.5.0rc1") > v("3.4.2"));
        assert!(v("3.4.2rc1") < v("3.4.2"));
        assert!(v("3.4.2.dev1") < v("3.4.2a1"));
        assert!(v("3.4.2.post1") > v("3.4.2"));
        assert!(v("3.4.2+local") > v("3.4.2"));
        assert!(v("1!0.1") > v("99.0"));
        assert!(v(" v3.4.2 ") == v("3.4.2"));
        assert!(v("3.4.2-1") == v("3.4.2.post1"));
        assert!(v("1.0-alpha.2") == v("1.0a2"));
        assert!(v("1.0.dev") == v("1.0.dev0"));
    }

    #[test]
    fn rejects_invalid() {
        for s in ["", "foo", "1.0.", "1..0", "1.0+", "1.0+a..b", "ngx-1.0", "1.0 beta"] {
            assert!(Version::parse(s).is_err(), "{s}");
        }
    }
}
