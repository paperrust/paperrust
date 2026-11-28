//! Migration helper (docs/SETTINGS.md §5, WP8): origin, "adopt into database", slim
//! `docker-compose.env`, checking uploaded `paperless.conf`/`.env`/Compose YAML.
//!
//! Pure logic (no DB); the API (`pl-api-v2::settings_migration`) and the CLI
//! (`manage settings origin|adopt|slim-env|check-file`) build on it.

use serde::Serialize;

use crate::layer::{Settings, Source};
use crate::schema::{Class, SettingDef, find};

/// Classification of a variable name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// A/B: can be adopted into the database.
    Adoptable,
    /// Already in the database with the same value – can be removed from the environment.
    Adopted,
    /// C/D: must stay in the environment.
    MustStay,
    /// X: no effect in PaperRust – can be removed.
    NoEffect,
    /// Older paperless-ngx names without effect in v3.4.2 (§7).
    Obsolete,
    /// Unknown (typo? another program?).
    Unknown,
}

/// An entry of the origin/check view.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// Name as in the source.
    pub name: String,
    /// Canonical key (if known).
    pub key: Option<&'static str>,
    /// Value (secrets: `None`).
    pub value: Option<String>,
    /// Raw value (never serialized; for `slim_env` from local files).
    #[serde(skip)]
    pub raw: String,
    pub secret: bool,
    /// `env` | `file` | `desktop` | `upload`
    pub source: &'static str,
    pub class: Option<&'static str>,
    pub verdict: Verdict,
    /// Reason/hint (e.g. successor of an obsolete name).
    pub reason: Option<String>,
}

/// Older names that paperless-ngx v3.4.2 no longer knows.
pub const OBSOLETE: &[(&str, &str)] = &[
    ("PAPERLESS_CONSUMER_INOTIFY_DELAY", "replaced by PAPERLESS_CONSUMER_STABILITY_DELAY"),
    ("PAPERLESS_FILENAME_PARSE_TRANSFORMS", "removed in paperless-ngx"),
    ("PAPERLESS_PASSPHRASE", "GPG storage was removed in paperless-ngx 2.x"),
    ("PAPERLESS_CONSUMER_POLLING_DELAY", "removed, see PAPERLESS_CONSUMER_STABILITY_DELAY"),
    ("PAPERLESS_CONSUMER_POLLING_RETRY_COUNT", "removed, see PAPERLESS_CONSUMER_STABILITY_DELAY"),
    ("PAPERLESS_WORKER_RETRY", "removed in paperless-ngx"),
    ("PAPERLESS_OCR_ALWAYS", "replaced by PAPERLESS_OCR_MODE"),
];

fn reason_for(def: &SettingDef) -> Option<String> {
    Some(
        match def.class {
            Class::C => "read at startup (database access, paths, bind address) – keep it in the environment",
            Class::D => "trust boundary to the host – only settable in the environment",
            Class::X => "has no effect in PaperRust – can be removed",
            _ => return None,
        }
        .to_string(),
    )
}

/// Name fragments that mark a variable as secret when the schema does not know it
/// (unknown/obsolete names and foreign lines such as `POSTGRES_PASSWORD`).
const SECRET_HINTS: &[&str] = &["PASS", "PWD", "SECRET", "TOKEN", "KEY", "CREDENTIAL", "PRIVATE"];

/// Heuristic for names outside the schema: does the name look like it holds a secret?
/// Known settings use the schema flag (`SettingDef::secret`) instead.
pub fn looks_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_HINTS.iter().any(|h| upper.contains(h))
}

/// Is the name one that we classify (PaperRust/paperless-ngx)?
pub fn is_ours(name: &str) -> bool {
    name.starts_with(pl_core::config::UPSTREAM_PREFIX) || name.starts_with(pl_core::config::PREFIX)
}

/// Classifies a name with value. `current` = running snapshot (for "already adopted").
pub fn classify(name: &str, value: &str, source: &'static str, current: Option<&Settings>) -> Item {
    // Unknown and obsolete names fall back to the name heuristic; known names use the schema.
    let heuristic_secret = looks_secret(name);
    let mut item = Item {
        name: name.to_string(),
        key: None,
        value: if heuristic_secret { None } else { Some(value.to_string()) },
        raw: value.to_string(),
        secret: heuristic_secret,
        source,
        class: None,
        verdict: Verdict::Unknown,
        reason: None,
    };
    if let Some((_, hint)) =
        OBSOLETE.iter().find(|(n, _)| *n == name || n.strip_prefix("PAPERLESS_").is_some_and(|r| name == format!("PAPERRUST_{r}")))
    {
        item.verdict = Verdict::Obsolete;
        item.reason = Some((*hint).to_string());
        return item;
    }
    let Some(def) = find(name) else { return item };
    item.key = Some(def.key);
    item.secret = def.secret;
    item.value = if def.secret { None } else { Some(value.to_string()) };
    item.class = Some(def.class.as_str());
    item.verdict = match def.class {
        Class::C | Class::D => Verdict::MustStay,
        Class::X => Verdict::NoEffect,
        Class::A | Class::B if !def.is_env() => Verdict::MustStay,
        Class::A | Class::B => {
            let adopted = current.and_then(|s| s.get(def.key)).is_some_and(|e| {
                (e.in_db || e.source == Source::AppConfig || e.shadowed.as_ref().is_some_and(|sh| sh.source == Source::Db))
                    && db_value(current, def) == Some(value.to_string())
            });
            if adopted { Verdict::Adopted } else { Verdict::Adoptable }
        }
    };
    item.reason = match item.verdict {
        Verdict::Adopted => Some("already in the database – can be removed from the environment".into()),
        _ => reason_for(def),
    };
    item
}

/// Value of a key in the database (`paperrust_setting` or AC column), if present.
fn db_value(s: Option<&Settings>, def: &SettingDef) -> Option<String> {
    let e = s?.get(def.key)?;
    match e.source {
        Source::Db | Source::AppConfig => e.value.clone(),
        _ => e.shadowed.as_ref().filter(|sh| matches!(sh.source, Source::Db | Source::AppConfig)).and_then(|sh| sh.value.clone()),
    }
}

/// Origin view of the running process: all values from environment/file/desktop app.
pub fn origin(s: &Settings) -> Vec<Item> {
    let mut out = Vec::new();
    for e in s.entries() {
        if e.def.internal || !e.def.is_env() {
            continue;
        }
        let src = match e.source {
            Source::Env => "env",
            Source::File => "file",
            Source::Desktop => "desktop",
            // AC value shadows an environment variable
            Source::AppConfig => match e.shadowed.as_ref().map(|sh| sh.source) {
                Some(Source::Env) => "env",
                Some(Source::File) => "file",
                Some(Source::Desktop) => "desktop",
                _ => continue,
            },
            _ => continue,
        };
        let raw = if e.source == Source::AppConfig { e.shadowed.as_ref().and_then(|sh| sh.value.clone()) } else { e.value.clone() };
        let Some(raw) = raw else { continue };
        let name = e.source_detail.clone().filter(|d| is_ours(d)).unwrap_or_else(|| e.def.key.to_string());
        let mut it = classify(&name, &raw, src, Some(s));
        if src == "desktop" && it.verdict == Verdict::Adoptable {
            it.verdict = Verdict::MustStay;
            it.reason = Some("managed by the desktop app".into());
        }
        out.push(it);
    }
    out
}

/// Checks uploaded variables (from [`parse_file`]).
pub fn check_pairs(pairs: &[(String, String)], current: Option<&Settings>) -> Vec<Item> {
    pairs.iter().filter(|(k, _)| is_ours(k)).map(|(k, v)| classify(k, v, "upload", current)).collect()
}

/// Detected file kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Dotenv,
    Compose,
}

/// Result of [`parse_file`].
#[derive(Debug, Clone, Serialize)]
pub struct ParsedFile {
    pub kind: FileKind,
    /// Service (Compose).
    pub service: Option<String>,
    pub pairs: Vec<(String, String)>,
    /// `env_file:` references (Compose; please upload the content as well).
    pub env_files: Vec<String>,
}

/// Reads `paperless.conf`/`docker-compose.env` (dotenv) or `docker-compose.yml`
/// (`environment:`/`env_file:` of the service `webserver`, otherwise of the first service with
/// PAPERLESS_ variables).
pub fn parse_file(text: &str) -> ParsedFile {
    if text.lines().any(|l| l.trim_end() == "services:")
        && let Some(p) = parse_compose(text)
    {
        return p;
    }
    ParsedFile { kind: FileKind::Dotenv, service: None, pairs: pl_core::config::parse_dotenv(text), env_files: Vec::new() }
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    let v = match v.find(" #") {
        Some(i) if !(v.starts_with('"') || v.starts_with('\'')) => v[..i].trim_end(),
        _ => v,
    };
    if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

fn parse_compose(text: &str) -> Option<ParsedFile> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.trim_end() == "services:")?;
    // Services: first level under services:
    let mut services: Vec<(String, usize, usize)> = Vec::new(); // (name, from, to)
    let mut svc_indent = None;
    for (i, l) in lines.iter().enumerate().skip(start + 1) {
        if l.trim().is_empty() || l.trim_start().starts_with('#') {
            continue;
        }
        let ind = indent(l);
        if ind == 0 {
            break;
        }
        let si = *svc_indent.get_or_insert(ind);
        if ind == si && l.trim_end().ends_with(':') {
            if let Some(last) = services.last_mut() {
                last.2 = i;
            }
            services.push((l.trim().trim_end_matches(':').to_string(), i + 1, lines.len()));
        }
    }
    if let Some(last) = services.last_mut() {
        let end = lines.iter().enumerate().skip(last.1).find(|(_, l)| !l.trim().is_empty() && indent(l) == 0).map(|(i, _)| i);
        if let Some(e) = end {
            last.2 = last.2.min(e);
        }
    }
    let parse_service = |from: usize, to: usize| -> (Vec<(String, String)>, Vec<String>) {
        let mut pairs = Vec::new();
        let mut files = Vec::new();
        let body: Vec<&str> = lines[from..to].to_vec();
        let child = body.iter().find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).map(|l| indent(l)).unwrap_or(0);
        let mut i = 0;
        while i < body.len() {
            let l = body[i];
            let t = l.trim();
            if indent(l) == child && (t.starts_with("environment:") || t.starts_with("env_file:")) {
                let is_env = t.starts_with("environment:");
                let inline = t.split_once(':').map(|(_, r)| r.trim()).unwrap_or("");
                if !inline.is_empty() && !is_env {
                    files.push(unquote(inline.trim_start_matches('[').trim_end_matches(']')));
                }
                i += 1;
                while i < body.len() {
                    let l2 = body[i];
                    let t2 = l2.trim();
                    if t2.is_empty() || t2.starts_with('#') {
                        i += 1;
                        continue;
                    }
                    if indent(l2) <= child {
                        break;
                    }
                    let item = t2.strip_prefix("- ").map(str::trim);
                    if is_env {
                        match item {
                            Some(it) => {
                                let it = unquote(it);
                                if let Some((k, v)) = it.split_once('=') {
                                    pairs.push((k.trim().to_string(), v.to_string()));
                                }
                            }
                            None => {
                                if let Some((k, v)) = t2.split_once(':') {
                                    pairs.push((k.trim().trim_matches('"').to_string(), unquote(v)));
                                }
                            }
                        }
                    } else if let Some(it) = item {
                        files.push(unquote(it.trim_start_matches("path:").trim()));
                    } else if let Some(p) = t2.strip_prefix("path:") {
                        files.push(unquote(p));
                    }
                    i += 1;
                }
                continue;
            }
            i += 1;
        }
        (pairs, files)
    };
    type Chosen = (String, Vec<(String, String)>, Vec<String>);
    let mut chosen: Option<Chosen> = None;
    for (name, from, to) in &services {
        let (pairs, files) = parse_service(*from, *to);
        let relevant = pairs.iter().any(|(k, _)| is_ours(k)) || !files.is_empty();
        if name == "webserver" && relevant {
            chosen = Some((name.clone(), pairs, files));
            break;
        }
        if chosen.is_none() && pairs.iter().any(|(k, _)| is_ours(k)) {
            chosen = Some((name.clone(), pairs, files));
        }
    }
    let (service, pairs, env_files) = chosen?;
    Some(ParsedFile { kind: FileKind::Compose, service: Some(service), pairs, env_files })
}

fn quote_env(v: &str) -> String {
    if v.is_empty() || v.chars().any(|c| c.is_whitespace() || matches!(c, '#' | '"' | '\'' | '$' | '\\')) {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        v.to_string()
    }
}

/// Generates a slim `docker-compose.env` (§5 no. 3): keeps C/D values, values not adopted
/// from A/B and unknown names; removes adopted, ineffective and obsolete ones with a comment.
/// With `mask_secrets`, secrets (schema secrets, and secret-looking names among unknown names
/// and `other_lines`) appear only as a comment, never with their value.
pub fn slim_env(items: &[Item], other_lines: &[(String, String)], mask_secrets: bool) -> String {
    let mut keep = Vec::new();
    let mut removed = Vec::new();
    for it in items {
        match it.verdict {
            Verdict::Adopted => removed.push(format!("# {} – {}", it.name, "already in the database (web UI)")),
            Verdict::NoEffect => removed.push(format!("# {} – no effect in PaperRust", it.name)),
            Verdict::Obsolete => removed.push(format!("# {} – {}", it.name, it.reason.clone().unwrap_or_default())),
            _ => {
                let line = if it.secret && mask_secrets {
                    format!("# {}=<secret – copy the value from your current configuration>", it.name)
                } else {
                    format!("{}={}", it.name, quote_env(&it.raw))
                };
                let note = match it.verdict {
                    Verdict::Adoptable => "  # can be moved to the web UI (adopt)",
                    Verdict::Unknown => "  # unknown to PaperRust",
                    _ => "",
                };
                keep.push(format!("{line}{note}"));
            }
        }
    }
    let mut out = String::from("# docker-compose.env generated by PaperRust (only what must stay in the environment)\n");
    for l in keep {
        out.push_str(&l);
        out.push('\n');
    }
    for (k, v) in other_lines {
        if mask_secrets && looks_secret(k) {
            out.push_str(&format!("# {k}=<secret – copy the value from your current configuration>\n"));
        } else {
            out.push_str(&format!("{k}={}\n", quote_env(v)));
        }
    }
    if !removed.is_empty() {
        out.push_str("\n# removed:\n");
        for l in removed {
            out.push_str(&l);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{DbEntry, DbValue, Layers};
    use pl_core::config::Env;

    #[test]
    fn classify_names() {
        assert_eq!(classify("PAPERLESS_DBHOST", "db", "env", None).verdict, Verdict::MustStay);
        assert_eq!(classify("PAPERLESS_TRUSTED_PROXIES", "x", "env", None).verdict, Verdict::MustStay);
        assert_eq!(classify("PAPERLESS_TIKA_ENABLED", "1", "env", None).verdict, Verdict::NoEffect);
        assert_eq!(classify("PAPERLESS_TIME_ZONE", "Europe/Zurich", "env", None).verdict, Verdict::Adoptable);
        assert_eq!(classify("PAPERRUST_TIME_ZONE", "Europe/Zurich", "env", None).key, Some("PAPERLESS_TIME_ZONE"));
        assert_eq!(classify("PAPERLESS_CONSUMER_INOTIFY_DELAY", "1", "env", None).verdict, Verdict::Obsolete);
        assert_eq!(classify("PAPERLESS_TYPO_X", "1", "env", None).verdict, Verdict::Unknown);
        let s = classify("PAPERLESS_EMAIL_HOST_PASSWORD", "pw", "env", None);
        assert!(s.secret && s.value.is_none());
        // already adopted
        let mut l = Layers::new(Env::from_pairs([("PAPERLESS_EMAIL_HOST", "smtp")]));
        l.db.insert(
            "PAPERLESS_EMAIL_HOST".into(),
            DbEntry { value: DbValue::Plain("smtp".into()), rev: 1, updated_at: None, updated_by: None },
        );
        let snap = Settings::build(&l, 1, None);
        assert_eq!(classify("PAPERLESS_EMAIL_HOST", "smtp", "env", Some(&snap)).verdict, Verdict::Adopted);
        assert_eq!(classify("PAPERLESS_EMAIL_HOST", "other", "env", Some(&snap)).verdict, Verdict::Adoptable);
        let o = origin(&snap);
        assert_eq!(o.len(), 1);
        assert_eq!((o[0].name.as_str(), o[0].verdict), ("PAPERLESS_EMAIL_HOST", Verdict::Adopted));
    }

    /// upstream examples from `upstream/docker/compose` are classified correctly (WP8 test).
    #[test]
    fn upstream_compose_examples() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../upstream/docker/compose");
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut seen = 0;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            let p = parse_file(&text);
            // Installation examples (ci-test/portainer are special cases without env_file)
            if name.ends_with(".yml") && ["postgres", "mariadb", "sqlite"].iter().any(|d| name.starts_with(&format!("docker-compose.{d}")))
            {
                assert_eq!(p.kind, FileKind::Compose, "{name}");
                assert_eq!(p.service.as_deref(), Some("webserver"), "{name}");
                assert!(p.env_files.iter().any(|f| f.contains("docker-compose.env")), "{name}: {:?}", p.env_files);
                seen += 1;
            }
            if name == "docker-compose.env" {
                assert_eq!(p.kind, FileKind::Dotenv);
                let items = check_pairs(&p.pairs, None);
                assert!(items.iter().all(|i| i.verdict != Verdict::Unknown), "{items:?}");
            }
            for it in check_pairs(&p.pairs, None) {
                // DB host from the upstream example stays in the environment; PaperRust does not need Redis
                if it.name == "PAPERLESS_DBHOST" {
                    assert_eq!(it.verdict, Verdict::MustStay, "{name}");
                }
                if it.name == "PAPERLESS_REDIS" {
                    assert_eq!(it.verdict, Verdict::NoEffect, "{name}");
                }
            }
        }
        assert!(seen >= 3, "upstream compose examples not found");
    }

    #[test]
    fn compose_list_and_map_forms() {
        let y = "services:\n  broker:\n    image: redis\n  webserver:\n    image: x\n    env_file: docker-compose.env\n    environment:\n      PAPERLESS_REDIS: redis://broker:6379\n      PAPERLESS_DBHOST: \"db\"\n  other:\n    environment:\n      - PAPERLESS_TIME_ZONE=Europe/Zurich\nvolumes:\n  data:\n";
        let p = parse_file(y);
        assert_eq!(p.service.as_deref(), Some("webserver"));
        assert_eq!(p.env_files, vec!["docker-compose.env".to_string()]);
        assert_eq!(p.pairs, vec![("PAPERLESS_REDIS".into(), "redis://broker:6379".into()), ("PAPERLESS_DBHOST".into(), "db".into())]);
        let y2 = "services:\n  app:\n    environment:\n      - PAPERLESS_TIME_ZONE=Europe/Zurich\n      - \"PAPERLESS_OCR_LANGUAGE=deu\"\n";
        assert_eq!(parse_file(y2).pairs.len(), 2);
    }

    #[test]
    fn slim_env_output() {
        let items = vec![
            classify("PAPERLESS_DBHOST", "db", "upload", None),
            classify("PAPERLESS_TIKA_ENABLED", "1", "upload", None),
            classify("PAPERLESS_SECRET_KEY", "s3cr3t", "upload", None),
            classify("PAPERLESS_OCR_LANGUAGE", "deu eng", "upload", None),
        ];
        let out = slim_env(&items, &[], true);
        assert!(out.contains("PAPERLESS_DBHOST=db\n"));
        assert!(out.contains("# PAPERLESS_TIKA_ENABLED – no effect"));
        assert!(!out.contains("s3cr3t") && out.contains("# PAPERLESS_SECRET_KEY=<secret"));
        assert!(out.contains("PAPERLESS_OCR_LANGUAGE=\"deu eng\"  # can be moved"));
    }

    /// Regression: obsolete and unknown secret-looking names must not echo their value.
    #[test]
    fn obsolete_and_unknown_secrets_are_masked() {
        let it = classify("PAPERLESS_PASSPHRASE", "gpg-pass", "upload", None);
        assert_eq!(it.verdict, Verdict::Obsolete);
        assert!(it.secret && it.value.is_none());
        assert!(!serde_json::to_string(&it).unwrap_or_default().contains("gpg-pass"));
        let it = classify(&format!("PAPERRUST_{}", "PASSPHRASE"), "gpg-pass", "upload", None);
        assert!(it.secret && it.value.is_none());
        let it = classify("PAPERLESS_MY_API_TOKEN", "tok", "upload", None);
        assert_eq!(it.verdict, Verdict::Unknown);
        assert!(it.secret && it.value.is_none());
        // non-secret obsolete/unknown names keep their value
        assert_eq!(classify("PAPERLESS_OCR_ALWAYS", "true", "upload", None).value.as_deref(), Some("true"));
        assert_eq!(classify("PAPERLESS_TYPO_X", "1", "upload", None).value.as_deref(), Some("1"));
        // the schema flag wins for known names (TOKEN_THROTTLE_RATE is a rate, not a secret)
        assert_eq!(classify("PAPERLESS_TOKEN_THROTTLE_RATE", "5/min", "upload", None).value.as_deref(), Some("5/min"));
    }

    /// Regression: with masking, secrets in items and in foreign lines never appear in clear text.
    #[test]
    fn slim_env_masks_all_secrets() {
        let items = vec![
            classify("PAPERLESS_DBPASS", "dbpw", "upload", None),
            classify("PAPERLESS_PASSPHRASE", "gpg-pass", "upload", None),
            classify("PAPERLESS_MY_API_TOKEN", "tok123", "upload", None),
            classify("PAPERLESS_DBHOST", "db", "upload", None),
        ];
        let others = vec![
            ("POSTGRES_PASSWORD".to_string(), "pgpw".to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), "awssecret".to_string()),
            ("USERMAP_UID".to_string(), "1000".to_string()),
        ];
        let out = slim_env(&items, &others, true);
        for secret in ["dbpw", "gpg-pass", "tok123", "pgpw", "awssecret"] {
            assert!(!out.contains(secret), "{secret} leaked: {out}");
        }
        assert!(out.contains("# POSTGRES_PASSWORD=<secret"));
        assert!(out.contains("PAPERLESS_DBHOST=db\n"));
        assert!(out.contains("USERMAP_UID=1000\n"));
        // unmasked (local CLI) keeps the values
        let raw = slim_env(&items, &others, false);
        assert!(raw.contains("PAPERLESS_DBPASS=dbpw") && raw.contains("POSTGRES_PASSWORD=pgpw"));
    }
}
