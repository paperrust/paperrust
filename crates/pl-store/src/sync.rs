//! `sync/ids` and `sync/resolve` (docs/SYNC.md §3.3, §3.5).

use std::collections::BTreeSet;

use pl_authz::objects::{FilterOpts, permitted_objects_filter};
use pl_domain::model::*;
use pl_domain::{DomainError, DomainResult, Kind, Principal};
use sea_query::{Expr, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder as _;
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
pub(crate) async fn resolve(store: &Store, p: &Principal, sel: &Selection) -> DomainResult<Vec<ResolvedDoc>> {
    let user = auth::user(p)?;
    perm::require(user, "documents.view_document")?;
    let mut ids: BTreeSet<i64> = BTreeSet::new();

    if !sel.documents.is_empty() {
        let map = ident::resolve(&store.db, Kind::Document, &sel.documents).await?;
        let cand: Vec<i64> = map.values().map(|(id, _)| *id).collect();
        ids.extend(select(store, user, &[("id__in".into(), join(&cand))]).await?);
    }
    if !sel.tags.is_empty() {
        let map = ident::resolve(&store.db, Kind::Tag, &sel.tags).await?;
        let tag_ids: Vec<i64> = sel.tags.iter().filter_map(|u| map.get(u).map(|(id, _)| *id)).collect();
        let key = match sel.tag_mode.as_deref().unwrap_or("any") {
            "any" => "tags__id__in",
            "all" => "tags__id__all",
            other => return Err(DomainError::field("tag_mode", format!("Unknown tag_mode \"{other}\" (any, all)."))),
        };
        if !tag_ids.is_empty() {
            ids.extend(select(store, user, &[(key.into(), join(&tag_ids))]).await?);
        }
    }
    if !sel.saved_views.is_empty() {
        let map = ident::resolve(&store.db, Kind::SavedView, &sel.saved_views).await?;
        let sv_ids: Vec<i64> = sel.saved_views.iter().filter_map(|u| map.get(u).map(|(id, _)| *id)).collect();
        // only visible views (like v1); invisible ones contribute nothing
        let visible = objects::by_legacy_ids(store, p, Kind::SavedView, &sv_ids).await?;
        for (_, o) in visible {
            if let Some(Object::SavedView(v)) = o {
                let params = rules::to_params(&v.filter_rules)?;
                ids.extend(select(store, user, &params).await?);
            }
        }
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
    Ok(out)
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
