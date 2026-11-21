//! Report as JSON and text.

use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct Issue {
    pub pk: String,
    pub column: String,
    /// true = value cannot be adopted (abort); false = changed according to policy (reported)
    pub fatal: bool,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct TableReport {
    pub name: String,
    pub source_rows: i64,
    pub source_bytes: Option<i64>,
    pub copied_rows: Option<u64>,
    pub target_rows: Option<i64>,
    pub source_checksum: Option<String>,
    pub target_checksum: Option<String>,
    pub verified: Option<bool>,
    pub errors: u64,
    pub fixed: u64,
    pub issues: Vec<Issue>,
    pub diff_rows: Vec<String>,
}

impl TableReport {
    pub fn new(name: &str) -> Self {
        TableReport {
            name: name.to_string(),
            source_rows: 0,
            source_bytes: None,
            copied_rows: None,
            target_rows: None,
            source_checksum: None,
            target_checksum: None,
            verified: None,
            errors: 0,
            fixed: 0,
            issues: vec![],
            diff_rows: vec![],
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub tool_version: String,
    pub mode: String,
    pub source: String,
    pub target: Option<String>,
    pub source_dialect: String,
    pub source_version: String,
    pub target_version: String,
    pub schema_version: String,
    pub started: String,
    pub finished: String,
    pub duration_s: f64,
    /// migrated | verified | dry-run-ok | dry-run-problems | verify-failed | failed | aborted
    pub result: String,
    pub error: Option<String>,
    pub migrations_replaced_rows: usize,
    pub skipped_tables: Vec<String>,
    pub tables: Vec<TableReport>,
    pub sequences_checked: usize,
    pub constraints_checked: usize,
    pub indexes_checked: usize,
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
    pub nul_policy: String,
    pub utf8_policy: String,
    /// paperless-ngx migrations caught up while copying (older source, `catchup.rs`)
    pub catchup: Vec<String>,
}

impl Report {
    pub fn has_errors(&self) -> bool {
        self.tables.iter().any(|t| t.errors > 0)
    }

    pub fn ok(&self) -> bool {
        matches!(self.result.as_str(), "migrated" | "verified" | "dry-run-ok")
    }

    pub fn to_json(&self) -> Value {
        let tables: Vec<Value> = self
            .tables
            .iter()
            .map(|t| {
                json!({
                    "table": t.name,
                    "source_rows": t.source_rows,
                    "source_bytes_approx": t.source_bytes,
                    "copied_rows": t.copied_rows,
                    "target_rows": t.target_rows,
                    "source_checksum": t.source_checksum,
                    "target_checksum": t.target_checksum,
                    "verified": t.verified,
                    "conversion_errors": t.errors,
                    "values_changed_by_policy": t.fixed,
                    "issues": t.issues.iter().map(|i| json!({
                        "pk": i.pk, "column": i.column, "fatal": i.fatal, "message": i.message
                    })).collect::<Vec<_>>(),
                    "differing_rows": t.diff_rows,
                })
            })
            .collect();
        json!({
            "tool": "paperrust-migrate-db",
            "tool_version": self.tool_version,
            "mode": self.mode,
            "result": self.result,
            "ok": self.ok(),
            "error": self.error,
            "schema_version": self.schema_version,
            "catchup_migrations": self.catchup,
            "source": { "url": self.source, "dialect": self.source_dialect, "version": self.source_version },
            "target": { "url": self.target, "version": self.target_version },
            "started": self.started,
            "finished": self.finished,
            "duration_s": self.duration_s,
            "policies": { "nul": self.nul_policy, "invalid_utf8": self.utf8_policy },
            "migrations_replaced_rows": self.migrations_replaced_rows,
            "skipped_tables": self.skipped_tables,
            "totals": {
                "tables": self.tables.len(),
                "source_rows": self.tables.iter().map(|t| t.source_rows).sum::<i64>(),
                "target_rows": self.tables.iter().filter_map(|t| t.target_rows).sum::<i64>(),
                "conversion_errors": self.tables.iter().map(|t| t.errors).sum::<u64>(),
                "values_changed_by_policy": self.tables.iter().map(|t| t.fixed).sum::<u64>(),
                "sequences_checked": self.sequences_checked,
                "constraints_checked": self.constraints_checked,
                "indexes_checked": self.indexes_checked,
            },
            "problems": self.problems,
            "warnings": self.warnings,
            "tables": tables,
        })
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("paperrust-migrate-db {} – Modus {}\n", self.tool_version, self.mode));
        s.push_str(&format!("Quelle: {} ({} {})\n", self.source, self.source_dialect, self.source_version));
        if let Some(t) = &self.target {
            s.push_str(&format!("Ziel:   {} ({})\n", t, self.target_version));
        }
        s.push_str(&format!("Schema: paperless-ngx v{}\n", self.schema_version));
        if !self.catchup.is_empty() {
            s.push_str(&format!("Nachgeholt (ältere Quelle): {}\n", self.catchup.join(", ")));
        }
        s.push_str(&format!("Dauer:  {:.1} s\n\n", self.duration_s));
        s.push_str(&format!("{:<45} {:>10} {:>10} {:>8} {:>7} {:>7}\n", "Tabelle", "Quelle", "Ziel", "Prüfung", "Fehler", "Geänd."));
        for t in &self.tables {
            s.push_str(&format!(
                "{:<45} {:>10} {:>10} {:>8} {:>7} {:>7}\n",
                t.name,
                t.source_rows,
                t.target_rows.map(|r| r.to_string()).unwrap_or("-".into()),
                match t.verified {
                    Some(true) => "ok",
                    Some(false) => "FEHLER",
                    None => "-",
                },
                t.errors,
                t.fixed
            ));
        }
        let rows: i64 = self.tables.iter().map(|t| t.source_rows).sum();
        let bytes: i64 = self.tables.iter().filter_map(|t| t.source_bytes).sum();
        s.push_str(&format!("\nSumme: {} Tabellen, {rows} Zeilen", self.tables.len()));
        if bytes > 0 {
            s.push_str(&format!(", Quelle ca. {:.1} MiB", bytes as f64 / 1048576.0));
        }
        s.push('\n');
        if self.sequences_checked > 0 || self.constraints_checked > 0 {
            s.push_str(&format!(
                "Geprüft: {} Sequenzen, {} Constraints, {} Indizes\n",
                self.sequences_checked, self.constraints_checked, self.indexes_checked
            ));
        }
        for t in &self.tables {
            for i in &t.issues {
                s.push_str(&format!(
                    "{} {}.{} (pk={}): {}\n",
                    if i.fatal { "FEHLER " } else { "GEÄNDERT" },
                    t.name,
                    i.column,
                    i.pk,
                    i.message
                ));
            }
            if t.errors + t.fixed > t.issues.len() as u64 {
                s.push_str(&format!("  … {} weitere Befunde in {}\n", t.errors + t.fixed - t.issues.len() as u64, t.name));
            }
            for d in &t.diff_rows {
                s.push_str(&format!("ABWEICHUNG {}: {d}\n", t.name));
            }
        }
        for w in &self.warnings {
            s.push_str(&format!("Warnung: {w}\n"));
        }
        for p in &self.problems {
            s.push_str(&format!("Problem: {p}\n"));
        }
        if let Some(e) = &self.error {
            s.push_str(&format!("\nFehler: {e}\n"));
        }
        s.push_str(&format!(
            "\nErgebnis: {}\n",
            match self.result.as_str() {
                "migrated" => "UMZUG ERFOLGREICH – alle Prüfungen bestanden.",
                "verified" => "PRÜFUNG BESTANDEN – Ziel entspricht der Quelle.",
                "dry-run-ok" => "PROBELAUF OK – Umzug sollte gelingen.",
                "dry-run-problems" => "PROBELAUF MIT PROBLEMEN – siehe oben.",
                "verify-failed" => "PRÜFUNG FEHLGESCHLAGEN – Ziel NICHT verwenden.",
                "aborted" => "ABGEBROCHEN – Ziel zurückgerollt (leer), Quelle unverändert.",
                _ => "FEHLGESCHLAGEN – Ziel zurückgerollt (leer), Quelle unverändert.",
            }
        ));
        s
    }
}
