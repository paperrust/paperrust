//! `document_count` from the counters of the own data model (stage 2, docs/DATAMODEL-V2.md §1.5,
//! migration `pl_store.dm2_paperrust_tag_count`, contract docs/contract/pl-store.md "Stufe 2").
//!
//! `paperrust_label_count_live (kind, label_id, owner_bucket, n)` counts non-deleted documents
//! (versions included) per label and "visibility owner" (owner of the root, 0 = none).
//! Same set as [`crate::perms::document_count_source`] (upstream
//! `get_document_count_filter_for_user` / `annotate_document_count_for_related_queryset`):
//!
//! * Superuser (even inactive): sum over all buckets.
//! * active restricted user: buckets `{uid, 0}` (own and ownerless documents or
//!   roots) **plus** the shared foreign documents, counted live, but only via the
//!   user's shares (`granted_key_cond` on the root id; typically few instead of 1 million).
//! * inactive user: the previous path (counts nothing).
//!
//! `last_correspondence` (max over `created`) cannot be maintained incrementally → the
//! previous path stays for it. Likewise without a counter table (database before stage 2).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use pl_authz::User;
use pl_authz::objects as authz_objects;
use sea_query::{Alias, Expr, Query, SelectStatement, UnionType};
use sqlx::PgPool;

use crate::model::Kind;

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// Kind in the counter (as in SYNC.md / pl_sync: 2 tag, 3 correspondent, 4 document_type, 5 storage_path).
pub fn counter_kind(kind: Kind) -> i16 {
    match kind {
        Kind::Tag => 2,
        Kind::Correspondent => 3,
        Kind::DocumentType => 4,
        Kind::StoragePath => 5,
    }
}

/// Do the counters exist (stage 2 applied)? Cached per database for 60 s.
pub async fn available(db: &PgPool) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, bool)>>> = OnceLock::new();
    let key = db.connect_options().get_database().unwrap_or_default().to_string();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, v)) = cache.lock().ok().and_then(|m| m.get(&key).copied())
        && at.elapsed() < Duration::from_secs(60)
    {
        return v;
    }
    let v =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('paperrust_label_count_live') IS NOT NULL").fetch_one(db).await.unwrap_or(false);
    if let Ok(mut m) = cache.lock() {
        m.insert(key, (Instant::now(), v));
    }
    if v {
        maybe_fold(db);
    }
    v
}

/// Fold differences in the background once enough have accumulated (at most every
/// 10 s per process; one run at a time via the advisory lock in `paperrust_label_count_fold`).
fn maybe_fold(db: &PgPool) {
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let last = LAST.get_or_init(|| Mutex::new(None));
    let due = match last.lock() {
        Ok(mut g) => {
            let due = g.is_none_or(|t| t.elapsed() > Duration::from_secs(10));
            if due {
                *g = Some(Instant::now());
            }
            due
        }
        Err(_) => false,
    };
    if !due || tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let db = db.clone();
    tokio::spawn(async move {
        let many: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM paperrust_label_count_delta OFFSET 1000)")
            .fetch_one(&db)
            .await
            .unwrap_or(false);
        if many {
            let _ = sqlx::query("SELECT paperrust_label_count_fold()").execute(&db).await;
        }
    });
}

/// Count source `(k, n)` via the counters; `None` = take the previous path (inactive user).
pub fn count_source(user: &User, kind: Kind, ids: Option<&[i64]>) -> Option<SelectStatement> {
    if !user.is_superuser && !user.is_active {
        return None;
    }
    let ck = counter_kind(kind);
    let mut buckets = Query::select();
    buckets
        .expr_as(Expr::col((a("lc"), a("label_id"))), a("k"))
        .expr_as(Expr::col((a("lc"), a("n"))), a("n"))
        .from_as(a("paperrust_label_count_live"), a("lc"))
        .and_where(Expr::col((a("lc"), a("kind"))).eq(ck));
    if let Some(ids) = ids {
        buckets.and_where(Expr::col((a("lc"), a("label_id"))).is_in(ids.iter().copied()));
    }
    if !user.is_superuser {
        let uid = i32::try_from(user.id).unwrap_or(-1);
        buckets.and_where(Expr::col((a("lc"), a("owner_bucket"))).is_in([uid, 0]));
        // shared foreign documents: roots and versions separately (one hash semi-join each over
        // the user's shares)
        buckets.union(UnionType::All, granted_part(user, kind, ids, false));
        buckets.union(UnionType::All, granted_part(user, kind, ids, true));
    }
    let mut out = Query::select();
    out.expr_as(Expr::col((a("pl_cnt"), a("k"))), a("k"))
        .expr_as(Expr::cust(r#"SUM("pl_cnt"."n")::int8"#), a("n"))
        .from_subquery(buckets, a("pl_cnt"))
        .group_by_col((a("pl_cnt"), a("k")));
    Some(out)
}

/// Non-deleted documents with a label whose root belongs to another user and is shared with the user
/// (or their group) with `view_document`.
fn granted_part(user: &User, kind: Kind, ids: Option<&[i64]>, versions: bool) -> SelectStatement {
    let uid = i32::try_from(user.id).unwrap_or(-1);
    let mut q = Query::select();
    q.from_as(a("paperrust_document"), a("pd")).and_where(Expr::col((a("pd"), a("deleted_at"))).is_null());
    // Root row: for versions the linked root, otherwise the document itself
    let root = if versions {
        // explicitly, so that the partial index paperrust_doc_root (few versions) drives the query
        q.and_where(Expr::col((a("pd"), a("root_document_id"))).is_not_null());
        q.join_as(
            sea_query::JoinType::InnerJoin,
            a("paperrust_document"),
            a("pr"),
            Expr::col((a("pr"), a("id"))).equals((a("pd"), a("root_document_id"))),
        );
        "pr"
    } else {
        q.and_where(Expr::col((a("pd"), a("root_document_id"))).is_null());
        "pd"
    };
    q.and_where(Expr::col((a(root), a("owner_id"))).is_not_null()).and_where(Expr::col((a(root), a("owner_id"))).ne(uid)).and_where(
        authz_objects::granted_key_cond(user, &pl_authz::models::DOCUMENT, "view_document", Expr::col((a(root), a("id"))).into()),
    );
    let key = match kind.document_fk() {
        Some(fk) => {
            q.and_where(Expr::col((a("pd"), a(fk))).is_not_null());
            Expr::col((a("pd"), a(fk)))
        }
        None => {
            q.join_as(
                sea_query::JoinType::InnerJoin,
                a("paperrust_document_tag"),
                a("pdt"),
                Expr::col((a("pdt"), a("document_id"))).equals((a("pd"), a("id"))),
            );
            Expr::col((a("pdt"), a("tag_id")))
        }
    };
    if let Some(ids) = ids {
        q.and_where(key.clone().is_in(ids.iter().copied()));
    }
    q.expr_as(key.clone(), a("k")).expr_as(Expr::cust("COUNT(*)::int8"), a("n")).group_by_col(match kind.document_fk() {
        Some(fk) => (a("pd"), a(fk)),
        None => (a("pdt"), a("tag_id")),
    });
    q
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::PostgresQueryBuilder;

    fn user(su: bool, active: bool) -> User {
        User {
            id: 7,
            username: "u".into(),
            password: String::new(),
            first_name: String::new(),
            last_name: String::new(),
            email: String::new(),
            is_active: active,
            is_staff: false,
            is_superuser: su,
            last_login: None,
            date_joined: chrono::Utc::now(),
            groups: vec![3],
            permissions: Default::default(),
        }
    }

    #[test]
    fn superuser_sums_all_buckets() {
        let s = count_source(&user(true, false), Kind::Tag, None).expect("counter").to_string(PostgresQueryBuilder);
        assert!(s.contains("paperrust_label_count_live") && !s.contains("owner_bucket"), "{s}");
    }

    #[test]
    fn restricted_adds_granted_parts() {
        let s = count_source(&user(false, true), Kind::Correspondent, Some(&[1, 2])).expect("counter").to_string(PostgresQueryBuilder);
        assert!(s.contains(r#""owner_bucket" IN (7, 0)"#), "{s}");
        assert!(s.matches("UNION ALL").count() >= 2, "{s}");
        assert!(s.contains(r#""pr"."id" = "pd"."root_document_id""#), "{s}");
        assert!(count_source(&user(false, false), Kind::Tag, None).is_none());
    }
}
