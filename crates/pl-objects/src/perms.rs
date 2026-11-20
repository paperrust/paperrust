//! Permission-dependent query pieces (built on pl-authz).
//!
//! upstream: src/documents/permissions.py@v3.4.2 (get_document_count_filter_for_user,
//!           annotate_document_count_for_related_queryset, permitted_object_ids),
//!           src/documents/filters.py@v3.4.2 (PermittedObjectsFilter)

use pl_authz::User;
use pl_authz::objects::{self as authz_objects, FilterOpts, PermOpts};
use sea_query::{Alias, Condition, Expr, Func, Query, SelectStatement};

use crate::model::Kind;

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// `PermittedObjectsFilter` with `perm_codename` (default view) on the outer alias.
pub fn visible_cond(user: &User, kind: Kind, codename: &'static str, alias: &str) -> Condition {
    authz_objects::permitted_objects_filter(
        Some(user),
        kind.model(),
        alias,
        &FilterOpts { include_granted: true, perm_codename: Some(codename), parent_field: None },
    )
}

/// `.filter(pk__in=permitted_object_ids(user, model, perm))` (no superuser shortcut in the caller).
pub fn permitted_ids_cond(user: &User, kind: Kind, codename: &str, alias: &str) -> Condition {
    Condition::all().add(
        Expr::col((a(alias), a("id")))
            .in_subquery(authz_objects::permitted_object_ids(Some(user), kind.model(), codename, &PermOpts::default())),
    )
}

/// `restrict_queryset_to_visible(qs, user, perm)`.
pub fn restrict_visible_cond(user: &User, kind: Kind, codename: &str, alias: &str) -> Condition {
    authz_objects::restrict_to_visible(Some(user), kind.model(), codename, alias)
}

/// `alias.id IN permitted_document_ids(user, "view_document")` as a condition on the document
/// row itself (alias of `documents_document`), for the counts – same set, without the generic
/// form's `LEFT JOIN documents_document <parent>` self-join and second scan (PERF-SCALE #3c / Q7).
///
/// Upstream authorizes a row by `authorizing_owner = CASE WHEN root IS NULL THEN owner ELSE
/// parent.owner END` and `authorizing_id = COALESCE(root, id)` (manager: not deleted). Split by
/// `root_document_id`:
/// * root rows (`root_document_id IS NULL`): owner/key are the row's own → plain row predicate;
/// * versions: `EXISTS` on their root (PK lookup) with the same predicate on the root's owner/id –
///   only version rows evaluate it. The FK guarantees the root exists (LEFT JOIN = EXISTS).
///
/// Superusers: every non-deleted row (the generic form joins the parent without using it).
/// Inactive users: nothing. The grant test is `pl_authz::objects::granted_key_cond` (the L5 swap
/// point: guardian `object_pk` text keys or `paperrust_grant`).
pub fn permitted_doc_row(user: &User, alias: &str) -> Condition {
    let col = |c: &str| Expr::col((a(alias), a(c)));
    let base = Condition::all().add(col("deleted_at").is_null());
    match permitted_doc_split(user, alias) {
        DocRowSplit::Nothing => base.add(Expr::cust("FALSE")),
        DocRowSplit::All => base,
        DocRowSplit::Split { roots, versions } => base.add(Condition::any().add(roots).add(versions)),
    }
}

/// [`permitted_doc_row`] (without the `deleted_at` test) split into its two disjoint halves.
pub enum DocRowSplit {
    /// inactive user
    Nothing,
    /// superuser
    All,
    /// `roots`: `root_document_id IS NULL` + own owner/grant; `versions`: `root_document_id IS NOT
    /// NULL` + `EXISTS` on the root.
    Split { roots: Condition, versions: Condition },
}

/// See [`DocRowSplit`].
pub fn permitted_doc_split(user: &User, alias: &str) -> DocRowSplit {
    let col = |c: &str| Expr::col((a(alias), a(c)));
    if !user.is_active {
        return DocRowSplit::Nothing;
    }
    if user.is_superuser {
        return DocRowSplit::All;
    }
    let owner_vis = |al: &str| {
        let owner = Expr::col((a(al), a("owner_id")));
        Condition::any().add(owner.clone().eq(user.id)).add(owner.is_null()).add(authz_objects::granted_key_cond(
            user,
            &pl_authz::models::DOCUMENT,
            "view_document",
            Expr::col((a(al), a("id"))).into(),
        ))
    };
    let mut root = Query::select();
    root.expr(Expr::cust("1"))
        .from_as(a("documents_document"), a("pl_pr"))
        .and_where(Expr::col((a("pl_pr"), a("id"))).equals((a(alias), a("root_document_id"))))
        .cond_where(owner_vis("pl_pr"));
    DocRowSplit::Split {
        roots: Condition::all().add(col("root_document_id").is_null()).add(owner_vis(alias)),
        versions: Condition::all().add(col("root_document_id").is_not_null()).add(Expr::exists(root)),
    }
}

/// Alias of the grouped count table joined by [`crate::listing::select`].
pub const COUNT_ALIAS: &str = "pl_dc";

/// The `document_count` (and for correspondents `last_correspondence`) annotation as **one**
/// grouped query over the counted relation, keyed by the object id (`k`, `n`[, `lc`]); the
/// caller LEFT JOINs it on `k = o.id` and reads `COALESCE(n, 0)`.
///
/// upstream v3.4.2: FK models `Count("documents", filter=get_document_count_filter_for_user(user),
/// distinct=True)` (LEFT JOIN + GROUP BY), Tag `annotate_document_count_for_related_queryset`
/// (one `GROUP BY tag_id` over the through table with `document_id IN permitted_document_ids`).
/// Same filter as the former per-row correlated subquery (Phase 6, docs/contract/pgvalidate.md:
/// that form was 5-60x slower at 100k documents). `ids`: restrict the aggregation to these
/// objects (upstream: `tag_id__in=queryset.values("pk")`), for callers with a known small set.
/// `with_last`: also `MAX(created)` with the same filter (`last_correspondence`).
///
/// Without `ids` and for non-superusers the aggregation is restricted to the objects visible to
/// the user (`PermittedObjectsFilter` with `view_*`, the same restriction the list applies to the
/// outer rows), so a user who sees only a few objects does not pay for counting all of them.
pub fn document_count_source(user: &User, kind: Kind, ids: Option<&[i64]>, with_last: bool) -> SelectStatement {
    document_count_source_ext(user, kind, ids, with_last, false)
}

/// [`document_count_source`]; with `counters` (data model stage 2 applied, see
/// [`crate::counters::available`]) and without `last_correspondence` from the label counters
/// (same values, [`crate::counters`]).
pub fn document_count_source_ext(user: &User, kind: Kind, ids: Option<&[i64]>, with_last: bool, counters: bool) -> SelectStatement {
    if counters
        && !with_last
        && let Some(q) = crate::counters::count_source(user, kind, ids)
    {
        return q;
    }
    let mut q = Query::select();
    let key_col = match kind.document_fk() {
        Some(fk) => (a("cd"), a(fk)),
        None => (a("dt"), a("tag_id")),
    };
    let prefilter = (ids.is_none() && !user.is_superuser).then(|| {
        let mut vis = Query::select();
        vis.column((a("pl_dco"), a("id")))
            .from_as(a(kind.table()), a("pl_dco"))
            .cond_where(visible_cond(user, kind, kind.codename("view"), "pl_dco"));
        vis
    });
    if let Some(vis) = &prefilter
        && (kind.document_fk().is_none() || !user.is_active)
    {
        q.and_where(Expr::col(key_col.clone()).in_subquery(vis.clone()));
    }
    match kind.document_fk() {
        // Count("documents", filter=get_document_count_filter_for_user(user), distinct=True)
        Some(fk) if !user.is_superuser && user.is_active => {
            // Two disjoint aggregations (root rows / versions), summed per object. In one query the
            // planner costs the versions' EXISTS sub-plan for every row and picks an index scan in
            // FK order (1 Mio., restricted, correspondents by count: 9.9 s); split, the root half
            // is a seq scan + hash aggregate (0.86 s) and the version half uses the root index.
            let DocRowSplit::Split { roots, versions } = permitted_doc_split(user, "cd") else {
                return q;
            };
            let half = |cond: Condition| {
                let mut h = Query::select();
                h.expr_as(Expr::col((a("cd"), a(fk))), a("k"))
                    .expr_as(Func::count(Expr::col((a("cd"), a("id")))), a("n"))
                    .from_as(a("documents_document"), a("cd"))
                    .and_where(Expr::col((a("cd"), a(fk))).is_not_null())
                    .and_where(Expr::col((a("cd"), a("deleted_at"))).is_null())
                    .cond_where(cond);
                if with_last {
                    h.expr_as(Func::max(Expr::col((a("cd"), a("created")))), a("lc"));
                }
                if let Some(ids) = ids {
                    h.and_where(Expr::col((a("cd"), a(fk))).is_in(ids.iter().copied()));
                }
                if let Some(vis) = &prefilter {
                    h.and_where(Expr::col((a("cd"), a(fk))).in_subquery(vis.clone()));
                }
                h.group_by_col((a("cd"), a(fk)));
                h
            };
            let mut u = half(roots);
            u.union(sea_query::UnionType::All, half(versions));
            let mut out = Query::select();
            out.expr_as(Expr::col((a("pl_dcu"), a("k"))), a("k"))
                .expr_as(Expr::cust(r#"SUM("pl_dcu"."n")::int8"#), a("n"))
                .from_subquery(u, a("pl_dcu"))
                .group_by_col((a("pl_dcu"), a("k")));
            if with_last {
                out.expr_as(Func::max(Expr::col((a("pl_dcu"), a("lc")))), a("lc"));
            }
            return out;
        }
        Some(fk) => {
            q.expr_as(Expr::col((a("cd"), a(fk))), a("k"))
                .expr_as(Func::count(Expr::col((a("cd"), a("id")))), a("n"))
                .from_as(a("documents_document"), a("cd"))
                .and_where(Expr::col((a("cd"), a(fk))).is_not_null());
            if with_last {
                // Max("documents__created", filter=self.get_document_count_filter())
                q.expr_as(Func::max(Expr::col((a("cd"), a("created")))), a("lc"));
            }
            if user.is_superuser {
                // get_document_count_filter_for_user: `is_superuser` (also when inactive)
                q.and_where(Expr::col((a("cd"), a("deleted_at"))).is_null());
            } else {
                q.cond_where(permitted_doc_row(user, "cd"));
            }
            if let Some(ids) = ids {
                q.and_where(Expr::col((a("cd"), a(fk))).is_in(ids.iter().copied()));
            }
            q.group_by_col((a("cd"), a(fk)));
        }
        // annotate_document_count_for_related_queryset(Document.tags.through, "tag_id", user)
        None => {
            q.expr_as(Expr::col((a("dt"), a("tag_id"))), a("k"))
                // COUNT(*) = COUNT(DISTINCT document_id): (document_id, tag_id) is UNIQUE and the
                // permitted set is a semi-join (PERF-SCALE Q2, no sort spill)
                .expr_as(Expr::cust("COUNT(*)"), a("n"))
                .from_as(a("documents_document_tags"), a("dt"))
                // `document_id IN permitted_document_ids(user)` as a 1:1 join on the primary key
                .join_as(
                    sea_query::JoinType::InnerJoin,
                    a("documents_document"),
                    a("cd"),
                    Expr::col((a("cd"), a("id"))).equals((a("dt"), a("document_id"))),
                )
                .cond_where(permitted_doc_row(user, "cd"));
            if let Some(ids) = ids {
                q.and_where(Expr::col((a("dt"), a("tag_id"))).is_in(ids.iter().copied()));
            }
            q.group_by_col((a("dt"), a("tag_id")));
        }
    }
    q
}

/// `SELECT id FROM django_content_type WHERE app_label='documents' AND model='<model>'` as SQL text
/// (constant identifiers only) for raw INSERT statements.
pub fn ctype_sql(kind: Kind) -> String {
    format!("(SELECT id FROM django_content_type WHERE app_label = 'documents' AND model = '{}')", kind.model_name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::PostgresQueryBuilder;

    #[test]
    fn fk_count_split_for_restricted_users() {
        let r = document_count_source(&user(false, true), Kind::Correspondent, None, true).to_string(PostgresQueryBuilder);
        assert!(r.contains("UNION ALL"), "{r}");
        assert!(r.contains(r#"SUM("pl_dcu"."n")::int8"#), "{r}");
        assert!(r.contains(r#""root_document_id" IS NULL"#) && r.contains(r#""root_document_id" IS NOT NULL"#));
        assert!(r.contains(r#"MAX("pl_dcu"."lc")"#));
        let su = document_count_source(&user(true, true), Kind::Correspondent, None, false).to_string(PostgresQueryBuilder);
        assert!(!su.contains("pl_dcu"), "{su}");
        let tag = document_count_source(&user(false, true), Kind::Tag, None, false).to_string(PostgresQueryBuilder);
        assert!(!tag.contains("pl_dcu"), "{tag}");
    }

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

    fn sql(u: &User, kind: Kind, ids: Option<&[i64]>) -> String {
        document_count_source(u, kind, ids, false).to_string(PostgresQueryBuilder)
    }

    #[test]
    fn counts_without_self_join_and_distinct() {
        let s = sql(&user(false, true), Kind::Tag, Some(&[1, 2]));
        assert!(s.contains("COUNT(*)"), "{s}");
        assert!(!s.to_uppercase().contains("DISTINCT"), "{s}");
        assert!(!s.contains("LEFT JOIN"), "{s}");
        assert!(s.contains(r#""cd"."root_document_id" IS NULL"#), "{s}");
        assert!(s.contains(r#"EXISTS(SELECT 1 FROM "documents_document" AS "pl_pr""#), "{s}");
        assert!(s.contains(r#""cd"."owner_id" = 7"#) && s.contains(r#""pl_pr"."owner_id" = 7"#), "{s}");
        assert!(s.contains(r#""dt"."tag_id" IN (1, 2)"#), "{s}");
    }

    #[test]
    fn superuser_and_inactive() {
        let su = sql(&user(true, true), Kind::Correspondent, None);
        assert!(su.contains(r#""cd"."deleted_at" IS NULL"#) && !su.contains("EXISTS"), "{su}");
        let tag = sql(&user(true, true), Kind::Tag, None);
        assert!(tag.contains(r#""cd"."deleted_at" IS NULL"#) && !tag.contains("EXISTS"), "{tag}");
        let inactive = sql(&user(false, false), Kind::Tag, None);
        assert!(inactive.contains("FALSE"), "{inactive}");
    }
}
