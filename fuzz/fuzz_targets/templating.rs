//! Templates (pl-templating): file name/storage path templates (Jinja sandbox with fuel and
//! size limits), workflow titles, old {} syntax, path sanitizing.
#![no_main]
use libfuzzer_sys::fuzz_target;
include!("sancov_coff.rs");
use pl_templating::{DocumentContext, NamedRef, Options, OwnerRef, TagRef, WorkflowPlaceholders};

// Windows: libFuzzer does not always catch the abort after a panic (no crash-*-artifact).
// Therefore remember the current input and write it as an artifact in the panic hook itself.
static CURRENT: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());
static HOOK: std::sync::Once = std::sync::Once::new();

fn install_hook() {
    HOOK.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Known panic in minijinja caught in the server (Value::cmp, S-9): do not count as a crash,
            // otherwise the target stops at the same place again and again. pl-templating catches it via catch_unwind.
            if info.location().is_some_and(|l| l.file().contains("minijinja") && l.file().replace('\\', "/").ends_with("value/mod.rs")) {
                return;
            }
            if let Ok(cur) = CURRENT.try_lock() {
                let dir = std::env::var("FUZZ_PANIC_DIR").unwrap_or_else(|_| ".".into());
                let _ = std::fs::write(format!("{dir}/panic-templating-{}.txt", std::process::id()), &*cur);
            }
            prev(info);
        }));
    });
}

fuzz_target!(|data: &[u8]| {
    install_hook();
    if let Ok(mut c) = CURRENT.lock() {
        c.clear();
        c.extend_from_slice(data);
    }
    let Ok(t) = std::str::from_utf8(data) else { return };
    let opts = Options::default();
    let mut doc = DocumentContext::new(7, "Rechnung März", "application/pdf");
    doc.owner = Some(OwnerRef { id: 3, username: "alice".into() });
    doc.correspondent = Some(NamedRef { id: Some(1), name: "ACME/../x".into() });
    doc.tags = vec![TagRef { id: Some(1), name: "a".into() }, TagRef { id: Some(2), name: "b c".into() }];
    let _ = pl_templating::render_filename(t, &doc, &opts);
    let _ = pl_templating::validate_filepath_template_and_render(t, None, &opts);
    let _ = pl_templating::convert_format_str_to_template_format(t);
    let _ = pl_templating::clean_filepath(t);
    let _ = pl_templating::is_safe_relative_path(t);
    let _ = pl_templating::sanitize_filename(t, "_");
    let _ = pl_templating::validate_workflow_template(t);
    let p = WorkflowPlaceholders {
        correspondent_name: Some("ACME".into()),
        doc_type_name: None,
        owner_username: Some("alice".into()),
        local_added: chrono::DateTime::parse_from_rfc3339("2025-01-06T08:00:00+01:00").expect("const"),
        original_filename: "scan.pdf".into(),
        filename: "scan".into(),
        created: chrono::NaiveDate::from_ymd_opt(2025, 1, 6),
        doc_title: Some("T".into()),
        doc_url: None,
        doc_id: Some(7),
        custom_fields: Vec::new(),
    };
    let _ = pl_templating::render_workflow_title(t, &p);
});
