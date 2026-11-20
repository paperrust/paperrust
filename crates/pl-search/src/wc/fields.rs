// whoosh_compat/fields.py (whoosh-compat 0.3.0)
//
// FieldKind / Multitoken / ExistsStrategy / FieldRef / SubpathSpec /
// FieldSpec / ResolvedField / FieldRegistry.
//
// Skipped: FieldRegistry.__init__'s construction-time validation (duplicate
// names, alias collisions, unreachable subpath names, ...). The only
// registries built here are paperless' fixed PUBLIC_FIELDS (+ the bigram
// emit fields), which upstream validates successfully; a misconfiguration is
// therefore unreachable. Pickling support is not applicable.

use std::fmt;
use std::sync::Arc;

/// whoosh_compat/fields.py::FieldKind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldKind {
    Text,
    Keyword,
    U64,
    Date,
    Datetime,
    BooleanExists,
    Json,
}

impl FieldKind {
    /// `kind.name` (upper case, as Python's Enum member name).
    pub fn name(self) -> &'static str {
        match self {
            FieldKind::Text => "TEXT",
            FieldKind::Keyword => "KEYWORD",
            FieldKind::U64 => "U64",
            FieldKind::Date => "DATE",
            FieldKind::Datetime => "DATETIME",
            FieldKind::BooleanExists => "BOOLEAN_EXISTS",
            FieldKind::Json => "JSON",
        }
    }
}

/// whoosh_compat/fields.py::Multitoken
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Multitoken {
    Default,
    And,
    Or,
    Phrase,
    First,
}

/// whoosh_compat/fields.py::ExistsStrategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistsStrategy {
    FastField,
    FastJsonField,
    TermScan,
}

/// whoosh_compat/fields.py::resolve_exists_strategy
pub fn resolve_exists_strategy(kind: FieldKind, fast: bool) -> Option<ExistsStrategy> {
    if fast {
        if kind == FieldKind::Json {
            return Some(ExistsStrategy::FastJsonField);
        }
        return Some(ExistsStrategy::FastField);
    }
    if matches!(kind, FieldKind::Text | FieldKind::Keyword) {
        return Some(ExistsStrategy::TermScan);
    }
    None
}

/// whoosh_compat/fields.py::FieldRef
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldRef {
    pub name: String,
    pub json_path: Option<String>,
}

impl FieldRef {
    pub fn plain(name: impl Into<String>) -> Self {
        FieldRef { name: name.into(), json_path: None }
    }
    pub fn sub(name: impl Into<String>, path: impl Into<String>) -> Self {
        FieldRef { name: name.into(), json_path: Some(path.into()) }
    }
}

impl fmt::Display for FieldRef {
    /// `FieldRef.__str__`: "name.json_path" or "name".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.json_path {
            Some(p) => write!(f, "{}.{}", self.name, p),
            None => f.write_str(&self.name),
        }
    }
}

/// `FieldSpec.analyzer`: text -> tokens.
pub type Analyzer = Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>;
/// `PatternNormalizer`: fragment -> alternatives (a bare `str` is one).
pub type PatternNormalizer = Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>;

/// whoosh_compat/fields.py::FieldSpec (subpaths in canonical mapping form:
/// ordered `(subpath, default)` pairs = `SubpathSpec(default=...)`).
#[derive(Clone)]
pub struct FieldSpec {
    pub name: String,
    pub kind: FieldKind,
    pub aliases: Vec<String>,
    pub comma_values: bool,
    pub analyzer: Option<Analyzer>,
    pub pattern_normalizer: Option<PatternNormalizer>,
    pub multitoken: Multitoken,
    pub exists_target: Option<String>,
    pub subpaths: Vec<(String, bool)>,
    pub date_only: bool,
    pub fast: bool,
}

impl fmt::Debug for FieldSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldSpec")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl FieldSpec {
    pub fn new(name: &str, kind: FieldKind) -> Self {
        FieldSpec {
            name: name.to_string(),
            kind,
            aliases: Vec::new(),
            comma_values: false,
            analyzer: None,
            pattern_normalizer: None,
            multitoken: Multitoken::Default,
            exists_target: None,
            subpaths: Vec::new(),
            date_only: false,
            fast: false,
        }
    }

    /// `FieldSpec.default_subpath`
    pub fn default_subpath(&self) -> Option<&str> {
        self.subpaths.iter().find(|(_, d)| *d).map(|(p, _)| p.as_str())
    }

    pub fn has_subpath(&self, path: &str) -> bool {
        self.subpaths.iter().any(|(p, _)| p == path)
    }
}

/// whoosh_compat/fields.py::ResolvedField
#[derive(Debug, Clone)]
pub struct ResolvedField<'a> {
    pub spec: &'a FieldSpec,
    pub json_path: Option<String>,
}

impl ResolvedField<'_> {
    pub fn is_subpath(&self) -> bool {
        self.json_path.is_some()
    }
    /// `dotted_name`
    pub fn dotted_name(&self) -> String {
        match &self.json_path {
            Some(p) => format!("{}.{}", self.spec.name, p),
            None => self.spec.name.clone(),
        }
    }
}

/// whoosh_compat/fields.py::FieldRegistry
#[derive(Clone, Debug)]
pub struct FieldRegistry {
    specs: Vec<FieldSpec>,
    /// canonical name or alias -> index into `specs`
    by_name: Vec<(String, usize)>,
}

impl FieldRegistry {
    pub fn new(specs: Vec<FieldSpec>) -> Self {
        let mut by_name = Vec::new();
        for (i, s) in specs.iter().enumerate() {
            by_name.push((s.name.clone(), i));
            for a in &s.aliases {
                by_name.push((a.clone(), i));
            }
        }
        FieldRegistry { specs, by_name }
    }

    fn lookup(&self, raw: &str) -> Option<&FieldSpec> {
        self.by_name
            .iter()
            .find(|(n, _)| n == raw)
            .and_then(|(_, i)| self.specs.get(*i))
    }

    pub fn specs(&self) -> &[FieldSpec] {
        &self.specs
    }

    /// `FieldRegistry.resolve`
    pub fn resolve(&self, r: &FieldRef) -> Option<ResolvedField<'_>> {
        let spec = self.lookup(&r.name)?;
        if let Some(p) = &r.json_path {
            if spec.kind != FieldKind::Json || !spec.has_subpath(p) {
                return None;
            }
        }
        Some(ResolvedField { spec, json_path: r.json_path.clone() })
    }

    /// `FieldRegistry.make_ref`
    pub fn make_ref(&self, raw: &str) -> Option<FieldRef> {
        if let Some(spec) = self.lookup(raw) {
            if spec.kind != FieldKind::Json {
                return Some(FieldRef::plain(spec.name.clone()));
            }
            if let Some(d) = spec.default_subpath() {
                return Some(FieldRef::sub(spec.name.clone(), d));
            }
        }
        if let Some((name, subpath)) = raw.split_once('.') {
            if let Some(spec) = self.lookup(name) {
                if spec.kind == FieldKind::Json && spec.has_subpath(subpath) {
                    return Some(FieldRef::sub(spec.name.clone(), subpath));
                }
            }
        }
        None
    }

    /// `FieldRegistry.is_bare_json_field`
    pub fn is_bare_json_field(&self, raw: &str) -> bool {
        self.lookup(raw)
            .is_some_and(|s| s.kind == FieldKind::Json && s.default_subpath().is_none())
    }

    /// `FieldRegistry.exists_strategy` (resolved from kind/fast).
    pub fn exists_strategy(&self, spec: &FieldSpec) -> Option<ExistsStrategy> {
        resolve_exists_strategy(spec.kind, spec.fast)
    }

    /// `__contains__`
    pub fn contains(&self, name: &str) -> bool {
        self.make_ref(name).is_some()
    }

    /// The canonical spec registered under `name` (canonical or alias).
    pub fn spec(&self, name: &str) -> Option<&FieldSpec> {
        self.lookup(name)
    }
}
