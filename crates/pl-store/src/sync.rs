//! `sync/ids` and `sync/resolve` (docs/SYNC.md §3.3, §3.5).

use std::collections::BTreeSet;

use pl_authz::objects::{FilterOpts, permitted_objects_filter};
use pl_core::sqlbind::SqlxBinder as _;
use pl_domain::model::*;
use pl_domain::{DomainError, DomainResult, Kind, Principal};
use sea_query::{Expr, PostgresQueryBuilder, Query};
use uuid::Uuid;

use crate::{Store, a, auth, db_err, documents, ident, objects, perm, rules};

/// Visible ids of a kind as an SQL subquery (one column), without a model permission check:
/// documents like the list (root documents outside the trash + `permitted_objects_filter`),
/// other kinds with object permissions via `permitted_objects_filter`, without object permissions all.
pub(crate) fn visible_sql(user: &pl_authz::User, kind: Kind) -> String {
    let spec = perm::spec(kind);
    if kind == Kind::Document {
        // L1: visibility directly on the row
        crate::docsql::Visibility::for_user(user).visible_ids_sql()
    } else {
        let mut q = Query::select();
        q.expr(Expr::cust("o.id")).from_as(a(spec.table), a("o"));
        if let Some(m) = &spec.model {
            q.cond_where(permitted_objects_filter(Some(user), m, "o", &FilterOpts::default()));
        }
        q.to_string(PostgresQueryBuilder)
    }
}

/// All visible `(uuid, rev)` of a kind, sorted by `legacy_id`.
pub(crate) async fn ids(store: &Store, p: &Principal, kind: Kind) -> DomainResult<Vec<(Uuid, i64)>> {
    let user = auth::user(p)?;
    let spec = perm::spec(kind);
    perm::require(user, spec.view_perm)?;
    let visible = visible_sql(user, kind);
    // create missing identities (PERF-SCALE Q6: only missing ones). From data model stage 5 documents
    // and labels carry uuid/rev in the row: the gate condition paperrust_sync_ensure_needed
    // then prevents the subquery from running at all.
    sqlx::query(&format!(
        "SELECT paperrust_sync_ensure($1, ARRAY(SELECT v.id::int8 FROM ({visible}) AS v(id)
          WHERE paperrust_sync_ensure_needed($1)
            AND NOT EXISTS (SELECT 1 FROM paperrust_sync_object s WHERE s.kind = $1 AND s.local_id = v.id)))"
    ))
    .bind(kind.code())
    .execute(&store.db)
    .await
    .map_err(db_err)?;
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(&format!(
        "SELECT o.uuid, o.rev FROM paperrust_sync_object o
          WHERE o.kind = $1 AND o.local_id IN (SELECT v.id FROM ({visible}) AS v(id)) ORDER BY o.local_id"
    ))
    .bind(kind.code())
    .fetch_all(&store.db)
    .await
    .map_err(db_err)?;
    Ok(rows)
}

/// Selection → visible documents with `rev` and file sizes.
pub(crate) async fn resolve(store: &Store, p: &Principal, sel: &Selection) -> DomainResult<Resolution> {
    let user = auth::user(p)?;
    perm::require(user, "documents.view_document")?;
    let mut ids: BTreeSet<i64> = BTreeSet::new();
    let mut skipped_saved_views: Vec<Uuid> = Vec::new();

    if !sel.documents.is_empty() {
        let map = ident::resolve(&store.db, Kind::Document, &sel.documents).await?;
        let cand: Vec<i64> = map.values().map(|(id, _)| *id).collect();
        ids.extend(select(store, user, &[("id__in".into(), join(&cand))]).await?);
    }
    if !sel.tags.is_empty() {
        let map = ident::resolve(&store.db, Kind::Tag, &sel.tags).await?;
        if let Some((key, tag_ids)) = tag_rule(&sel.tags, &map, sel.tag_mode.as_deref())? {
            ids.extend(select(store, user, &[(key.into(), join(&tag_ids))]).await?);
        }
    }
    if !sel.saved_views.is_empty() {
        let map = ident::resolve(&store.db, Kind::SavedView, &sel.saved_views).await?;
        let sv_ids: Vec<i64> = sel.saved_views.iter().filter_map(|u| map.get(u).map(|(id, _)| *id)).collect();
        // only visible views (like v1); invisible ones contribute nothing
        let visible = objects::by_legacy_ids(store, p, Kind::SavedView, &sv_ids).await?;
        let rules: Vec<(i64, Vec<FilterRule>)> = visible
            .into_iter()
            .filter_map(|(id, o)| match o {
                Some(Object::SavedView(v)) => Some((id, v.filter_rules)),
                _ => None,
            })
            .collect();
        let other_parts = !sel.documents.is_empty() || !sel.tags.is_empty();
        let (params_list, skipped) = view_params(&rules, other_parts)?;
        for params in params_list {
            ids.extend(select(store, user, &params).await?);
        }
        skipped_saved_views = skipped_uuids(&sel.saved_views, &map, &skipped);
    }

    let ids: Vec<i64> = ids.into_iter().collect();
    let idents = ident::idents(&store.db, Kind::Document, &ids).await?;
    let files = documents::latest_files(store, &ids).await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(idn) = idents.get(&id) else { continue };
        let size = match files.get(&id) {
            Some(f) => {
                let (o, ar, _) = documents::paths(store, f).await?;
                ResolvedSize {
                    original: size_of(o.as_ref()).await,
                    archive: if f.archive_filename.is_some() { size_of(ar.as_ref()).await } else { None },
                }
            }
            None => ResolvedSize { original: None, archive: None },
        };
        out.push(ResolvedDoc { uuid: idn.uuid, rev: idn.rev, size });
    }
    Ok(Resolution { documents: out, skipped_saved_views })
}

/// Skipped view ids → the UUIDs the request used, in request order, without duplicates.
fn skipped_uuids(requested: &[Uuid], map: &std::collections::HashMap<Uuid, (i64, i64)>, skipped: &[i64]) -> Vec<Uuid> {
    let mut seen = BTreeSet::new();
    requested.iter().filter(|u| map.get(u).is_some_and(|(id, _)| skipped.contains(id))).filter(|u| seen.insert(**u)).copied().collect()
}

/// Tag part of the selection → filter key + tag ids, or `None` when the part matches nothing.
///
/// `any`: unknown/deleted UUIDs are ignored (like unknown document UUIDs). `all`: every tag has to
/// be present on the document, and an unknown/deleted tag can never be present, so a single unknown
/// UUID makes the part match nothing (dropping it would widen the result to too many documents).
fn tag_rule(
    tags: &[Uuid],
    map: &std::collections::HashMap<Uuid, (i64, i64)>,
    mode: Option<&str>,
) -> DomainResult<Option<(&'static str, Vec<i64>)>> {
    let key = match mode.unwrap_or("any") {
        "any" => "tags__id__in",
        "all" => "tags__id__all",
        other => return Err(DomainError::field("tag_mode", format!("Unknown tag_mode \"{other}\" (any, all)."))),
    };
    let known: Vec<i64> = tags.iter().filter_map(|u| map.get(u).map(|(id, _)| *id)).collect();
    if known.is_empty() || (key == "tags__id__all" && known.len() != tags.len()) {
        return Ok(None);
    }
    Ok(Some((key, known)))
}

/// v1 parameters of one view.
type ViewParams = Vec<(String, String)>;

/// v1 parameters of the visible saved views plus the ids of the skipped views. A view whose rules
/// cannot be evaluated here (full-text rules, unknown rule types → `NotImplemented`) is skipped with
/// a warning so it does not abort the rest of the selection; the caller reports it in
/// `skipped_saved_views`. Only when nothing else was requested (`other_parts == false`) and every
/// view is unsupported, the first error is returned (501) – a selection consisting only of such
/// views would otherwise silently resolve to nothing.
fn view_params(views: &[(i64, Vec<FilterRule>)], other_parts: bool) -> DomainResult<(Vec<ViewParams>, Vec<i64>)> {
    let mut out = Vec::with_capacity(views.len());
    let mut skipped = Vec::new();
    let mut first_skip: Option<DomainError> = None;
    for (id, rules) in views {
        match rules::to_params(rules) {
            Ok(p) => out.push(p),
            Err(e @ DomainError::NotImplemented(_)) => {
                tracing::warn!(saved_view = *id, "sync/resolve: skipping saved view: {e}");
                skipped.push(*id);
                first_skip.get_or_insert(e);
            }
            Err(e) => return Err(e),
        }
    }
    match first_skip {
        Some(e) if out.is_empty() && !other_parts => Err(e),
        _ => Ok((out, skipped)),
    }
}

fn join(ids: &[i64]) -> String {
    if ids.is_empty() {
        return "0".into();
    }
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

async fn size_of(p: Option<&std::path::PathBuf>) -> Option<u64> {
    match p {
        Some(p) => tokio::fs::metadata(p).await.ok().filter(|m| m.is_file()).map(|m| m.len()),
        None => None,
    }
}

/// Visible root documents for v1 filter parameters.
async fn select(store: &Store, user: &pl_authz::User, params: &[(String, String)]) -> DomainResult<Vec<i64>> {
    let q = documents::filtered_query(store, user, params).await?;
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let v: Vec<i32> = sqlx::query_scalar_with(&sql, values).fetch_all(&store.db).await.map_err(db_err)?;
    Ok(v.into_iter().map(i64::from).collect())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn tag_rule_all_with_unknown_uuid_matches_nothing() {
        let map: HashMap<Uuid, (i64, i64)> = [(u(1), (10, 1)), (u(2), (20, 1))].into_iter().collect();
        // all known → tags__id__all with every id
        assert_eq!(tag_rule(&[u(1), u(2)], &map, Some("all")).unwrap(), Some(("tags__id__all", vec![10, 20])));
        // one unknown/deleted → no document can carry it → nothing (was: tags__id__all=10 → too many)
        assert_eq!(tag_rule(&[u(1), u(3)], &map, Some("all")).unwrap(), None);
        assert_eq!(tag_rule(&[u(3)], &map, Some("all")).unwrap(), None);
        // any: unknown ones are ignored
        assert_eq!(tag_rule(&[u(1), u(3)], &map, None).unwrap(), Some(("tags__id__in", vec![10])));
        assert_eq!(tag_rule(&[u(3)], &map, Some("any")).unwrap(), None);
        assert!(tag_rule(&[u(1)], &map, Some("some")).is_err());
    }

    fn r(t: i64, v: Option<&str>) -> FilterRule {
        FilterRule { rule_type: t, value: v.map(str::to_string) }
    }

    #[test]
    fn unsupported_view_is_skipped_not_fatal() {
        let fulltext = (1, vec![r(20, Some("invoice"))]);
        let normal = (2, vec![r(3, Some("7"))]);
        // one full-text view does not abort the other view
        let (p, skipped) = view_params(&[fulltext.clone(), normal.clone()], false).unwrap();
        assert_eq!(p, vec![vec![("correspondent__id".to_string(), "7".to_string())]]);
        assert_eq!(skipped, vec![1]);
        // nor documents/tags requested alongside
        let (p, skipped) = view_params(std::slice::from_ref(&fulltext), true).unwrap();
        assert!(p.is_empty());
        assert_eq!(skipped, vec![1]);
        // only unsupported views requested → 501 as before (client reports the entry)
        assert!(matches!(view_params(&[fulltext], false), Err(DomainError::NotImplemented(_))));
        // unknown rule type is skipped the same way
        let (_, skipped) = view_params(&[(3, vec![r(999, None)]), normal.clone()], false).unwrap();
        assert_eq!(skipped, vec![3]);
        // nothing skipped → empty list
        assert!(view_params(&[normal], false).unwrap().1.is_empty());
    }

    #[test]
    fn skipped_uuids_follow_request_order_without_duplicates() {
        let map: HashMap<Uuid, (i64, i64)> = [(u(1), (10, 1)), (u(2), (20, 1)), (u(3), (30, 1))].into_iter().collect();
        // u(9) unknown, u(2) not skipped, u(3) requested twice
        let got = skipped_uuids(&[u(3), u(9), u(2), u(1), u(3)], &map, &[10, 30]);
        assert_eq!(got, vec![u(3), u(1)]);
        assert!(skipped_uuids(&[u(1)], &map, &[]).is_empty());
    }
}
