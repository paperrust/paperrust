//! Optional ASN number ranges with storage location (PaperRust extension, docs/contract/pl-labels.md
//! §Nummernkreise).
//!
//! * Default (switch off): one consecutive number sequence as before – nothing here has an effect.
//! * Switch "Multiple number ranges" on (`paperrust_asn_config.ranges_enabled`): ranges with name,
//!   abbreviation, storage location and number span (`paperrust_asn_range`, without overlaps). Printing
//!   suggests the next free number **within** the chosen range; when a document gets an ASN from
//!   a range, the storage-location hook sets the user field "Ablageort" (name configurable).
//!
//! The barcode content stays unchanged (`<prefix><number>`), scanning/compatibility are the same.

use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, Row};

#[cfg(test)]
use crate::LabelError;
use crate::{ASN_MAX_STORABLE, Result, invalid};

/// Migration (name, SQL) for `pl-cli migrate` (`paperrust_migrations`).
pub const MIGRATION: (&str, &str) = ("pl_labels.0002_paperrust_asn_ranges", include_str!("../migrations/0002_paperrust_asn_ranges.sql"));

/// Default name of the user field for the storage location.
pub const DEFAULT_LOCATION_FIELD: &str = "Ablageort";
/// Fixed advisory lock for changes to number ranges (overlap check serialised).
const LOCK_KEY: i64 = 7_052_041_511;

pub const MAX_NAME: usize = 64;
pub const MAX_CODE: usize = 8;
/// = `max_length` of a user field of type text (upstream `CustomFieldInstance.value_text`).
pub const MAX_LOCATION: usize = 128;
pub const MAX_NOTE: usize = 2000;

/// Settings (one row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RangeConfig {
    pub ranges_enabled: bool,
    pub location_field_name: String,
}

impl Default for RangeConfig {
    fn default() -> Self {
        RangeConfig { ranges_enabled: false, location_field_name: DEFAULT_LOCATION_FIELD.into() }
    }
}

/// Change of the settings (`PUT /asn/config`): missing fields stay.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigUpdate {
    #[serde(default)]
    pub ranges_enabled: Option<bool>,
    #[serde(default)]
    pub location_field_name: Option<String>,
}

impl ConfigUpdate {
    /// Checks and applies the change to `cur`.
    pub fn apply(&self, cur: &RangeConfig) -> Result<RangeConfig> {
        let mut c = cur.clone();
        if let Some(e) = self.ranges_enabled {
            c.ranges_enabled = e;
        }
        if let Some(n) = &self.location_field_name {
            let n = n.trim();
            if n.is_empty() || n.chars().count() > 128 {
                return invalid("location_field_name", "The field name must have 1–128 characters.");
            }
            c.location_field_name = n.to_string();
        }
        Ok(c)
    }
}

/// A number range (stored).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AsnRange {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub location: String,
    pub first_asn: i64,
    pub last_asn: i64,
    pub note: String,
}

/// Input for create/replace (`POST`/`PUT /asn/ranges`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeInput {
    pub name: String,
    pub code: String,
    #[serde(default)]
    pub location: String,
    pub first_asn: i64,
    pub last_asn: i64,
    #[serde(default)]
    pub note: String,
}

/// Partial change (`PATCH /asn/ranges/{id}`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangePatch {
    pub name: Option<String>,
    pub code: Option<String>,
    pub location: Option<String>,
    pub first_asn: Option<i64>,
    pub last_asn: Option<i64>,
    pub note: Option<String>,
}

impl RangePatch {
    pub fn merge(self, cur: &AsnRange) -> RangeInput {
        RangeInput {
            name: self.name.unwrap_or_else(|| cur.name.clone()),
            code: self.code.unwrap_or_else(|| cur.code.clone()),
            location: self.location.unwrap_or_else(|| cur.location.clone()),
            first_asn: self.first_asn.unwrap_or(cur.first_asn),
            last_asn: self.last_asn.unwrap_or(cur.last_asn),
            note: self.note.unwrap_or_else(|| cur.note.clone()),
        }
    }
}

/// Checks a number range against the others (`others`, without `self_id`) and returns it cleaned up
/// (surrounding whitespace removed).
pub fn validate(input: &RangeInput, others: &[AsnRange], self_id: Option<i64>) -> Result<RangeInput> {
    let name = input.name.trim().to_string();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return invalid("name", format!("The name must have 1–{MAX_NAME} characters."));
    }
    let code = input.code.trim().to_string();
    if code.is_empty() || code.len() > MAX_CODE {
        return invalid("code", format!("The short code must have 1–{MAX_CODE} characters."));
    }
    if !code.chars().all(|c| (' '..='~').contains(&c)) {
        return invalid("code", "The short code may only contain printable ASCII characters (A–Z, 0–9, -, …).");
    }
    let location = input.location.trim().to_string();
    if location.chars().count() > MAX_LOCATION {
        return invalid("location", format!("The location may have at most {MAX_LOCATION} characters."));
    }
    let note = input.note.trim().to_string();
    if note.chars().count() > MAX_NOTE {
        return invalid("note", format!("The note may have at most {MAX_NOTE} characters."));
    }
    let (first, last) = (input.first_asn, input.last_asn);
    if first < 0 {
        return invalid("first_asn", "Must be 0 or greater.");
    }
    if last < first {
        return invalid("last_asn", "The last number must not be smaller than the first number.");
    }
    if last as u64 > ASN_MAX_STORABLE {
        return invalid("last_asn", format!("Must not exceed the largest storable ASN ({ASN_MAX_STORABLE})."));
    }
    for o in others.iter().filter(|o| Some(o.id) != self_id) {
        if o.code.eq_ignore_ascii_case(&code) {
            return invalid("code", format!("The short code {:?} is already used by the range {:?}.", o.code, o.name));
        }
        if o.first_asn <= last && o.last_asn >= first {
            return invalid("first_asn", format!("The numbers overlap with the range {:?} ({}–{}).", o.name, o.first_asn, o.last_asn));
        }
    }
    Ok(RangeInput { name, code, location, first_asn: first, last_asn: last, note })
}

/// Next free number in the range `first..=last`: after the largest assigned (`max_document`,
/// incl. trash) or printed (`max_printed`) number **within** the range – like
/// `store::next_asn`, only restricted to the range. `None` = range exhausted.
pub fn next_in_range(first: i64, last: i64, max_document: Option<i64>, max_printed: Option<i64>) -> Option<i64> {
    let inside = |v: Option<i64>| v.filter(|n| (first..=last).contains(n));
    let used = inside(max_document).max(inside(max_printed));
    let next = used.map_or(first, |u| u + 1);
    (next <= last).then_some(next)
}

/// Text below the number on the label: `<abbreviation> · <storage location>` (or only the abbreviation).
pub fn caption(r: &AsnRange) -> String {
    if r.location.is_empty() { r.code.clone() } else { format!("{} · {}", r.code, r.location) }
}

/// Number range with occupancy (for list and printing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RangeStatus {
    #[serde(flatten)]
    pub range: AsnRange,
    pub created: chrono::DateTime<chrono::Utc>,
    pub modified: chrono::DateTime<chrono::Utc>,
    /// Documents (incl. trash) with a number from the range.
    pub used: i64,
    /// Suggestion for the next print, `null` = exhausted.
    pub next_asn: Option<i64>,
    /// Still free numbers from `next_asn` to the end of the range.
    pub remaining: i64,
}

const SELECT: &str = "SELECT r.id, r.name, r.code, r.location, r.first_asn, r.last_asn, r.note, r.created, r.modified,
        (SELECT count(*) FROM documents_document d WHERE d.archive_serial_number BETWEEN r.first_asn AND r.last_asn) AS used,
        (SELECT max(d.archive_serial_number)::bigint FROM documents_document d WHERE d.archive_serial_number BETWEEN r.first_asn AND r.last_asn) AS max_doc,
        (SELECT max(LEAST(p.last_asn, r.last_asn)) FROM paperrust_asn_print p WHERE p.first_asn <= r.last_asn AND p.last_asn >= r.first_asn) AS max_printed
   FROM paperrust_asn_range r";

fn status_of(r: &sqlx::postgres::PgRow) -> Result<RangeStatus> {
    let range = AsnRange {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        code: r.try_get("code")?,
        location: r.try_get("location")?,
        first_asn: r.try_get("first_asn")?,
        last_asn: r.try_get("last_asn")?,
        note: r.try_get("note")?,
    };
    let next = next_in_range(range.first_asn, range.last_asn, r.try_get("max_doc")?, r.try_get("max_printed")?);
    let remaining = next.map_or(0, |n| range.last_asn - n + 1);
    Ok(RangeStatus {
        created: r.try_get("created")?,
        modified: r.try_get("modified")?,
        used: r.try_get("used")?,
        next_asn: next,
        remaining,
        range,
    })
}

/// Are the tables there? (Consumer/PATCH create nothing, they only read.)
async fn tables_exist(db: &mut PgConnection) -> Result<bool> {
    let ok: bool =
        sqlx::query_scalar("SELECT to_regclass('paperrust_asn_range') IS NOT NULL AND to_regclass('paperrust_asn_config') IS NOT NULL")
            .fetch_one(&mut *db)
            .await?;
    Ok(ok)
}

pub async fn get_config(db: &mut PgConnection) -> Result<RangeConfig> {
    let row: Option<(bool, String)> = sqlx::query_as("SELECT ranges_enabled, location_field_name FROM paperrust_asn_config WHERE id = 1")
        .fetch_optional(&mut *db)
        .await?;
    Ok(row.map(|(ranges_enabled, location_field_name)| RangeConfig { ranges_enabled, location_field_name }).unwrap_or_default())
}

pub async fn set_config(db: &mut PgConnection, c: &RangeConfig) -> Result<()> {
    sqlx::query(
        "INSERT INTO paperrust_asn_config (id, ranges_enabled, location_field_name, modified) VALUES (1, $1, $2, now())
         ON CONFLICT (id) DO UPDATE SET ranges_enabled = EXCLUDED.ranges_enabled, location_field_name = EXCLUDED.location_field_name, modified = now()",
    )
    .bind(c.ranges_enabled)
    .bind(&c.location_field_name)
    .execute(&mut *db)
    .await?;
    Ok(())
}

pub async fn list(db: &mut PgConnection) -> Result<Vec<RangeStatus>> {
    let rows = sqlx::query(&format!("{SELECT} ORDER BY r.first_asn, r.id")).fetch_all(&mut *db).await?;
    rows.iter().map(status_of).collect()
}

pub async fn get(db: &mut PgConnection, id: i64) -> Result<Option<RangeStatus>> {
    let row = sqlx::query(&format!("{SELECT} WHERE r.id = $1")).bind(id).fetch_optional(&mut *db).await?;
    row.as_ref().map(status_of).transpose()
}

async fn all_plain(db: &mut PgConnection) -> Result<Vec<AsnRange>> {
    Ok(list(db).await?.into_iter().map(|s| s.range).collect())
}

/// Creates (`id = None`) or replaces (`id = Some`) – check and write in one transaction under
/// the advisory lock. `Ok(None)` = `id` does not exist.
pub async fn save(db: &mut PgConnection, id: Option<i64>, input: &RangeInput) -> Result<Option<RangeStatus>> {
    let mut tx = sqlx::Connection::begin(&mut *db).await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(LOCK_KEY).execute(&mut *tx).await?;
    let others = all_plain(&mut tx).await?;
    if let Some(id) = id
        && !others.iter().any(|o| o.id == id)
    {
        tx.rollback().await?;
        return Ok(None);
    }
    let v = validate(input, &others, id)?;
    let new_id: i64 = match id {
        None => {
            sqlx::query_scalar(
                "INSERT INTO paperrust_asn_range (name, code, location, first_asn, last_asn, note) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
            )
            .bind(&v.name)
            .bind(&v.code)
            .bind(&v.location)
            .bind(v.first_asn)
            .bind(v.last_asn)
            .bind(&v.note)
            .fetch_one(&mut *tx)
            .await?
        }
        Some(id) => {
            sqlx::query(
                "UPDATE paperrust_asn_range SET name = $2, code = $3, location = $4, first_asn = $5, last_asn = $6, note = $7, modified = now() WHERE id = $1",
            )
            .bind(id)
            .bind(&v.name)
            .bind(&v.code)
            .bind(&v.location)
            .bind(v.first_asn)
            .bind(v.last_asn)
            .bind(&v.note)
            .execute(&mut *tx)
            .await?;
            id
        }
    };
    let s = get(&mut tx, new_id).await?;
    tx.commit().await?;
    Ok(s)
}

/// Deletes a number range (documents and printed ranges stay unchanged). `true` = deleted.
pub async fn delete(db: &mut PgConnection, id: i64) -> Result<bool> {
    let n = sqlx::query("DELETE FROM paperrust_asn_range WHERE id = $1").bind(id).execute(&mut *db).await?.rows_affected();
    Ok(n > 0)
}

// ---------------------------------------------------------------------------------------------
// Storage-location hook (consumer + document PATCH)
// ---------------------------------------------------------------------------------------------

/// What the hook is to set for an ASN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationTarget {
    pub field_name: String,
    /// Storage location of the range the ASN lies in (never empty).
    pub location: String,
    /// Storage locations of all ranges: an existing field value that matches one of them comes from the
    /// hook and may be replaced when moving to another range.
    pub known_locations: Vec<String>,
}

/// Target for `asn`, or `None` (switch off, tables missing, ASN in no range, range without
/// storage location).
pub async fn location_target(db: &mut PgConnection, asn: i64) -> Result<Option<LocationTarget>> {
    if !tables_exist(db).await? {
        return Ok(None);
    }
    let cfg = get_config(db).await?;
    if !cfg.ranges_enabled {
        return Ok(None);
    }
    let rows: Vec<(i64, i64, String)> =
        sqlx::query_as("SELECT first_asn, last_asn, location FROM paperrust_asn_range").fetch_all(&mut *db).await?;
    let Some(location) = rows.iter().find(|(f, l, _)| (*f..=*l).contains(&asn)).map(|r| r.2.clone()).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    let known_locations = rows.into_iter().map(|r| r.2).filter(|l| !l.is_empty()).collect();
    Ok(Some(LocationTarget { field_name: cfg.location_field_name, location, known_locations }))
}

/// Should the hook set the field value? Only if the field is missing/empty or carries the storage location of a
/// range (i.e. comes from the hook) – a different value set by hand always stays.
pub fn should_write(existing: Option<&str>, t: &LocationTarget) -> bool {
    match existing.map(str::trim) {
        None | Some("") => true,
        Some(v) if v == t.location => false,
        Some(v) => t.known_locations.iter().any(|k| k == v),
    }
}

/// Id of the user field `name` of type text; creates it if missing. `None` if the name exists
/// with another type (then the hook sets nothing).
pub async fn ensure_location_field(db: &mut PgConnection, name: &str) -> Result<Option<i64>> {
    let row: Option<(i32, String)> = sqlx::query_as("SELECT id, data_type FROM documents_customfield WHERE name = $1 ORDER BY id LIMIT 1")
        .bind(name)
        .fetch_optional(&mut *db)
        .await?;
    if let Some((id, ty)) = row {
        return Ok((ty == "string").then_some(i64::from(id)));
    }
    let id: i32 =
        sqlx::query_scalar("INSERT INTO documents_customfield (created, name, data_type) VALUES (now(), $1, 'string') RETURNING id")
            .bind(name)
            .fetch_one(&mut *db)
            .await?;
    Ok(Some(i64::from(id)))
}

/// Current (non-deleted) value of the field on the document: `None` = no instance,
/// `Some((id, value))` otherwise.
pub async fn current_value(db: &mut PgConnection, doc_id: i64, field_id: i64) -> Result<Option<(i64, Option<String>)>> {
    let row: Option<(i32, Option<String>)> = sqlx::query_as(
        "SELECT id, value_text FROM documents_customfieldinstance WHERE document_id = $1 AND field_id = $2 AND deleted_at IS NULL ORDER BY id LIMIT 1",
    )
    .bind(doc_id as i32)
    .bind(field_id as i32)
    .fetch_optional(&mut *db)
    .await?;
    Ok(row.map(|(id, v)| (i64::from(id), v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: i64, code: &str, first: i64, last: i64, loc: &str) -> AsnRange {
        AsnRange {
            id,
            name: format!("R{id}"),
            code: code.into(),
            location: loc.into(),
            first_asn: first,
            last_asn: last,
            note: String::new(),
        }
    }

    fn input(code: &str, first: i64, last: i64) -> RangeInput {
        RangeInput {
            name: " Zürich ".into(),
            code: code.into(),
            location: " Archiv Zürich, Regal 3 ".into(),
            first_asn: first,
            last_asn: last,
            note: String::new(),
        }
    }

    fn field(res: Result<RangeInput>) -> String {
        match res {
            Err(LabelError::Invalid(f, _)) => f,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn validate_trims_and_accepts() {
        let v = validate(&input("ZH", 1000, 1999), &[r(1, "BE", 1, 999, "Bern")], None).unwrap();
        assert_eq!((v.name.as_str(), v.code.as_str(), v.location.as_str()), ("Zürich", "ZH", "Archiv Zürich, Regal 3"));
        // directly adjacent is not an overlap
        validate(&input("ZH", 1000, 1000), &[r(1, "BE", 1, 999, ""), r(2, "GE", 1001, 2000, "")], None).unwrap();
    }

    #[test]
    fn validate_rejects() {
        let others = [r(1, "BE", 100, 199, "Bern"), r(2, "GE", 500, 599, "Genf")];
        assert_eq!(field(validate(&input("ZH", 150, 300), &others, None)), "first_asn");
        assert_eq!(field(validate(&input("ZH", 0, 100), &others, None)), "first_asn");
        assert_eq!(field(validate(&input("ZH", 0, 1000), &others, None)), "first_asn", "umschließt einen Bereich");
        assert_eq!(field(validate(&input("ZH", 599, 700), &others, None)), "first_asn");
        assert_eq!(field(validate(&input("be", 1000, 1100), &others, None)), "code", "Kürzel ohne Groß/klein");
        assert_eq!(field(validate(&input("", 1000, 1100), &others, None)), "code");
        assert_eq!(field(validate(&input("ABCDEFGHI", 1000, 1100), &others, None)), "code");
        assert_eq!(field(validate(&input("ZÜ", 1000, 1100), &others, None)), "code");
        assert_eq!(field(validate(&input("ZH", -1, 10), &[], None)), "first_asn");
        assert_eq!(field(validate(&input("ZH", 10, 9), &[], None)), "last_asn");
        assert_eq!(field(validate(&input("ZH", 1, ASN_MAX_STORABLE as i64 + 1), &[], None)), "last_asn");
        assert_eq!(field(validate(&RangeInput { name: "  ".into(), ..input("ZH", 1, 2) }, &[], None)), "name");
        assert_eq!(field(validate(&RangeInput { location: "x".repeat(129), ..input("ZH", 1, 2) }, &[], None)), "location");
        // own range does not count when changing
        validate(&input("BE", 100, 250), &others, Some(1)).unwrap();
        // at most 8 characters, printable ASCII with hyphen
        validate(&input("ZH-ARCH1", 1000, 1100), &others, None).unwrap();
    }

    #[test]
    fn next_number_inside_range() {
        assert_eq!(next_in_range(1000, 1999, None, None), Some(1000));
        // numbers outside the range do not count
        assert_eq!(next_in_range(1000, 1999, Some(5000), Some(12)), Some(1000));
        assert_eq!(next_in_range(1000, 1999, Some(1041), None), Some(1042));
        assert_eq!(next_in_range(1000, 1999, Some(1041), Some(1080)), Some(1081));
        assert_eq!(next_in_range(1000, 1999, Some(1500), Some(1080)), Some(1501));
        assert_eq!(next_in_range(1000, 1999, Some(1999), None), None, "erschöpft");
        assert_eq!(next_in_range(1000, 1999, None, Some(1999)), None);
        assert_eq!(next_in_range(5, 5, None, None), Some(5));
    }

    #[test]
    fn caption_text() {
        assert_eq!(caption(&r(1, "ZH", 1, 2, "Archiv Zürich, Regal 3")), "ZH · Archiv Zürich, Regal 3");
        assert_eq!(caption(&r(1, "ZH", 1, 2, "")), "ZH");
    }

    #[test]
    fn hook_never_overwrites_manual_value() {
        let t = LocationTarget {
            field_name: "Ablageort".into(),
            location: "Zürich".into(),
            known_locations: vec!["Zürich".into(), "Bern".into()],
        };
        assert!(should_write(None, &t));
        assert!(should_write(Some(""), &t));
        assert!(should_write(Some("  "), &t));
        assert!(!should_write(Some("Zürich"), &t), "schon richtig");
        assert!(should_write(Some("Bern"), &t), "vom Hook gesetzt (anderer Bereich) → ersetzen");
        assert!(!should_write(Some("Keller links"), &t), "von Hand gesetzt → bleibt");
    }

    #[test]
    fn config_update() {
        let c = RangeConfig::default();
        assert!(!c.ranges_enabled);
        assert_eq!(c.location_field_name, "Ablageort");
        let n = ConfigUpdate { ranges_enabled: Some(true), location_field_name: Some(" Standort ".into()) }.apply(&c).unwrap();
        assert_eq!(n, RangeConfig { ranges_enabled: true, location_field_name: "Standort".into() });
        assert!(ConfigUpdate { location_field_name: Some(" ".into()), ..Default::default() }.apply(&c).is_err());
        assert_eq!(ConfigUpdate::default().apply(&n).unwrap(), n);
    }

    #[test]
    fn patch_merges() {
        let cur = r(3, "ZH", 1, 9, "A");
        let m = RangePatch { last_asn: Some(20), ..Default::default() }.merge(&cur);
        assert_eq!((m.code.as_str(), m.first_asn, m.last_asn, m.location.as_str()), ("ZH", 1, 20, "A"));
    }
}
