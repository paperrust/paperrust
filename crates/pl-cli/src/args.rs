//! Small argparse replacement: options with short/long names (like the `add_argument` calls of the
//! upstream commands), flag or value, `--opt=value`, positional arguments.
//!
//! upstream: argparse (CPython 3.12), django/core/management/base.py (BaseCommand.create_parser:
//! `--verbosity/-v`, `--settings`, `--pythonpath`, `--traceback`, `--no-color`, `--force-color`,
//! `--skip-checks` are accepted and ignored).

use std::collections::BTreeMap;

use crate::{CliError, CliResult};

/// One option: names (`-c`, `--compare-checksums`), destination (`compare_checksums`), takes a value?
#[derive(Debug, Clone, Copy)]
pub struct Opt {
    pub names: &'static [&'static str],
    pub dest: &'static str,
    /// 0 = flag, n = number of values
    pub nargs: usize,
}

pub const fn flag(names: &'static [&'static str], dest: &'static str) -> Opt {
    Opt { names, dest, nargs: 0 }
}

pub const fn value(names: &'static [&'static str], dest: &'static str) -> Opt {
    Opt { names, dest, nargs: 1 }
}

/// Django default options of every command (ignored).
const DJANGO_COMMON: &[Opt] = &[
    value(&["-v", "--verbosity"], "verbosity"),
    value(&["--settings"], "settings"),
    value(&["--pythonpath"], "pythonpath"),
    flag(&["--traceback"], "traceback"),
    flag(&["--no-color"], "no_color"),
    flag(&["--force-color"], "force_color"),
    flag(&["--skip-checks"], "skip_checks"),
];

/// PaperlessCommand: `--no-progress-bar`, `--processes` (base.py).
pub const PROGRESS: Opt = flag(&["--no-progress-bar"], "no_progress_bar");
pub const PROCESSES: Opt = value(&["--processes"], "processes");

#[derive(Debug, Default, Clone)]
pub struct Parsed {
    pub positionals: Vec<String>,
    values: BTreeMap<&'static str, Vec<String>>,
    flags: BTreeMap<&'static str, bool>,
}

impl Parsed {
    pub fn flag(&self, dest: &str) -> bool {
        self.flags.get(dest).copied().unwrap_or(false)
    }

    pub fn value(&self, dest: &str) -> Option<&str> {
        self.values.get(dest).and_then(|v| v.first()).map(String::as_str)
    }

    pub fn values(&self, dest: &str) -> Option<&[String]> {
        self.values.get(dest).map(Vec::as_slice)
    }

    pub fn int(&self, dest: &str) -> CliResult<Option<i64>> {
        match self.value(dest) {
            None => Ok(None),
            Some(v) => v
                .trim()
                .parse::<i64>()
                .map(Some)
                .map_err(|_| CliError::Usage(format!("argument --{}: invalid int value: '{v}'", dest.replace('_', "-")))),
        }
    }
}

/// Parses `args` against `opts` (+ Django default options). `max_positionals` = allowed count.
pub fn parse(args: &[String], opts: &[Opt], max_positionals: usize) -> CliResult<Parsed> {
    let mut out = Parsed::default();
    let all: Vec<&Opt> = opts.iter().chain(DJANGO_COMMON.iter()).collect();
    let find = |name: &str| all.iter().copied().find(|o| o.names.contains(&name));
    let mut i = 0;
    let mut only_positional = false;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if only_positional || !a.starts_with('-') || a == "-" {
            out.positionals.push(a.clone());
            continue;
        }
        if a == "--" {
            only_positional = true;
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let Some(opt) = find(&name) else {
            return Err(CliError::Usage(format!("unrecognized arguments: {a}")));
        };
        if opt.nargs == 0 {
            if inline.is_some() {
                return Err(CliError::Usage(format!("argument {name}: ignored explicit argument")));
            }
            out.flags.insert(opt.dest, true);
            continue;
        }
        let mut vals = Vec::new();
        if let Some(v) = inline {
            vals.push(v);
        }
        while vals.len() < opt.nargs {
            match args.get(i) {
                Some(v) if !(v.starts_with('-') && v.len() > 1 && v.parse::<f64>().is_err()) => {
                    vals.push(v.clone());
                    i += 1;
                }
                _ => {
                    let what =
                        if opt.nargs == 1 { "expected one argument".to_string() } else { format!("expected {} arguments", opt.nargs) };
                    return Err(CliError::Usage(format!("argument {}: {what}", opt.names.join("/"))));
                }
            }
        }
        out.values.insert(opt.dest, vals);
    }
    if out.positionals.len() > max_positionals {
        let extra: Vec<&str> = out.positionals[max_positionals..].iter().map(String::as_str).collect();
        return Err(CliError::Usage(format!("unrecognized arguments: {}", extra.join(" "))));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_flags_values_and_positionals() {
        let opts = [
            flag(&["-z", "--zip"], "zip"),
            value(&["-zn", "--zip-name"], "zip_name"),
            Opt { names: &["--id-range"], dest: "id_range", nargs: 2 },
        ];
        let p = parse(&s(&["/tmp/x", "-z", "--zip-name=foo", "--id-range", "1", "5", "-v", "2"]), &opts, 1).unwrap();
        assert!(p.flag("zip"));
        assert_eq!(p.value("zip_name"), Some("foo"));
        assert_eq!(p.values("id_range").unwrap(), &["1".to_string(), "5".to_string()]);
        assert_eq!(p.positionals, vec!["/tmp/x"]);
        assert!(parse(&s(&["--bogus"]), &opts, 1).is_err());
        assert!(parse(&s(&["a", "b"]), &opts, 1).is_err());
        assert!(parse(&s(&["--zip-name"]), &opts, 1).is_err());
        // negative number as value
        let p = parse(&s(&["--zip-name", "-3"]), &opts, 0).unwrap();
        assert_eq!(p.value("zip_name"), Some("-3"));
    }
}
