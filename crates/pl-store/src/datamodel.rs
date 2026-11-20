//! Own data model (L6, docs/DATAMODEL-V2.md): register of the conversion stages.
//!
//! Unlike the other `paperrust_*` migrations these stages are **not expand-only**: they
//! create own tables, copy the data (same ids), move the Django table as an empty
//! structure template to `ngx_legacy` and provide, under the old name, a compatibility view
//! with exactly the v3.4.2 columns (DATAMODEL-V2 §2.1). Therefore every stage has
//!
//! * a name in `paperrust_migrations` (applied by `pl_cli::migrate`, after all other
//!   own migrations; **no** entry in `django_migrations` – the v3.4.2 state remains the
//!   detection for the switch),
//! * a rollback script (`down`) that restores the Django state from the native tables
//!   (`paperrust manage datamodel rollback --to <n>`),
//! * the list of views whose columns must match v3.4.2 exactly
//!   (`paperrust manage datamodel check`).
//!
//! The order is fixed; stages build on each other and are rolled back only in reverse
//! order.

/// A conversion stage.
#[derive(Clone, Copy, Debug)]
pub struct Stage {
    /// Stage number from DATAMODEL-V2 §2.2 (1 = slim document, …).
    pub n: u8,
    /// Name in `paperrust_migrations`.
    pub name: &'static str,
    /// Short description (output of `datamodel status`).
    pub title: &'static str,
    /// Conversion (idempotent: checks itself whether it has already run).
    pub up: &'static str,
    /// Rollback to the state before the stage (idempotent).
    pub down: &'static str,
    /// Django tables that are compatibility views after the stage.
    pub views: &'static [&'static str],
}

/// Stage 1 "slim document" (DATAMODEL-V2 §1.1–1.3): `paperrust_document`,
/// `paperrust_document_content`, `paperrust_document_file`; view `documents_document`.
pub const DM1_DOCUMENT: Stage = Stage {
    n: 1,
    name: "pl_store.dm1_paperrust_document",
    title: "Dokument schlank (Text und Dateien ausgelagert)",
    up: include_str!("../migrations/dm1_paperrust_document.sql"),
    down: include_str!("../migrations/dm1_paperrust_document.down.sql"),
    views: &["documents_document"],
};

/// Stage 2 "tags + counters" (DATAMODEL-V2 §1.4/§1.5): `paperrust_document_tag`, counters
/// `paperrust_label_count` (+ `_delta`, view `_live`); view `documents_document_tags`.
pub const DM2_TAG_COUNT: Stage = Stage {
    n: 2,
    name: "pl_store.dm2_paperrust_tag_count",
    title: "Tag-Zuordnung ohne Ersatzschlüssel, Zähler je Label",
    up: concat!(include_str!("../migrations/dm_label_count_fns.sql"), include_str!("../migrations/dm2_paperrust_tag_count.sql")),
    down: include_str!("../migrations/dm2_paperrust_tag_count.down.sql"),
    views: &["documents_document_tags"],
};

/// Stage 3 "typed field values" (DATAMODEL-V2 §1.7): `paperrust_field_value`, counters kind 6;
/// view `documents_customfieldinstance`.
pub const DM3_FIELD_VALUE: Stage = Stage {
    n: 3,
    name: "pl_store.dm3_paperrust_field_value",
    title: "Custom-Field-Werte typisiert",
    up: concat!(include_str!("../migrations/dm_label_count_fns.sql"), include_str!("../migrations/dm3_paperrust_field_value.sql")),
    down: concat!(include_str!("../migrations/dm_label_count_fns.sql"), include_str!("../migrations/dm3_paperrust_field_value.down.sql")),
    views: &["documents_customfieldinstance"],
};

/// Stage 4 "permissions" (DATAMODEL-V2 §1.8): `paperrust_object_grant` (bitmask, truth),
/// `paperrust_grant_raw`; views `guardian_*objectpermission` and `paperrust_grant`.
pub const DM4_OBJECT_GRANT: Stage = Stage {
    n: 4,
    name: "pl_store.dm4_paperrust_object_grant",
    title: "Freigaben als Bitmaske je Objekt und Subjekt",
    up: include_str!("../migrations/dm4_paperrust_object_grant.sql"),
    down: include_str!("../migrations/dm4_paperrust_object_grant.down.sql"),
    views: &["guardian_userobjectpermission", "guardian_groupobjectpermission"],
};

/// Stage 5 "labels + sync" (DATAMODEL-V2 §1.9/§1.10): label tables `paperrust_tag`,
/// `_correspondent`, `_document_type`, `_storage_path`, `_custom_field` with `uuid`/`rev`; `uuid`/`rev`
/// in `paperrust_document`; `paperrust_sync_object` becomes a read view (kinds 7–9 in
/// `paperrust_sync_ident`, tombstones in `paperrust_sync_tombstone`). Before it the state-dependent
/// sync functions from pl_sync 0003.
pub const DM5_LABEL_SYNC: Stage = Stage {
    n: 5,
    name: "pl_store.dm5_paperrust_label_sync",
    title: "Labels mit uuid/rev, Sync-Identität in der Zeile",
    up: concat!(
        include_str!("../../pl-sync/migrations/0003_paperrust_sync_native.sql"),
        // recreate counter triggers: rev changes of the document row must cost them nothing
        include_str!("../migrations/dm_label_count_fns.sql"),
        include_str!("../migrations/dm5_paperrust_label_sync.sql")
    ),
    down: include_str!("../migrations/dm5_paperrust_label_sync.down.sql"),
    views: &[
        "documents_tag",
        "documents_correspondent",
        "documents_documenttype",
        "documents_storagepath",
        "documents_customfield",
    ],
};

/// All stages in application order.
pub const STAGES: &[Stage] = &[DM1_DOCUMENT, DM2_TAG_COUNT, DM3_FIELD_VALUE, DM4_OBJECT_GRANT, DM5_LABEL_SYNC];

/// Native document table as soon as stage 1 is applied, otherwise the Django table. For the
/// few places that do not work via the view (`SELECT … FOR UPDATE` fails on the view's
/// outer joins).
pub async fn document_table(conn: &mut sqlx::PgConnection) -> Result<&'static str, sqlx::Error> {
    let native: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace           WHERE n.nspname = current_schema() AND c.relname = 'paperrust_document' AND c.relkind = 'r')",
    )
    .fetch_one(conn)
    .await?;
    Ok(if native { "paperrust_document" } else { "documents_document" })
}

/// For `pl_cli::migrate::paperrust_migrations` (name, SQL), after all other own migrations.
pub fn migrations() -> Vec<(&'static str, &'static str)> {
    STAGES.iter().map(|s| (s.name, s.up)).collect()
}

/// Is `name` a conversion stage (back up before applying)?
pub fn is_stage(name: &str) -> bool {
    STAGES.iter().any(|s| s.name == name)
}

/// Stage by number.
pub fn stage(n: u8) -> Option<&'static Stage> {
    STAGES.iter().find(|s| s.n == n)
}

/// All views that must have v3.4.2 columns after the given (applied) stages.
pub fn compat_views<'a>(applied: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let applied: Vec<&str> = applied.into_iter().collect();
    STAGES.iter().filter(|s| applied.contains(&s.name)).flat_map(|s| s.views.iter().copied()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_are_ordered_unique_and_reversible() {
        let mut last = 0u8;
        let mut names = std::collections::BTreeSet::new();
        for s in STAGES {
            assert!(s.n > last, "Stufen aufsteigend: {}", s.name);
            last = s.n;
            assert!(names.insert(s.name), "doppelter Name {}", s.name);
            assert!(s.name.starts_with("pl_store.dm"), "Namensschema: {}", s.name);
            assert!(!s.down.trim().is_empty(), "Rückbau fehlt: {}", s.name);
            // conversion and rollback run in the transaction of the migration run (no own BEGIN)
            for sql in [s.up, s.down] {
                assert!(!sql.lines().any(|l| l.trim().eq_ignore_ascii_case("BEGIN;")), "{}", s.name);
            }
        }
        assert_eq!(migrations().len(), STAGES.len());
    }

    /// `paperrust_native_table` is in pl_sync 0001 (fresh install, `sync enable`) and in
    /// stage 1 (existing installations) – both versions must be identical.
    #[test]
    fn native_table_mapping_is_identical() {
        fn body(sql: &str) -> &str {
            let a = sql.find("CREATE OR REPLACE FUNCTION paperrust_native_table").expect("Funktion fehlt");
            let rest = &sql[a..];
            let b = rest.find("$fn$;").expect("Ende fehlt");
            &rest[..b]
        }
        let sync = include_str!("../../pl-sync/migrations/0001_paperrust_sync.sql");
        for s in STAGES.iter().filter(|s| s.up.contains("FUNCTION paperrust_native_table")) {
            assert_eq!(body(sync), body(s.up), "{}", s.name);
        }
        // every view of a stage has a mapping
        // guardian views (stage 4) need no mapping: capture there runs via the
        // mode 'grant' on paperrust_object_grant
        for s in STAGES {
            for v in s.views.iter().filter(|v| !v.starts_with("guardian_")) {
                assert!(body(sync).contains(&format!("('{v}',")), "{v} fehlt in paperrust_native_table");
            }
        }
    }

    /// The sync trigger installation is in pl_sync 0001 and (for existing installations) in
    /// stage 4 – same version.
    #[test]
    fn sync_install_function_is_identical() {
        fn body(sql: &str) -> &str {
            let a = sql.find("CREATE OR REPLACE FUNCTION paperrust_sync_install_triggers()").expect("Funktion fehlt");
            let rest = &sql[a..];
            &rest[..rest.find("$fn$;").expect("Ende fehlt")]
        }
        let sync = include_str!("../../pl-sync/migrations/0001_paperrust_sync.sql");
        assert_eq!(body(sync), body(DM4_OBJECT_GRANT.up));
    }

    #[test]
    fn view_triggers_cover_every_column() {
        // The view triggers write every v3.4.2 column (except content_length, which is computed).
        let cols = [
            "deleted_at", "restored_at", "transaction_id", "title", "content", "mime_type", "checksum", "archive_checksum",
            "page_count", "created", "modified", "added", "filename", "archive_filename", "original_filename",
            "archive_serial_number", "correspondent_id", "document_type_id", "storage_path_id", "owner_id",
            "root_document_id", "version_label", "version_index",
        ];
        let up = DM1_DOCUMENT.up;
        for f in ["paperrust_document_view_ins", "paperrust_document_view_upd"] {
            let a = up.find(&format!("FUNCTION {f}()")).expect("Trigger-Funktion");
            let body = &up[a..a + up[a..].find("$fn$;").expect("Ende")];
            for c in cols {
                assert!(body.contains(&format!("NEW.{c}")), "{f} schreibt {c} nicht");
            }
        }
    }
}
