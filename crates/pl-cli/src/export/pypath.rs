//! `pathlib.PurePosixPath` semantics for relative export names (`suffix`, `stem`, `parent`,
//! `with_suffix("")`, `with_name`), as upstream uses them for archive names.
//!
//! upstream: CPython 3.12 Lib/pathlib.py.

fn split(p: &str) -> (&str, &str) {
    match p.rfind('/') {
        Some(i) => (&p[..i], &p[i + 1..]),
        None => ("", p),
    }
}

pub fn name(p: &str) -> &str {
    split(p).1
}

/// `PurePath.suffix`.
pub fn suffix(p: &str) -> &str {
    let n = name(p);
    match n.rfind('.') {
        Some(i) if i > 0 && i < n.len() - 1 => &n[i..],
        _ => "",
    }
}

/// `PurePath.stem`.
pub fn stem(p: &str) -> &str {
    let n = name(p);
    let s = suffix(p);
    &n[..n.len() - s.len()]
}

/// `str(PurePath.parent)` (`"."` without a directory).
pub fn parent(p: &str) -> &str {
    let (d, _) = split(p);
    if d.is_empty() { "." } else { d }
}

/// `parent / name` (`"." / x` = `x`).
pub fn join(dir: &str, name: &str) -> String {
    if dir == "." || dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// `str(Path(p).with_suffix(""))`.
pub fn without_suffix(p: &str) -> String {
    let s = suffix(p);
    p[..p.len() - s.len()].to_string()
}

/// Normalizes `\` (Windows `str(Path)`) to `/` and removes `./` parts.
pub fn posix(p: &str) -> String {
    let p = p.replace('\\', "/");
    p.split('/').filter(|s| !s.is_empty() && *s != ".").collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathlib_like() {
        assert_eq!(suffix("a/b.pdf"), ".pdf");
        assert_eq!(suffix(".bashrc"), "");
        assert_eq!(suffix("a."), "");
        assert_eq!(stem("x/2019 Foo 1.2 bar"), "2019 Foo 1");
        assert_eq!(parent("a.pdf"), ".");
        assert_eq!(parent("x/y/a.pdf"), "x/y");
        assert_eq!(join(".", "a"), "a");
        assert_eq!(without_suffix("x/a.tar.gz"), "x/a.tar");
        assert_eq!(posix("a\\b/./c"), "a/b/c");
    }
}
