//! HTTP handlers of the four object ViewSets.
//!
//! upstream: src/documents/views.py@v3.4.2 (CorrespondentViewSet.list/retrieve, TagViewSet.list,
//!           DocumentTypeViewSet, StoragePathViewSet.destroy, PermissionsAwareDocumentCountMixin,
//!           BulkPermissionMixin, PassUserMixin), rest_framework 3.18.3 mixins
//!
//! Extractor order like DRF `initial()`: RequestContext (negotiation/version → 406) first,
//! then authentication (AuthUser → 401/403), then permissions.

use std::collections::{HashMap, HashSet};

use axum::body::Bytes;
use axum::extract::{Path, State};
use pl_authz::owned::ObjectGrantees;
use pl_authz::{AuthUser, User};
use pl_core::pagination::{self, PageRequest};
use pl_core::{AppState, RequestContext};
use sea_query::{Alias, Expr, SelectStatement};
use serde_json::{Value, json};

use crate::drf::{self, AuditMeta, ClientAddr, HResult};
use crate::listing::{self, ALIAS, FilterData, OrderTerm};
use crate::model::{Kind, ObjectRow};
use crate::tagtree::TagTree;
use crate::objects;
use crate::perms;
use crate::serialize::{self, ChangeSource, SerCtx};
use crate::write;

/// GET list.
pub async fn list(state: &AppState, kind: Kind, auth: &AuthUser, ctx: &RequestContext) -> HResult {
    let user = objects::check_model_perm(auth, kind)?;
    let db = state.db();
    let with_last = kind == Kind::Correspondent && drf::param(ctx, "last_correspondence").is_some_and(|v| !v.is_empty());
    let terms = listing::ordering_terms(kind, drf::param(ctx, "ordering"), kind == Kind::Correspondent);
    // PERF-SCALE #3b: the count aggregation only joins the list query when the ordering needs it;
    // otherwise the counts are fetched for the shown objects only (same values, same filter).
    let in_query = listing::ordering_needs_counts(&terms);
    let counters = in_query && crate::counters::available(db).await;
    let mut q = listing::select(kind, user, in_query, with_last && in_query, counters);
    listing::apply_filters(&mut q, kind, &FilterData::Query(ctx.query.pairs()))?;
    listing::restrict_visible(&mut q, kind, user, kind.codename("view"));
    listing::apply_ordering(&mut q, &terms, with_last && in_query)?;

    if kind == Kind::Tag {
        return tag_list(state, user, ctx, q, &terms, in_query).await;
    }

    let req = PageRequest::from_query(&ctx.query);
    let count = pagination::count(db, &q).await?;
    let page = req.resolve(count)?;
    let mut pq = q.clone();
    pq.limit(page.page_size).offset(page.offset());
    let mut rows = listing::fetch(db, kind, &pq, in_query, with_last && in_query).await?;
    if !in_query {
        listing::fill_counts(db, kind, user, &mut rows, with_last).await?;
    }
    let all = if ctx.version().includes_all() {
        Some(listing::fetch(db, kind, &q, in_query, with_last && in_query).await?.iter().map(|r| json!(r.id)).collect())
    } else {
        None
    };
    // BulkPermissionMixin: grantees of the page objects
    let page_pks: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let batch = pl_authz::owned::get_permission_grantees(db, kind.model(), &page_pks).await.map_err(drf::auth)?;
    let sctx = SerCtx { user, full_perms: drf::full_perms(ctx), change: ChangeSource::Batch(&batch), grantees: &batch, children: None };
    let results = rows.iter().map(|r| serialize::serialize(kind, r, &sctx)).collect();
    Ok(drf::ok(pagination::paginated_body(ctx, &page, results, all)))
}

/// `TagViewSet.list` (children map, `display_count`, legacy `all`).
async fn tag_list(state: &AppState, user: &User, ctx: &RequestContext, q: SelectStatement, terms: &Option<Vec<OrderTerm>>, need_count: bool) -> HResult {
    let db = state.db();
    // need_count: ordering by document_count → counts inside the query (all tags); otherwise
    // counted afterwards for the shown tags and their children only (PERF-SCALE #3b)
    let all_tags = listing::fetch(db, Kind::Tag, &q, need_count, false).await?;
    // Tag.get_hierarchy().descendants(all_tags) – one snapshot of the parent edges (upstream #14612)
    let descendant_pks: HashSet<i64> = if all_tags.is_empty() {
        HashSet::new()
    } else {
        TagTree::load_pool(db).await?.descendants(all_tags.iter().map(|t| t.id))
    };
    let children_source: Vec<ObjectRow> = if !descendant_pks.is_empty() {
        let mut ids: HashSet<i64> = descendant_pks.clone();
        ids.extend(all_tags.iter().map(|t| t.id));
        let mut ids: Vec<i64> = ids.into_iter().collect();
        ids.sort_unstable();
        let counters = need_count && crate::counters::available(db).await;
        let mut sq = listing::select_counted(Kind::Tag, user, need_count, false, Some(&ids), counters);
        sq.and_where(Expr::col((Alias::new(ALIAS), Alias::new("id"))).is_in(ids));
        // .filter(pk__in=permitted_object_ids(user, Tag, "view_tag")) – for every user
        sq.cond_where(perms::permitted_ids_cond(user, Kind::Tag, "view_tag", ALIAS));
        listing::apply_ordering(&mut sq, terms, false)?;
        listing::fetch(db, Kind::Tag, &sq, need_count, false).await?
    } else {
        all_tags.clone()
    };
    let mut children_map: HashMap<i64, Vec<ObjectRow>> = HashMap::new();
    for t in &children_source {
        if let Some(p) = t.tag.as_ref().and_then(|x| x.tn_parent_id) {
            children_map.entry(p).or_default().push(t.clone());
        }
    }

    let page = PageRequest::from_query(&ctx.query).resolve(all_tags.len() as i64)?;
    let start = (page.offset() as usize).min(all_tags.len());
    let end = (start + page.page_size as usize).min(all_tags.len());
    let mut rows: Vec<ObjectRow> = all_tags[start..end].to_vec();
    if !need_count {
        // document_count of the shown tags and of every child reachable from them (the count is
        // per tag, independent of where it appears; same annotation as upstream)
        let shown = serialize::all_pks(&rows, Some(&children_map));
        let counts = listing::fetch_counts(db, Kind::Tag, user, &shown).await?;
        let count_of = |id: i64| Some(counts.get(&id).copied().unwrap_or(0));
        for r in &mut rows {
            r.document_count = count_of(r.id);
        }
        for kids in children_map.values_mut() {
            for k in kids {
                k.document_count = count_of(k.id);
            }
        }
    }

    let page_pks: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let batch = pl_authz::owned::get_permission_grantees(db, Kind::Tag.model(), &page_pks).await.map_err(drf::auth)?;
    let full_perms = drf::full_perms(ctx);
    let grantees: HashMap<i64, ObjectGrantees> = if full_perms {
        let pks = serialize::all_pks(&rows, Some(&children_map));
        pl_authz::owned::get_permission_grantees(db, Kind::Tag.model(), &pks).await.map_err(drf::auth)?
    } else {
        HashMap::new()
    };
    let sctx = SerCtx { user, full_perms, change: ChangeSource::Batch(&batch), grantees: &grantees, children: Some(&children_map) };
    let results = rows.iter().map(|r| serialize::serialize(Kind::Tag, r, &sctx)).collect();
    let all = if ctx.version().includes_all() {
        let src = if descendant_pks.is_empty() { &all_tags } else { &children_source };
        Some(src.iter().map(|t| json!(t.id)).collect())
    } else {
        None
    };
    let mut body = pagination::paginated_body(ctx, &page, results, all);
    if let Value::Object(m) = &mut body {
        m.insert("display_count".into(), json!(children_source.len()));
    }
    Ok(drf::ok(body))
}

/// GET detail.
pub async fn retrieve(state: &AppState, kind: Kind, auth: &AuthUser, ctx: &RequestContext, pk: &str) -> HResult {
    let user = objects::check_model_perm(auth, kind)?;
    let pk = objects::parse_pk(pk)?;
    let db = state.db();
    let row = objects::get_object(db, kind, user, pk, kind == Kind::Correspondent).await?;
    objects::check_object_perm(db, auth, kind, &row).await?;
    let out = objects::serialize_single(db, kind, user, ctx, &row).await?;
    Ok(drf::ok(out))
}

/// POST create.
pub async fn create(state: &AppState, kind: Kind, auth: &AuthUser, ctx: &RequestContext, meta: &AuditMeta, body: &Bytes) -> HResult {
    let user = objects::check_model_perm(auth, kind)?;
    let data = drf::parse_body(ctx, body, kind, false)?;
    write::create(state, kind, user, ctx, meta, &data).await
}

/// PUT / PATCH.
#[allow(clippy::too_many_arguments)]
pub async fn update(state: &AppState, kind: Kind, auth: &AuthUser, ctx: &RequestContext, meta: &AuditMeta, pk: &str, body: &Bytes, partial: bool) -> HResult {
    let user = objects::check_model_perm(auth, kind)?;
    let pk = objects::parse_pk(pk)?;
    let db = state.db();
    let row = objects::get_object(db, kind, user, pk, false).await?;
    objects::check_object_perm(db, auth, kind, &row).await?;
    let data = drf::parse_body(ctx, body, kind, partial)?;
    write::update(state, kind, user, ctx, meta, row, &data, partial).await
}

/// DELETE.
pub async fn destroy(state: &AppState, kind: Kind, auth: &AuthUser, meta: &AuditMeta, pk: &str) -> HResult {
    let user = objects::check_model_perm(auth, kind)?;
    let pk = objects::parse_pk(pk)?;
    let db = state.db();
    let row = objects::get_object(db, kind, user, pk, false).await?;
    objects::check_object_perm(db, auth, kind, &row).await?;
    if kind == Kind::Tag {
        // TagViewSet.perform_destroy (upstream fix fa83a4c5d, after v3.4.2)
        objects::check_tag_descendants_deletable(db, user, row.id).await?;
    }
    write::destroy(state, kind, user, meta, &row).await
}

// ----------------------------------------------------------------------------------------------
// axum glue: one handler per (kind, action)
// ----------------------------------------------------------------------------------------------

macro_rules! kind_handlers {
    ($modname:ident, $kind:expr) => {
        pub mod $modname {
            use super::*;
            pub async fn list(State(s): State<AppState>, ctx: RequestContext, auth: AuthUser) -> HResult {
                super::list(&s, $kind, &auth, &ctx).await
            }
            pub async fn retrieve(State(s): State<AppState>, ctx: RequestContext, auth: AuthUser, Path(pk): Path<String>) -> HResult {
                super::retrieve(&s, $kind, &auth, &ctx, &pk).await
            }
            pub async fn create(State(s): State<AppState>, ctx: RequestContext, auth: AuthUser, client: ClientAddr, body: Bytes) -> HResult {
                let meta = AuditMeta::new(&ctx, &client, &auth);
                super::create(&s, $kind, &auth, &ctx, &meta, &body).await
            }
            pub async fn update(
                State(s): State<AppState>,
                ctx: RequestContext,
                auth: AuthUser,
                client: ClientAddr,
                Path(pk): Path<String>,
                body: Bytes,
            ) -> HResult {
                let meta = AuditMeta::new(&ctx, &client, &auth);
                super::update(&s, $kind, &auth, &ctx, &meta, &pk, &body, false).await
            }
            pub async fn partial_update(
                State(s): State<AppState>,
                ctx: RequestContext,
                auth: AuthUser,
                client: ClientAddr,
                Path(pk): Path<String>,
                body: Bytes,
            ) -> HResult {
                let meta = AuditMeta::new(&ctx, &client, &auth);
                super::update(&s, $kind, &auth, &ctx, &meta, &pk, &body, true).await
            }
            pub async fn destroy(State(s): State<AppState>, ctx: RequestContext, auth: AuthUser, client: ClientAddr, Path(pk): Path<String>) -> HResult {
                let meta = AuditMeta::new(&ctx, &client, &auth);
                super::destroy(&s, $kind, &auth, &meta, &pk).await
            }
        }
    };
}

kind_handlers!(correspondents, Kind::Correspondent);
kind_handlers!(document_types, Kind::DocumentType);
kind_handlers!(tags, Kind::Tag);
kind_handlers!(storage_paths, Kind::StoragePath);
