//! `document_count` of the custom fields from the label counters (kind 6) of the own data model
//! (stage 3, docs/DATAMODEL-V2.md §1.5/§1.7, contract docs/contract/pl-store.md "Stage 3").
//!
//! Same set as `custom_fields::count_select` (non-deleted field values of visible, non-
//! deleted documents; versions per the permissions of the root): active superuser = sum of all
//! owner buckets, active restricted user = bucket `{uid, 0}` + live over the foreign documents
//! shared with them; inactive users and databases before stage 3: previous way.
//! Columns as there: `field_id`, `c`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use pl_authz::User;
use pl_authz::models::DOCUMENT;
use pl_authz::objects::granted_key_cond;
use sea_query::{Alias, Expr, JoinType, Query, SelectStatement, UnionType};
use sqlx::PgPool;

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// Are there counters of kind 6 (stage 3 applied)? Cached for 60 s per database.
pub async fn available(db: &PgPool) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, bool)>>> = OnceLock::new();
    let key = db.connect_options().get_database().unwrap_or_default().to_string();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, v)) = cache.lock().ok().and_then(|m| m.get(&key).copied())
        && at.elapsed() < Duration::from_secs(60)
    {
        return v;
    }
    let v = sqlx::query_scalar::<_, bool>(
        "SELECT to_regclass('paperrust_label_count_live') IS NOT NULL AND to_regclass('paperrust_field_value') IS NOT NULL",
    )
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if let Ok(mut m) = cache.lock() {
        m.insert(key, (Instant::now(), v));
    }
    v
}

/// Count source `(field_id, c)`; `None` = take the previous way (inactive user).
pub fn count_source(user: &User, ids: Option<&[i64]>) -> Option<SelectStatement> {
    if !user.is_active {
        return None;
    }
    let mut buckets = Query::select();
    buckets
        .expr_as(Expr::col((a("lc"), a("label_id"))), a("field_id"))
        .expr_as(Expr::col((a("lc"), a("n"))), a("c"))
        .from_as(a("paperrust_label_count_live"), a("lc"))
        .and_where(Expr::col((a("lc"), a("kind"))).eq(6));
    if let Some(ids) = ids {
        buckets.and_where(Expr::col((a("lc"), a("label_id"))).is_in(ids.iter().copied()));
    }
    if !user.is_superuser {
        let uid = i32::try_from(user.id).unwrap_or(-1);
        buckets.and_where(Expr::col((a("lc"), a("owner_bucket"))).is_in([uid, 0]));
        buckets.union(UnionType::All, granted_part(user, ids, false));
        buckets.union(UnionType::All, granted_part(user, ids, true));
    }
    let mut out = Query::select();
    out.expr_as(Expr::col((a("pl_cfc"), a("field_id"))), a("field_id"))
        .expr_as(Expr::cust(r#"SUM("pl_cfc"."c")::int8"#), a("c"))
        .from_subquery(buckets, a("pl_cfc"))
        .group_by_col((a("pl_cfc"), a("field_id")));
    Some(out)
}

/// Non-deleted field values of non-deleted documents whose root belongs to another user
/// and is shared with the user (or their group) with `view_document`.
fn granted_part(user: &User, ids: Option<&[i64]>, versions: bool) -> SelectStatement {
    let uid = i32::try_from(user.id).unwrap_or(-1);
    let mut q = Query::select();
    q.from_as(a("paperrust_document"), a("pd"))
        .join_as(
            JoinType::InnerJoin,
            a("paperrust_field_value"),
            a("pfv"),
            Expr::col((a("pfv"), a("document_id"))).equals((a("pd"), a("id"))),
        )
        .and_where(Expr::col((a("pd"), a("deleted_at"))).is_null())
        .and_where(Expr::col((a("pfv"), a("deleted_at"))).is_null());
    let root = if versions {
        q.and_where(Expr::col((a("pd"), a("root_document_id"))).is_not_null()).join_as(
            JoinType::InnerJoin,
            a("paperrust_document"),
            a("pr"),
            Expr::col((a("pr"), a("id"))).equals((a("pd"), a("root_document_id"))),
        );
        "pr"
    } else {
        q.and_where(Expr::col((a("pd"), a("root_document_id"))).is_null());
        "pd"
    };
    q.and_where(Expr::col((a(root), a("owner_id"))).is_not_null())
        .and_where(Expr::col((a(root), a("owner_id"))).ne(uid))
        .and_where(granted_key_cond(user, &DOCUMENT, "view_document", Expr::col((a(root), a("id"))).into()));
    if let Some(ids) = ids {
        q.and_where(Expr::col((a("pfv"), a("field_id"))).is_in(ids.iter().copied()));
    }
    q.expr_as(Expr::col((a("pfv"), a("field_id"))), a("field_id"))
        .expr_as(Expr::cust("COUNT(*)::int8"), a("c"))
        .group_by_col((a("pfv"), a("field_id")));
    q
}
