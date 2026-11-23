//! WP1 acceptance (docs/SETTINGS.md §6): `inventory_complete` and `parse_parity`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use pl_core::config::{Env, OWN_SETTINGS};
use pl_settings::layer::{DbEntry, DbValue, Layers, Settings};
use pl_settings::schema::{Class, SCHEMA, Ty, counts, find};
use pl_settings::value::{self, parse_from_env};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Deliberately not in the inventory: test variables (§1.16a last paragraph), internal signals between
/// supervisor and server, name variants without a meaning of their own.
const EXCEPTIONS: &[(&str, &str)] = &[
    // removed 2026-11-14 (Django forwarding, Redis); only still in the startup hint
    // `pl_core::config::retired_warnings` and in tests that they have no effect
    ("PAPERRUST_PROXY_CONNECT_TIMEOUT_SECS", "entfernt, nur Warnung"),
    ("PAPERRUST_PROXY_RESPONSE_TIMEOUT_SECS", "entfernt, nur Warnung"),
    ("PAPERRUST_PROXY_X_FORWARDED_FOR", "entfernt, nur Warnung"),
    ("PAPERRUST_REDIS_CONNECT_TIMEOUT", "entfernt, nur Warnung"),
    ("PAPERRUST_REDIS_RETRY_AFTER", "entfernt, nur Warnung"),
    ("PAPERRUST_ASN_TEST_DATABASE_URL", "Test"),
    // example names in the pl-system config.rs tests for the PAPERLESS_* startup mirror
    ("PAPERRUST_FUTURE_SETTING", "Test (pl-system config.rs)"),
    ("PAPERRUST_UPDATE_CHECK", "Test (pl-system config.rs)"),
    ("PAPERRUST_AUTOMATION_TEST_SERVER", "Test (pl-automation automation_db)"),
    ("PAPERRUST_CONSUMER_TEST_DATABASE_URL", "Test"),
    ("PAPERRUST_FOLDERS_TEST_DATABASE_URL", "Test (pl-api-v2 folders_api)"),
    ("PAPERRUST_INDEXER_DB_URL", "Test"),
    ("PAPERRUST_INDEXER_PARITY_CONTROL_INDEX", "Test"),
    ("PAPERRUST_INDEXER_PARITY_DIR", "Test"),
    ("PAPERRUST_INTEGRITY_TEST_DATABASE_URL", "Test"),
    ("PAPERRUST_QUEUE_RECOVERY_DATABASE_URL", "Test"),
    ("PAPERRUST_QUEUE_TEST_DATABASE_URL", "Test"),
    ("PAPERRUST_RELAY_TEST_DATABASE_URL", "Test"),
    ("PAPERRUST_ROLES_TEST_DATABASE_URL", "Test"),
    ("PAPERRUST_SYNC_TEST_DATABASE_URL", "Test"),
    ("PAPERRUST_SETTINGS_TEST_DATABASE_URL", "Test (dieses Crate)"),
    ("PAPERRUST_W15_DATABASE_URL", "Test"),
    ("PAPERRUST_TEST_DATABASE_URL", "Test (pl-queue maintenance)"),
    ("PAPERRUST_TEST_FONT", "Test (pl-ocr preprocess_e2e)"),
    ("PAPERRUST_SECRET_KEY_FILE", "Beispiel der `*_FILE`-Variante (pl-core `plan_file_env`, gilt für jede Einstellung)"),
    ("PAPERRUST_FIX1_SERVER", "Test (pl-documents-write lock_order_db)"),
    ("PAPERRUST_FIX2_SERVER", "Test (pl-documents-write concurrency_db)"),
    ("PAPERRUST_BULK_EQ_MEDIA", "Test (pl-consumer bulk_equal)"),
    ("PAPERRUST_SQLBIND_TEST_SERVER", "Test (pl-core sqlbind_db)"),
    ("PAPERRUST_BULK_EQ_SERVER", "Test (pl-consumer bulk_equal)"),
    ("PAPERRUST_CATCHUP_TEST_DB", "Test (pl-cli catchup_db)"),
    ("PAPERRUST_CATCHUP_TEST_PORT", "Test (pl-cli catchup_db)"),
    ("PAPERRUST_SCAN_TEST_ADMIN_URL", "Test (pl-scan api_db)"),
    ("PAPERRUST_GUARD_TEST_DATABASE_URL", "Test (pl-guard guard_db/backup_roundtrip)"),
    ("PAPERRUST_GUARD_TEST_TARGET_DATABASE_URL", "Test (pl-guard backup_roundtrip)"),
    ("PAPERRUST_DESKTOP_E2E_OPEN_LOG", "Test-Haken der Desktop-Hülle (app-2b-e2e.mjs, docs/DESKTOP.md)"),
    ("PAPERRUST_DESKTOP_LOGIN_TOKEN", "intern: Anmeldeschlüssel je Start, nur vom Desktop-Supervisor gesetzt (docs/DESKTOP.md §25)"),
    ("PAPERRUST_DESKTOP_OWNER", "intern: lokaler Eigentümer, nur vom Desktop-Supervisor gesetzt (docs/DESKTOP.md §25)"),
    ("PAPERRUST_RESTART_SUPERVISED", "intern: Aufseher meldet Exit-75-Neustart (§2.5 Nr. 7)"),
    ("PAPERRUST_SUPERVISOR_MANAGED_KEYS", "intern: vom Supervisor gesetzte Schlüssel (§2.6)"),
    ("PAPERRUST_SUPERVISOR_CONTROL", "intern: run-Ordner des Supervisors (§2.6, WP7)"),
    ("PAPERRUST_X", "Beispielname in Doku-Kommentaren/Tests"),
    ("PAPERRUST_ZZ_OV_FILE", "Test (pl-core live_overlay_precedence)"),
    ("PAPERRUST_ZZ_OV_GLOBAL", "Test (pl-core live_overlay_precedence)"),
    ("PAPERRUST_DB", "Präfix (starts_with) in pl-dbmigrate/src/switch.rs, kein eigener Name"),
];

fn covered(name: &str) -> bool {
    find(name).is_some() || EXCEPTIONS.iter().any(|(n, _)| *n == name)
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if matches!(name.as_str(), "target" | "node_modules" | ".git" | "gen") || name.starts_with("target") {
                continue;
            }
            rs_files(&p, out);
        } else if name.ends_with(".rs") {
            out.push(p);
        }
    }
}

/// Count from docs/SETTINGS.md §1.19 (`| A – GUI live | 143 | 45 |` …).
fn doc_counts() -> ([usize; 5], usize, usize) {
    let doc = std::fs::read_to_string(repo().join("docs/SETTINGS.md")).expect("docs/SETTINGS.md");
    let sec = doc.split("### 1.19 Zählung").nth(1).expect("§1.19").split("\n## ").next().unwrap();
    let mut c = [0usize; 5];
    let (mut total, mut ac) = (0, 0);
    for line in sec.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 4 {
            continue;
        }
        let num = |s: &str| s.trim_matches('*').parse::<usize>().ok();
        let label = cells[1];
        for (i, k) in ["A ", "B ", "C ", "D ", "X "].iter().enumerate() {
            if label.starts_with(k) {
                c[i] = num(cells[2]).expect("Zahl");
                if i == 0 {
                    ac = num(cells[3]).expect("AC-Zahl");
                }
            }
        }
        if label.contains("Summe System") {
            total = num(cells[2]).expect("Summe");
        }
    }
    (c, total, ac)
}

#[test]
fn inventory_complete() {
    // 1. Schema consistent in itself
    let mut keys = BTreeSet::new();
    let mut acs = BTreeSet::new();
    for d in SCHEMA {
        assert!(keys.insert(d.key), "doppelt: {}", d.key);
        if let Some(f) = d.ac_field {
            assert!(acs.insert(f), "AC-Feld doppelt: {f}");
            assert_eq!(d.class, Class::A, "{}", d.key);
        }
        if let Some(a) = d.alias_of {
            assert!(SCHEMA.iter().any(|x| x.key == a), "{} → {a}", d.key);
        }
        if let Some(def) = d.default {
            // Default values pass their own validation (path/empty values excluded)
            if !def.is_empty() && !matches!(d.ty, Ty::Path) {
                assert!(value::parse(d, def).is_ok(), "Standard von {} ungültig: {def:?} {:?}", d.key, value::parse(d, def));
            }
        }
        assert!(!d.help.is_empty(), "{}", d.key);
    }
    // 2. Count = documentation §1.19 (automatically generated, reconciled with the documentation)
    let (c, ac) = counts();
    let (dc, dtotal, dac) = doc_counts();
    eprintln!("Zählung: A {} · B {} · C {} · D {} · X {} (gesamt {}), AC {ac}", c[0], c[1], c[2], c[3], c[4], SCHEMA.len());
    assert_eq!(c, dc, "Zählung je Klasse weicht von docs/SETTINGS.md §1.19 ab");
    assert_eq!(SCHEMA.len(), dtotal);
    assert_eq!(ac, dac);
    assert_eq!(ac, 45);
    // 3. every own setting (pl_core::config::OWN_SETTINGS)
    let missing: Vec<&str> = OWN_SETTINGS.iter().map(|s| s.name).filter(|n| !covered(n)).collect();
    assert!(missing.is_empty(), "OWN_SETTINGS ohne Schema-Eintrag: {missing:?}");
    // 4. every PAPERLESS_* from upstream/src/paperless/settings
    let re_up = regex::Regex::new(r"PAPERLESS_[A-Z0-9_]*[A-Z0-9]").unwrap();
    let dir = repo().join("upstream/src/paperless/settings");
    let mut n_up = 0;
    let mut missing = BTreeSet::new();
    for e in std::fs::read_dir(&dir).expect("upstream/src/paperless/settings").flatten() {
        if e.path().extension().is_some_and(|x| x == "py") {
            let s = std::fs::read_to_string(e.path()).unwrap();
            for m in re_up.find_iter(&s) {
                n_up += 1;
                if !covered(m.as_str()) {
                    missing.insert(m.as_str().to_string());
                }
            }
        }
    }
    assert!(n_up > 150, "upstream-Einstellungen nicht gefunden ({n_up})");
    assert!(missing.is_empty(), "upstream-Einstellungen ohne Schema-Eintrag: {missing:?}");
    // 5. every PAPERRUST_* as a complete string literal in the Rust source
    let re_rs = regex::Regex::new(r#""(PAPERRUST_[A-Z0-9_]*[A-Z0-9])""#).unwrap();
    let mut files = Vec::new();
    rs_files(&repo().join("crates"), &mut files);
    rs_files(&repo().join("apps"), &mut files);
    let mut missing = BTreeSet::new();
    for f in &files {
        let Ok(s) = std::fs::read_to_string(f) else {
            continue;
        };
        for m in re_rs.captures_iter(&s) {
            if !covered(&m[1]) {
                missing.insert(format!("{} ({})", &m[1], f.strip_prefix(repo()).unwrap_or(f).display()));
            }
        }
    }
    assert!(missing.is_empty(), "PAPERRUST_* ohne Schema-Eintrag oder Ausnahme: {missing:#?}");
}

/// Env path (startup) and GUI path (JSON → canonical string → DB row → snapshot) yield
/// the same value for every type.
#[test]
fn parse_parity() {
    let samples: &[(&str, &str)] = &[
        ("PAPERLESS_EMAIL_USE_TLS", "yes"),
        ("PAPERLESS_EMAIL_PORT", "587"),
        ("PAPERLESS_CLASSIFIER_MATCH_THRESHOLD", "0.45"),
        ("PAPERLESS_EMAIL_HOST", "smtp.example.ch"),
        ("PAPERLESS_ALLOWED_HOSTS", "a.example, b.example,,c"),
        ("PAPERLESS_CONSUMER_IGNORE_PATTERNS", r#"["^\\._", "~$"]"#),
        ("PAPERLESS_ACCOUNT_EMAIL_VERIFICATION", "mandatory"),
        ("PAPERLESS_THUMBNAIL_FONT_NAME", "/fonts/x.ttf"),
        ("PAPERRUST_DOCS_URL", "https://docs.example/"),
        ("PAPERLESS_TRAIN_TASK_CRON", "15 3 * * mon"),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
        ("PAPERRUST_UI_DEFAULTS", r#"{"dark":true}"#),
        ("PAPERLESS_EMAIL_HOST_PASSWORD", "geh eim"),
        ("PAPERRUST_BRANDING", "OFF"),
    ];
    let mut kinds = BTreeSet::new();
    for (key, raw) in samples {
        let def = find(key).unwrap();
        kinds.insert(format!("{:?}", def.ty).split([' ', '{', '(']).next().unwrap().to_string());
        // Startup path
        let start = parse_from_env(def, &Env::from_pairs([(*key, *raw)])).unwrap().unwrap();
        // GUI path
        let json = value::to_json(def, raw);
        let canonical = value::from_json(def, &json).unwrap();
        let mut layers = Layers::new(Env::from_pairs([("UNRELATED", "1")]));
        layers.db.insert(def.key.into(), DbEntry { value: DbValue::Plain(canonical), rev: 1, updated_at: None, updated_by: None });
        let snap = Settings::build(&layers, 1, None);
        let gui = parse_from_env(def, snap.env()).unwrap().unwrap();
        assert_eq!(start, gui, "{key}");
    }
    for k in ["Bool", "OnOff", "Int", "Float", "Str", "List", "Json", "Choice", "Path", "Url", "Cron", "TimeZone"] {
        assert!(kinds.contains(k), "Typ {k} nicht abgedeckt: {kinds:?}");
    }
}
