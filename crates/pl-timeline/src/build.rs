//! Build timeline and "Upcoming" from loaded rows (pure, without DB).

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::areas::{Area, DocKeys, ResolvedRules};
use crate::{DateKind, Source};

/// Reference with name only when visible.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RefOut {
    pub uuid: Uuid,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Amount {
    pub value: String,
    pub currency: Option<String>,
    pub field: Uuid,
}

/// A visible document with everything an entry needs.
#[derive(Debug, Clone, PartialEq)]
pub struct DocRow {
    pub uuid: Uuid,
    pub title: String,
    pub created: NaiveDate,
    pub keys: DocKeys,
    pub correspondent: Option<RefOut>,
    pub document_type: Option<RefOut>,
    pub amount: Option<Amount>,
}

/// A date of a document (one source).
#[derive(Debug, Clone, PartialEq)]
pub struct DateRow {
    pub doc_id: i64,
    pub kind: DateKind,
    pub date: NaiveDate,
    pub source: Source,
    pub confidence: f32,
    pub field: Option<RefOut>,
    pub phrase: Option<String>,
    pub page: Option<i32>,
    pub reminder: Option<ReminderRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReminderRef {
    pub id: i64,
    pub note: Option<String>,
}

/// State per user and entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemState {
    pub done_at: Option<DateTime<Utc>>,
    pub snoozed_until: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Span {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocOut {
    pub uuid: Uuid,
    pub legacy_id: i64,
    pub title: String,
    pub created: NaiveDate,
    pub correspondent: Option<RefOut>,
    pub document_type: Option<RefOut>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StateOut {
    pub done: bool,
    pub done_at: Option<DateTime<Utc>>,
    pub snoozed_until: Option<NaiveDate>,
}

/// A timeline entry.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    pub id: String,
    pub kind: DateKind,
    pub date: NaiveDate,
    pub span: Option<Span>,
    pub source: Source,
    pub sources: Vec<Source>,
    pub confidence: f32,
    pub field: Option<RefOut>,
    pub phrase: Option<String>,
    pub page: Option<i32>,
    pub reminder: Option<ReminderRef>,
    pub document: DocOut,
    pub amount: Option<Amount>,
    pub state: StateOut,
    #[serde(skip)]
    pub doc_id: i64,
}

/// `<uuid>.<kind>.<YYYYMMDD>`.
pub fn item_id(doc: Uuid, kind: DateKind, date: NaiveDate) -> String {
    format!("{doc}.{}.{}", kind.as_str(), date.format("%Y%m%d"))
}

pub fn parse_item_id(s: &str) -> Option<(Uuid, DateKind, NaiveDate)> {
    let mut it = s.split('.');
    let (u, k, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() {
        return None;
    }
    Some((
        Uuid::parse_str(u).ok()?,
        DateKind::parse(k)?,
        NaiveDate::parse_from_str(d, "%Y%m%d").ok()?,
    ))
}

/// All entries (merged, with spans and state), unfiltered, sorted by date.
/// The caller produces `created` entries as [`DateRow`] with `Source::Created`.
pub fn items(
    docs: &HashMap<i64, DocRow>,
    rows: &[DateRow],
    states: &HashMap<String, ItemState>,
) -> Vec<Item> {
    let mut groups: BTreeMap<(i64, DateKind, NaiveDate), Vec<&DateRow>> = BTreeMap::new();
    for r in rows {
        if docs.contains_key(&r.doc_id) {
            groups
                .entry((r.doc_id, r.kind, r.date))
                .or_default()
                .push(r);
        }
    }
    let mut valid_from: HashMap<i64, NaiveDate> = HashMap::new();
    for (doc, kind, date) in groups.keys() {
        if *kind == DateKind::ValidFrom {
            let e = valid_from.entry(*doc).or_insert(*date);
            if date < e {
                *e = *date;
            }
        }
    }
    let mut out = Vec::with_capacity(groups.len());
    for ((doc_id, kind, date), mut g) in groups {
        g.sort_by(|a, b| {
            a.source.rank().cmp(&b.source.rank()).then(
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        let best = g[0];
        let mut sources: Vec<Source> = g.iter().map(|r| r.source).collect();
        sources.dedup();
        let d = &docs[&doc_id];
        let span = kind.has_span().then(|| {
            let start = valid_from
                .get(&doc_id)
                .copied()
                .filter(|s| *s <= date)
                .or(Some(d.created).filter(|c| *c <= date))?;
            Some(Span { start, end: date })
        });
        let id = item_id(d.uuid, kind, date);
        let st = states.get(&id).cloned().unwrap_or_default();
        out.push(Item {
            id,
            kind,
            date,
            span: span.flatten(),
            source: best.source,
            sources,
            confidence: best.confidence,
            field: g.iter().find_map(|r| r.field.clone()),
            phrase: g.iter().find_map(|r| r.phrase.clone()),
            page: g.iter().find_map(|r| r.page),
            reminder: g.iter().find_map(|r| r.reminder.clone()),
            document: DocOut {
                uuid: d.uuid,
                legacy_id: doc_id,
                title: d.title.clone(),
                created: d.created,
                correspondent: d.correspondent.clone(),
                document_type: d.document_type.clone(),
            },
            amount: d.amount.clone(),
            state: StateOut {
                done: st.done_at.is_some(),
                done_at: st.done_at,
                snoozed_until: st.snoozed_until,
            },
            doc_id,
        });
    }
    out.sort_by(|a, b| {
        (a.date, &a.document.title, a.doc_id, a.kind).cmp(&(
            b.date,
            &b.document.title,
            b.doc_id,
            b.kind,
        ))
    });
    out
}

/// Does the entry (date or span) lie in [from, to]?
pub fn in_range(i: &Item, from: NaiveDate, to: NaiveDate) -> bool {
    (i.date >= from && i.date <= to)
        || i.span
            .as_ref()
            .is_some_and(|s| s.start <= to && s.end >= from)
}

/// An area with resolved rules.
#[derive(Debug, Clone)]
pub struct LaneArea {
    pub area: Area,
    pub rules: ResolvedRules,
}

#[derive(Debug, Clone, Serialize)]
pub struct Lane {
    pub key: String,
    pub area: Option<Area>,
    pub count: usize,
    pub items: Vec<Item>,
}

/// Timeline query.
#[derive(Debug, Clone)]
pub struct TimelineQuery {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// `None` = all kinds.
    pub kinds: Option<HashSet<DateKind>>,
    /// `None` = all lanes; otherwise area UUIDs as text and/or `other`.
    pub lanes: Option<HashSet<String>>,
    pub limit: usize,
}

/// Area key of a document (UUIDs of the matching areas, otherwise `other`).
pub fn area_keys(areas: &[LaneArea], d: &DocKeys) -> Vec<String> {
    let v: Vec<String> = areas
        .iter()
        .filter(|a| a.rules.matches(d))
        .map(|a| a.area.uuid.to_string())
        .collect();
    if v.is_empty() {
        vec!["other".to_string()]
    } else {
        v
    }
}

/// Build lanes. Returns: (lanes, truncated).
pub fn lanes(
    all: Vec<Item>,
    docs: &HashMap<i64, DocRow>,
    areas: &[LaneArea],
    q: &TimelineQuery,
) -> (Vec<Lane>, bool) {
    let want = |key: &str| q.lanes.as_ref().is_none_or(|l| l.contains(key));
    let mut keep: Vec<Item> = all
        .into_iter()
        .filter(|i| {
            q.kinds.as_ref().is_none_or(|k| k.contains(&i.kind)) && in_range(i, q.from, q.to)
        })
        .filter(|i| {
            docs.get(&i.doc_id)
                .is_some_and(|d| area_keys(areas, &d.keys).iter().any(|k| want(k)))
        })
        .collect();
    let truncated = keep.len() > q.limit;
    keep.truncate(q.limit);
    let mut out: Vec<Lane> = Vec::new();
    let mut sorted: Vec<&LaneArea> = areas.iter().collect();
    sorted.sort_by(|a, b| (a.area.order, &a.area.name).cmp(&(b.area.order, &b.area.name)));
    for a in sorted {
        let key = a.area.uuid.to_string();
        if !want(&key) {
            continue;
        }
        let items: Vec<Item> = keep
            .iter()
            .filter(|i| {
                docs.get(&i.doc_id)
                    .is_some_and(|d| a.rules.matches(&d.keys))
            })
            .cloned()
            .collect();
        out.push(Lane {
            key,
            area: Some(a.area.clone()),
            count: items.len(),
            items,
        });
    }
    if want("other") {
        let items: Vec<Item> = keep
            .into_iter()
            .filter(|i| {
                docs.get(&i.doc_id)
                    .is_some_and(|d| !areas.iter().any(|a| a.rules.matches(&d.keys)))
            })
            .collect();
        out.push(Lane {
            key: "other".into(),
            area: None,
            count: items.len(),
            items,
        });
    }
    (out, truncated)
}

/// Entry in "Upcoming".
#[derive(Debug, Clone, Serialize)]
pub struct Upcoming {
    #[serde(flatten)]
    pub item: Item,
    pub overdue: bool,
    pub days_left: i64,
    pub areas: Vec<String>,
}

/// "Upcoming": open deadlines in [today − overdue_days, today + days].
#[allow(clippy::too_many_arguments)]
pub fn upcoming(
    all: Vec<Item>,
    docs: &HashMap<i64, DocRow>,
    areas: &[LaneArea],
    today: NaiveDate,
    days: i64,
    overdue_days: i64,
    lanes: Option<&HashSet<String>>,
    include_done: bool,
) -> Vec<Upcoming> {
    let from = today - chrono::Duration::days(overdue_days);
    let to = today + chrono::Duration::days(days);
    all.into_iter()
        .filter(|i| i.kind.is_deadline() && i.date >= from && i.date <= to)
        .filter(|i| include_done || !i.state.done)
        .filter(|i| i.state.snoozed_until.is_none_or(|s| s <= today))
        .filter_map(|i| {
            let keys = area_keys(areas, &docs.get(&i.doc_id)?.keys);
            if let Some(l) = lanes
                && !keys.iter().any(|k| l.contains(k))
            {
                return None;
            }
            Some(Upcoming {
                overdue: i.date < today,
                days_left: (i.date - today).num_days(),
                areas: keys,
                item: i,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::areas::{Rules, Scope};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn doc(id: i64, created: NaiveDate, tags: Vec<i64>) -> (i64, DocRow) {
        (
            id,
            DocRow {
                uuid: Uuid::from_u128(id as u128),
                title: format!("Doc {id}"),
                created,
                keys: DocKeys {
                    id,
                    tags,
                    ..Default::default()
                },
                correspondent: None,
                document_type: None,
                amount: None,
            },
        )
    }

    fn row(doc_id: i64, kind: DateKind, date: NaiveDate, source: Source, conf: f32) -> DateRow {
        DateRow {
            doc_id,
            kind,
            date,
            source,
            confidence: conf,
            field: None,
            phrase: None,
            page: None,
            reminder: None,
        }
    }

    fn area(n: u128, order: i64, tags: &[i64]) -> LaneArea {
        LaneArea {
            area: Area {
                uuid: Uuid::from_u128(1000 + n),
                scope: Scope::User,
                name: format!("A{n}"),
                color: "#000000".into(),
                icon: "tag".into(),
                order,
                rules: Rules::default(),
                created: Utc::now(),
                modified: Utc::now(),
                id: n as i64,
                user_id: None,
            },
            rules: ResolvedRules {
                tags: tags.iter().copied().collect(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn ids() {
        let u = Uuid::from_u128(7);
        let id = item_id(u, DateKind::CancelBy, d(2026, 3, 9));
        assert_eq!(id, format!("{u}.cancel_by.20260309"));
        assert_eq!(
            parse_item_id(&id),
            Some((u, DateKind::CancelBy, d(2026, 3, 9)))
        );
        for bad in [
            "x",
            "x.due.20260101",
            &format!("{u}.nope.20260101"),
            &format!("{u}.due.2026"),
            &format!("{u}.due.20260101.x"),
        ] {
            assert!(parse_item_id(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn merge_spans_and_states() {
        let docs: HashMap<i64, DocRow> = [
            doc(1, d(2026, 1, 10), vec![5]),
            doc(2, d(2026, 5, 1), vec![]),
        ]
        .into_iter()
        .collect();
        let mut rows = vec![
            row(1, DateKind::Due, d(2026, 2, 1), Source::Content, 0.8),
            row(1, DateKind::Due, d(2026, 2, 1), Source::CustomField, 1.0),
            row(
                1,
                DateKind::ValidUntil,
                d(2027, 1, 9),
                Source::CustomField,
                1.0,
            ),
            row(2, DateKind::ValidFrom, d(2026, 6, 1), Source::Content, 0.9),
            row(
                2,
                DateKind::ValidUntil,
                d(2028, 5, 31),
                Source::Content,
                0.9,
            ),
            row(2, DateKind::WarrantyUntil, d(2020, 1, 1), Source::User, 1.0),
            row(99, DateKind::Due, d(2026, 2, 1), Source::Content, 0.8), // invisible
        ];
        rows[0].phrase = Some("fällig am 01.02.2026".into());
        let mut states = HashMap::new();
        states.insert(
            item_id(Uuid::from_u128(1), DateKind::Due, d(2026, 2, 1)),
            ItemState {
                done_at: Some(Utc::now()),
                snoozed_until: None,
            },
        );
        let it = items(&docs, &rows, &states);
        assert_eq!(it.len(), 5);
        let due = it.iter().find(|i| i.kind == DateKind::Due).unwrap();
        assert_eq!(due.source, Source::CustomField);
        assert_eq!(due.sources, vec![Source::CustomField, Source::Content]);
        assert_eq!(due.phrase.as_deref(), Some("fällig am 01.02.2026"));
        assert!(due.state.done);
        let vu1 = it
            .iter()
            .find(|i| i.doc_id == 1 && i.kind == DateKind::ValidUntil)
            .unwrap();
        assert_eq!(
            vu1.span,
            Some(Span {
                start: d(2026, 1, 10),
                end: d(2027, 1, 9)
            }),
            "Spanne ab created"
        );
        let vu2 = it
            .iter()
            .find(|i| i.doc_id == 2 && i.kind == DateKind::ValidUntil)
            .unwrap();
        assert_eq!(
            vu2.span.as_ref().unwrap().start,
            d(2026, 6, 1),
            "Spanne ab valid_from"
        );
        let w = it
            .iter()
            .find(|i| i.kind == DateKind::WarrantyUntil)
            .unwrap();
        assert_eq!(w.span, None, "Ende vor Beginn → keine Spanne");
        // span overlaps the area, date lies outside
        assert!(in_range(vu2, d(2027, 1, 1), d(2027, 12, 31)));
        assert!(!in_range(due, d(2027, 1, 1), d(2027, 12, 31)));
    }

    #[test]
    fn lanes_and_limits() {
        let docs: HashMap<i64, DocRow> = [
            doc(1, d(2026, 1, 10), vec![5]),
            doc(2, d(2026, 3, 1), vec![6]),
            doc(3, d(2026, 4, 1), vec![5, 6]),
            doc(4, d(2026, 4, 2), vec![]),
        ]
        .into_iter()
        .collect();
        let rows: Vec<DateRow> = docs
            .iter()
            .map(|(id, dr)| row(*id, DateKind::Created, dr.created, Source::Created, 1.0))
            .collect();
        let areas = vec![area(2, 1, &[6]), area(1, 0, &[5])];
        let q = TimelineQuery {
            from: d(2026, 1, 1),
            to: d(2026, 12, 31),
            kinds: None,
            lanes: None,
            limit: 100,
        };
        let (l, t) = lanes(items(&docs, &rows, &HashMap::new()), &docs, &areas, &q);
        assert!(!t);
        assert_eq!(
            l.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(),
            vec![
                areas[1].area.uuid.to_string(),
                areas[0].area.uuid.to_string(),
                "other".into()
            ]
        );
        assert_eq!(
            l[0].items.iter().map(|i| i.doc_id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(
            l[1].items.iter().map(|i| i.doc_id).collect::<Vec<_>>(),
            vec![2, 3],
            "Dokument in zwei Bereichen"
        );
        assert_eq!(
            l[2].items.iter().map(|i| i.doc_id).collect::<Vec<_>>(),
            vec![4]
        );
        // only other
        let q2 = TimelineQuery {
            lanes: Some(["other".to_string()].into()),
            ..q.clone()
        };
        let (l, _) = lanes(items(&docs, &rows, &HashMap::new()), &docs, &areas, &q2);
        assert_eq!(l.len(), 1);
        // boundary
        let q3 = TimelineQuery {
            limit: 2,
            ..q.clone()
        };
        let (l, t) = lanes(items(&docs, &rows, &HashMap::new()), &docs, &areas, &q3);
        assert!(t);
        assert_eq!(
            l.iter().map(|x| x.count).sum::<usize>(),
            2,
            "Dok 1 (A1) + Dok 2 (A2)"
        );
        // kinds
        let q4 = TimelineQuery {
            kinds: Some([DateKind::Due].into()),
            ..q
        };
        let (l, _) = lanes(items(&docs, &rows, &HashMap::new()), &docs, &areas, &q4);
        assert!(l.iter().all(|x| x.items.is_empty()));
    }

    #[test]
    fn upcoming_filters() {
        let today = d(2026, 11, 10);
        let docs: HashMap<i64, DocRow> = [
            doc(1, d(2026, 1, 1), vec![5]),
            doc(2, d(2026, 1, 1), vec![]),
        ]
        .into_iter()
        .collect();
        let rows = vec![
            row(1, DateKind::Due, d(2026, 11, 30), Source::CustomField, 1.0),
            row(1, DateKind::CancelBy, d(2026, 11, 1), Source::Content, 0.9), // overdue
            row(
                1,
                DateKind::ValidFrom,
                d(2026, 11, 20),
                Source::Content,
                0.9,
            ), // no deadline
            row(2, DateKind::Due, d(2027, 3, 1), Source::User, 1.0),          // too far
            row(
                2,
                DateKind::WarrantyUntil,
                d(2026, 12, 1),
                Source::Content,
                0.9,
            ), // snoozed
            row(
                2,
                DateKind::ValidUntil,
                d(2026, 12, 2),
                Source::Content,
                0.9,
            ), // done
            row(1, DateKind::Created, d(2026, 11, 15), Source::Created, 1.0),
        ];
        let mut states = HashMap::new();
        states.insert(
            item_id(Uuid::from_u128(2), DateKind::WarrantyUntil, d(2026, 12, 1)),
            ItemState {
                done_at: None,
                snoozed_until: Some(d(2026, 11, 20)),
            },
        );
        states.insert(
            item_id(Uuid::from_u128(2), DateKind::ValidUntil, d(2026, 12, 2)),
            ItemState {
                done_at: Some(Utc::now()),
                snoozed_until: None,
            },
        );
        let areas = vec![area(1, 0, &[5])];
        let up = upcoming(
            items(&docs, &rows, &states),
            &docs,
            &areas,
            today,
            60,
            30,
            None,
            false,
        );
        let got: Vec<(i64, DateKind, bool, i64)> = up
            .iter()
            .map(|u| (u.item.doc_id, u.item.kind, u.overdue, u.days_left))
            .collect();
        assert_eq!(
            got,
            vec![
                (1, DateKind::CancelBy, true, -9),
                (1, DateKind::Due, false, 20)
            ]
        );
        assert_eq!(up[0].areas, vec![areas[0].area.uuid.to_string()]);
        let all = upcoming(
            items(&docs, &rows, &states),
            &docs,
            &areas,
            today,
            60,
            0,
            None,
            true,
        );
        assert_eq!(
            all.len(),
            2,
            "Erledigte mit include_done, Überfällige ohne overdue_days: {all:?}"
        );
        let other: HashSet<String> = ["other".to_string()].into();
        let o = upcoming(
            items(&docs, &rows, &states),
            &docs,
            &areas,
            today,
            60,
            30,
            Some(&other),
            true,
        );
        assert_eq!(
            o.iter().map(|u| u.item.kind).collect::<Vec<_>>(),
            vec![DateKind::ValidUntil]
        );
        // visible again after snoozing
        let later = upcoming(
            items(&docs, &rows, &states),
            &docs,
            &areas,
            d(2026, 11, 21),
            60,
            0,
            None,
            false,
        );
        assert!(later.iter().any(|u| u.item.kind == DateKind::WarrantyUntil));
    }
}
