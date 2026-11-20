// Scheduled workflow triggers (Celery beat "Check and run scheduled workflows", hourly).
//
// upstream: src/documents/tasks.py@v3.4.2 (check_scheduled_workflows),
//           src/documents/matching.py@v3.4.2 (prefilter_documents_by_workflowtrigger →
//           pl_matching::workflow::prefilter_matches), src/documents/models.py@v3.4.2
//           (WorkflowTrigger.ScheduleDateField, WorkflowRun, Document.Meta.ordering = -created),
//           django.db.models.DateField.to_python (aware datetime → date in TIME_ZONE).

use std::collections::HashMap;

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use pl_matching::workflow::{ExistingDocument, ObjRef, TRIGGER_SCHEDULED, WorkflowTrigger, prefilter_matches};
use sqlx::{PgConnection, Row};

use crate::consumer::Consumer;
use crate::db;
use crate::docops::DocInstance;
use crate::workflows::{self, WorkflowDef};

/// `WorkflowTrigger.ScheduleDateField`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleDateField {
    Added,
    Created,
    Modified,
    CustomField,
}

impl ScheduleDateField {
    pub fn parse(v: &str) -> Option<Self> {
        Some(match v {
            "added" => Self::Added,
            "created" => Self::Created,
            "modified" => Self::Modified,
            "custom_field" => Self::CustomField,
            _ => return None,
        })
    }
}

/// Schedule fields of a trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub trigger_id: i64,
    pub offset_days: i64,
    pub is_recurring: bool,
    pub recurring_interval_days: i64,
    /// `None` = unknown value (upstream: no `case` matches → `Document.objects.none()`).
    pub date_field: Option<ScheduleDateField>,
    pub custom_field_id: Option<i64>,
}

/// `now - timedelta(days=offset)`.
pub fn threshold(now: DateTime<Utc>, offset_days: i64) -> DateTime<Utc> {
    now - Duration::days(offset_days)
}

/// Compare a `DateField` with an aware datetime: Django converts it to `TIME_ZONE` and takes
/// the date (`DateField.to_python`).
pub fn local_date(t: DateTime<Utc>, tz: Tz) -> NaiveDate {
    t.with_timezone(&tz).date_naive()
}

/// `timezone.make_aware(datetime.combine(value_date, time.min)) + offset <= now`.
/// Ambiguous/missing local time (daylight saving) like zoneinfo: earlier variant or `fold=0`.
pub fn custom_date_due(value: NaiveDate, offset_days: i64, now: DateTime<Utc>, tz: Tz) -> bool {
    let naive = value.and_time(NaiveTime::MIN);
    let aware = match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) => t.with_timezone(&Utc),
        chrono::LocalResult::Ambiguous(a, _) => a.with_timezone(&Utc),
        // Gap: zoneinfo takes the offset before the gap (fold=0)
        chrono::LocalResult::None => match tz.from_local_datetime(&(naive - Duration::hours(1))) {
            chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.with_timezone(&Utc) + Duration::hours(1),
            chrono::LocalResult::None => return false,
        },
    };
    aware + Duration::days(offset_days) <= now
}

/// Repetition rule: `Ok(true)` = run.
/// Not recurring and already run → no; recurring and last run younger than
/// `recurring_interval_days` → no.
pub fn run_allowed(s: &Schedule, last_run: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match last_run {
        None => true,
        Some(_) if !s.is_recurring => false,
        Some(t) => t <= now - Duration::days(s.recurring_interval_days),
    }
}

async fn load_schedule(conn: &mut PgConnection, trigger_id: i64) -> Result<Schedule, sqlx::Error> {
    let r = sqlx::query(
        "SELECT schedule_offset_days, schedule_is_recurring, schedule_recurring_interval_days, schedule_date_field, \
         schedule_date_custom_field_id FROM documents_workflowtrigger WHERE id = $1",
    )
    .bind(trigger_id as i32)
    .fetch_one(&mut *conn)
    .await?;
    let field: String = r.try_get("schedule_date_field")?;
    Ok(Schedule {
        trigger_id,
        offset_days: i64::from(r.try_get::<i32, _>("schedule_offset_days")?),
        is_recurring: r.try_get("schedule_is_recurring")?,
        recurring_interval_days: i64::from(r.try_get::<i32, _>("schedule_recurring_interval_days")?),
        date_field: ScheduleDateField::parse(&field),
        custom_field_id: r.try_get::<Option<i32>, _>("schedule_date_custom_field_id")?.map(i64::from),
    })
}

/// Candidates by date field (`root_document__isnull=True`, without trash), order `-created`.
async fn candidates(conn: &mut PgConnection, s: &Schedule, now: DateTime<Utc>, tz: Tz) -> Result<Vec<i64>, sqlx::Error> {
    let thr = threshold(now, s.offset_days);
    const BASE: &str = "SELECT id FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL";
    const ORDER: &str = " ORDER BY created DESC, id";
    let ids: Vec<i32> = match s.date_field {
        Some(ScheduleDateField::Added) => {
            sqlx::query_scalar(&format!("{BASE} AND added <= $1{ORDER}")).bind(thr).fetch_all(&mut *conn).await?
        }
        Some(ScheduleDateField::Modified) => {
            sqlx::query_scalar(&format!("{BASE} AND modified <= $1{ORDER}")).bind(thr).fetch_all(&mut *conn).await?
        }
        Some(ScheduleDateField::Created) => {
            sqlx::query_scalar(&format!("{BASE} AND created <= $1{ORDER}")).bind(local_date(thr, tz)).fetch_all(&mut *conn).await?
        }
        Some(ScheduleDateField::CustomField) => {
            // cap earliest date to avoid massive scans
            let earliest = now - Duration::days(365);
            if s.offset_days < -365 {
                tracing::warn!(
                    target: "paperless.tasks",
                    "Trigger {} has large negative offset ({}), limiting earliest scan date to {}",
                    s.trigger_id,
                    s.offset_days,
                    earliest
                );
            }
            let rows: Vec<(i32, Option<NaiveDate>)> = sqlx::query_as(
                "SELECT document_id, value_date FROM documents_customfieldinstance \
                 WHERE field_id IS NOT DISTINCT FROM $1 AND deleted_at IS NULL AND value_date IS NOT NULL \
                   AND value_date <= $2 AND value_date >= $3 ORDER BY id",
            )
            .bind(s.custom_field_id.map(|i| i as i32))
            .bind(local_date(thr, tz))
            .bind(local_date(earliest, tz))
            .fetch_all(&mut *conn)
            .await?;
            let matched: Vec<i32> =
                rows.into_iter().filter(|(_, d)| d.is_some_and(|d| custom_date_due(d, s.offset_days, now, tz))).map(|(id, _)| id).collect();
            if matched.is_empty() {
                Vec::new()
            } else {
                sqlx::query_scalar(&format!("{BASE} AND id = ANY($1){ORDER}")).bind(&matched).fetch_all(&mut *conn).await?
            }
        }
        None => Vec::new(),
    };
    Ok(ids.into_iter().map(i64::from).collect())
}

/// `(id, correspondent_id, document_type_id, storage_path_id, original_filename)`.
type PrefilterRow = (i32, Option<i32>, Option<i32>, Option<i32>, Option<String>);

/// Ids per query in the prefilter (`= ANY($1)`).
const PREFILTER_CHUNK: usize = 1000;

/// Light `ExistingDocument`s for the prefilter (without content), in the order of `ids`.
/// Two queries per 1000 candidates instead of two per document (final security review K-N2: N+1
/// on old instances with almost all documents as candidates). The prefilter does not need
/// tag names (only ids).
async fn prefilter_docs(conn: &mut PgConnection, ids: &[i64]) -> Result<Vec<ExistingDocument>, sqlx::Error> {
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(PREFILTER_CHUNK) {
        let ids32: Vec<i32> = chunk.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
        let rows: Vec<PrefilterRow> = sqlx::query_as(
            "SELECT id, correspondent_id, document_type_id, storage_path_id, original_filename \
             FROM documents_document WHERE id = ANY($1)",
        )
        .bind(&ids32)
        .fetch_all(&mut *conn)
        .await?;
        let tag_rows: Vec<(i32, i32)> =
            sqlx::query_as("SELECT document_id, tag_id FROM documents_document_tags WHERE document_id = ANY($1) ORDER BY id")
                .bind(&ids32)
                .fetch_all(&mut *conn)
                .await?;
        let mut tags: HashMap<i64, Vec<ObjRef>> = HashMap::new();
        for (d, t) in tag_rows {
            tags.entry(i64::from(d)).or_default().push(ObjRef { id: i64::from(t), name: String::new() });
        }
        let mut by_id: HashMap<i64, ExistingDocument> = HashMap::with_capacity(rows.len());
        let named = |id: Option<i32>| id.map(|id| ObjRef { id: i64::from(id), name: String::new() });
        for (id, c, dt, sp, of) in rows {
            let id = i64::from(id);
            by_id.insert(
                id,
                ExistingDocument {
                    id,
                    effective_content: None,
                    display: String::new(),
                    tags: tags.remove(&id).unwrap_or_default(),
                    correspondent: named(c),
                    document_type: named(dt),
                    storage_path: named(sp),
                    original_filename: of,
                },
            );
        }
        out.extend(chunk.iter().filter_map(|id| by_id.remove(id)));
    }
    Ok(out)
}

/// Last `WorkflowRun` (type scheduled) of this workflow for the document (`order_by("-run_at").first()`).
async fn last_run(conn: &mut PgConnection, workflow_id: i64, document_id: i64) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT run_at FROM documents_workflowrun WHERE document_id = $1 AND type = $2 AND workflow_id = $3 \
         AND deleted_at IS NULL ORDER BY run_at DESC LIMIT 1",
    )
    .bind(document_id as i32)
    .bind(TRIGGER_SCHEDULED as i16)
    .bind(workflow_id as i32)
    .fetch_optional(&mut *conn)
    .await
}

impl Consumer {
    /// `documents.tasks.check_scheduled_workflows()`: check all active scheduled workflows and
    /// process due documents via `run_workflows(SCHEDULED, workflow_to_run=…)`, then
    /// `send_websocket_document_updated`. Autocommit like upstream. `now` = start time.
    pub async fn check_scheduled_workflows(&self) -> Result<(), sqlx::Error> {
        self.check_scheduled_workflows_at(db::now_micros()).await
    }

    /// Like [`Self::check_scheduled_workflows`] with a given `now` (tests/parity).
    pub async fn check_scheduled_workflows_at(&self, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        let scheduled: Vec<WorkflowDef> = workflows::workflows_for_trigger(&mut conn, TRIGGER_SCHEDULED).await?;
        if scheduled.is_empty() {
            return Ok(());
        }
        tracing::debug!(target: "paperless.tasks", "Checking {} scheduled workflows", scheduled.len());
        let tz = self.settings.time_zone;
        let audit = crate::audit::AuditLog { enabled: self.settings.audit_log_enabled, actor_id: None };
        for workflow in &scheduled {
            let triggers: Vec<&WorkflowTrigger> = workflow.triggers.iter().filter(|t| t.trigger_type == TRIGGER_SCHEDULED).collect();
            for trigger in triggers {
                let s = load_schedule(&mut conn, trigger.id).await?;
                tracing::debug!(
                    target: "paperless.tasks",
                    "Trigger {}: checking if (date + {} days) <= now ({now})",
                    trigger.id,
                    s.offset_days
                );
                let ids = candidates(&mut conn, &s, now, tz).await?;
                // K-N1: evaluate filter_custom_field_query once for all candidates
                let cfq = workflows::eval_custom_field_queries(&mut conn, &[trigger], &ids, tz).await?;
                let docs: Vec<i64> = prefilter_docs(&mut conn, &ids)
                    .await?
                    .into_iter()
                    // PaperRust: with an else branch also non-matching documents (they get the else branch)
                    .filter(|d| !workflow.else_actions.is_empty() || prefilter_matches(d, trigger, Some(&cfq)))
                    .map(|d| d.id)
                    .collect();
                if docs.is_empty() {
                    continue;
                }
                tracing::debug!(target: "paperless.tasks", "Found {} documents for trigger {}", docs.len(), trigger.display());
                for id in docs {
                    let last = last_run(&mut conn, workflow.id, id).await?;
                    if !run_allowed(&s, last, now) {
                        if s.is_recurring {
                            tracing::debug!(
                                target: "paperless.tasks",
                                "Skipping document {id} for recurring workflow {} as the last run was within the recurring interval",
                                workflow.name
                            );
                        } else {
                            tracing::debug!(
                                target: "paperless.tasks",
                                "Skipping document {id} for non-recurring workflow {} as it has already been run",
                                workflow.name
                            );
                        }
                        continue;
                    }
                    let Some(row) = db::load_document(&mut conn, id).await? else { continue };
                    let mut doc = DocInstance::from_db(row);
                    let outcome = workflows::run_workflows(&mut conn, &self.run_env(&audit), TRIGGER_SCHEDULED, &mut doc, Some(workflow)).await?;
                    workflows::enqueue_ai_suggestions(self.queue.as_deref(), &outcome).await;
                    // Scheduled workflows dont send document_updated signal, so send a websocket update here
                    self.send_websocket_document_updated(&mut conn, id).await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn sched(recurring: bool, interval: i64) -> Schedule {
        Schedule {
            trigger_id: 1,
            offset_days: 0,
            is_recurring: recurring,
            recurring_interval_days: interval,
            date_field: Some(ScheduleDateField::Added),
            custom_field_id: None,
        }
    }

    #[test]
    fn recurring_rules() {
        let now = utc("2026-11-08T12:00:00Z");
        assert!(run_allowed(&sched(false, 0), None, now));
        assert!(!run_allowed(&sched(false, 0), Some(utc("2020-01-01T00:00:00Z")), now));
        // last run 2 days ago, interval 3 → no; interval 2 → yes, exactly at the boundary
        let last = utc("2026-11-06T12:00:00Z");
        assert!(!run_allowed(&sched(true, 3), Some(last), now));
        assert!(run_allowed(&sched(true, 2), Some(last), now));
        assert!(run_allowed(&sched(true, 1), Some(last), now));
    }

    #[test]
    fn created_threshold_uses_local_date() {
        // 2026-11-07T23:30Z is already the 8th of November in Zurich.
        let t = utc("2026-11-07T23:30:00Z");
        assert_eq!(local_date(t, chrono_tz::Europe::Zurich), NaiveDate::from_ymd_opt(2026, 11, 8).unwrap());
        assert_eq!(threshold(utc("2026-11-08T00:00:00Z"), -7), utc("2026-11-15T00:00:00Z"));
    }

    #[test]
    fn custom_date_offsets() {
        let tz = chrono_tz::Europe::Zurich;
        let now = utc("2026-11-08T12:00:00Z");
        let d = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        // 1 Nov 00:00 Zurich (= 31 Oct 23:00Z) + 7 days = 7 Nov 23:00Z ≤ now
        assert!(custom_date_due(d, 7, now, tz));
        assert!(!custom_date_due(d, 8, now, tz));
        // negative offset: appointment in the future, due 7 days before
        let future = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        assert!(custom_date_due(future, -7, now, tz));
        assert!(!custom_date_due(future, -6, now, tz));
    }

    #[test]
    fn date_fields() {
        assert_eq!(ScheduleDateField::parse("custom_field"), Some(ScheduleDateField::CustomField));
        assert_eq!(ScheduleDateField::parse("x"), None);
    }
}
