//! L5 differential test: permission read path `legacy` (guardian) against `grants` (paperrust_grant) and
//! trigger maintenance of the index against a full recomputation under random changes.
//!
//! Only against a **DB copy** (writes!): `PL_L5_DB=postgres://…/pl_l5_snap`
//! `cargo test -p pl-server --test grants_diff -j 3 -- --ignored --nocapture`
//!
//! Environment: `PL_L5_ROUNDS` (mutation rounds, default 300), `PL_L5_FULL_EVERY` (full read comparisons
//! every N rounds, default 10), `PL_L5_SAMPLE` (objects per model for single checks, default 400),
//! `PL_L5_SEED` (start value, default 5).
//!
//! Contract: docs/contract/pl-authz.md "L5".

use std::collections::{BTreeMap, BTreeSet};

use pl_authz::grants::{self, Engine};
use pl_authz::models::{self, ObjectModel};
use pl_authz::objects::{PermOpts, permitted_object_ids};
use pl_authz::owned::{self, GranteesUpdate, PermissionsSpec};
use pl_authz::{User, perms, user};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use sea_query::{Alias, PostgresQueryBuilder, Query};
use sqlx::PgPool;

fn env_num(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

/// Models with owner + guardian (all kinds that have object permissions).
const MODELS: &[ObjectModel] = &[
    models::DOCUMENT,
    models::TAG,
    models::CORRESPONDENT,
    models::DOCUMENT_TYPE,
    models::STORAGE_PATH,
    models::SAVED_VIEW,
    models::SHARE_LINK,
    models::SHARE_LINK_BUNDLE,
    models::PAPERLESS_TASK,
    models::MAIL_ACCOUNT,
    models::MAIL_RULE,
    models::PROCESSED_MAIL,
];

const ACTIONS: &[&str] = &["view", "change", "delete"];

#[derive(Default, Debug)]
struct Stats {
    set_cmp: u64,
    set_rows: u64,
    checker_cmp: u64,
    grantee_cmp: u64,
    mail_cmp: u64,
    search_cmp: u64,
    mutations: BTreeMap<&'static str, u64>,
    checks: u64,
    full_runs: u64,
    diffs: Vec<String>,
}

async fn ids(db: &PgPool, sql: &str) -> BTreeSet<i64> {
    let rows: Vec<(i64,)> = sqlx::query_as(&format!("SELECT id::int8 FROM ({sql}) s"))
        .fetch_all(db)
        .await
        .unwrap();
    rows.into_iter().map(|r| r.0).collect()
}

async fn users(db: &PgPool) -> Vec<User> {
    let ids: Vec<(i64,)> = sqlx::query_as("SELECT id::int8 FROM auth_user ORDER BY id")
        .fetch_all(db)
        .await
        .unwrap();
    let mut out = Vec::new();
    for (id,) in ids {
        if let Some(u) = user::load_user_by_id(db, id).await.unwrap() {
            out.push(u);
        }
    }
    out
}

async fn table_ids(db: &PgPool, table: &str) -> Vec<i64> {
    let rows: Vec<(i64,)> = sqlx::query_as(&format!("SELECT id::int8 FROM {table} ORDER BY id"))
        .fetch_all(db)
        .await
        .unwrap();
    rows.into_iter().map(|r| r.0).collect()
}

/// Object IDs for single checks: all with a guardian row (incl. non-existent/
/// non-canonical keys, as far as numeric) + a sample from the table.
async fn candidates(db: &PgPool, m: &ObjectModel, rng: &mut StdRng, sample: usize) -> Vec<i64> {
    let g: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT object_pk FROM (SELECT object_pk, content_type_id FROM guardian_userobjectpermission
          UNION ALL SELECT object_pk, content_type_id FROM guardian_groupobjectpermission) x
          WHERE content_type_id IN (SELECT id FROM django_content_type WHERE app_label = $1 AND model = $2)",
    )
    .bind(m.app_label)
    .bind(m.model_name)
    .fetch_all(db)
    .await
    .unwrap();
    let mut set: BTreeSet<i64> = g
        .iter()
        .filter_map(|(s,)| s.trim().parse::<i64>().ok())
        .collect();
    let mut all = table_ids(db, m.table).await;
    all.shuffle(rng);
    set.extend(all.into_iter().take(sample));
    let mut v: Vec<i64> = set.into_iter().collect();
    v.shuffle(rng);
    v.truncate(sample * 2);
    v
}

fn with<T>(e: Engine, f: impl FnOnce() -> T) -> T {
    grants::set_override(Some(e));
    f()
}

fn diff(stats: &mut Stats, what: String) {
    eprintln!("DIFF {what}");
    stats.diffs.push(what);
}

/// Set comparisons: permitted_object_ids (all users + anonymous × models × actions), docsql
/// visibility, `shared_by`, search, mail lists.
async fn compare_sets(db: &PgPool, us: &[User], stats: &mut Stats) {
    let mut subjects: Vec<Option<&User>> = vec![None];
    subjects.extend(us.iter().map(Some));
    for u in &subjects {
        for m in MODELS {
            for a in ACTIONS {
                let perm = m.codename(a);
                let variants: &[(bool, Option<&'static str>)] = if *m == models::DOCUMENT {
                    &[
                        (false, Some(models::DOCUMENT_PARENT_FIELD)),
                        (true, Some(models::DOCUMENT_PARENT_FIELD)),
                        (false, None),
                    ]
                } else {
                    &[(false, None)]
                };
                for (inc, pf) in variants {
                    let opts = PermOpts {
                        include_deleted: *inc,
                        parent_field: *pf,
                    };
                    let l = with(Engine::Legacy, || {
                        permitted_object_ids(*u, m, &perm, &opts).to_string(PostgresQueryBuilder)
                    });
                    let g = with(Engine::Grants, || {
                        permitted_object_ids(*u, m, &perm, &opts).to_string(PostgresQueryBuilder)
                    });
                    let (a1, b1) = (ids(db, &l).await, ids(db, &g).await);
                    stats.set_cmp += 1;
                    stats.set_rows += a1.len() as u64;
                    if a1 != b1 {
                        diff(
                            stats,
                            format!(
                                "permitted_object_ids user={:?} {} {perm} {opts:?}: {} vs {}",
                                u.map(|x| x.id),
                                m.model_name,
                                a1.len(),
                                b1.len()
                            ),
                        );
                    }
                }
            }
        }
        let Some(u) = u else { continue };
        // docsql (list/count/selection/timeline/sync feed): visible_ids_sql
        let l = with(Engine::Legacy, || {
            pl_store::docsql::Visibility::for_user(u).visible_ids_sql()
        });
        let g = with(Engine::Grants, || {
            pl_store::docsql::Visibility::for_user(u).visible_ids_sql()
        });
        let (a1, b1) = (ids(db, &l).await, ids(db, &g).await);
        stats.set_cmp += 1;
        stats.set_rows += a1.len() as u64;
        if a1 != b1 {
            diff(
                stats,
                format!(
                    "docsql visibility user={}: {} vs {}",
                    u.id,
                    a1.len(),
                    b1.len()
                ),
            );
        }
        // pl-search visibility (full text/global search): docs_in_bulk over all documents
        let ru = pl_search::db::RequestUser {
            id: u.id,
            is_superuser: u.is_superuser,
            is_active: u.is_active,
            group_ids: u.groups.clone(),
            perms: u.permissions.clone(),
        };
        let all_docs = table_ids(db, "documents_document").await;
        let mut res = Vec::new();
        for e in [Engine::Legacy, Engine::Grants] {
            grants::set_override(Some(e));
            let mut s = BTreeSet::new();
            for chunk in all_docs.chunks(50_000) {
                for r in pl_search::db::docs_in_bulk(db, &ru, chunk).await.unwrap() {
                    s.insert(r.id);
                }
            }
            res.push(s);
        }
        stats.search_cmp += 1;
        if res[0] != res[1] {
            diff(
                stats,
                format!(
                    "pl-search docs_in_bulk user={}: {} vs {}",
                    u.id,
                    res[0].len(),
                    res[1].len()
                ),
            );
        }
        // pl-mail Listen
        let mu = pl_mail::ctx::MailUser {
            id: u.id as i32,
            is_active: u.is_active,
            is_superuser: u.is_superuser,
            perms: None,
        };
        for mm in [pl_mail::perms::MAIL_ACCOUNT, pl_mail::perms::MAIL_RULE] {
            let all: Vec<i32> = table_ids(db, mm.table)
                .await
                .into_iter()
                .map(|x| x as i32)
                .collect();
            for a in ["view", "change"] {
                let code = format!("{a}_{}", mm.name);
                grants::set_override(Some(Engine::Legacy));
                let x = pl_mail::perms::permitted_among(db, &mu, mm, &code, &all)
                    .await
                    .unwrap();
                grants::set_override(Some(Engine::Grants));
                let y = pl_mail::perms::permitted_among(db, &mu, mm, &code, &all)
                    .await
                    .unwrap();
                stats.mail_cmp += 1;
                if x != y {
                    diff(
                        stats,
                        format!("pl-mail permitted_among user={} {code}", u.id),
                    );
                }
            }
        }
    }
    // shared_by (any share on the document)
    let shared = |e| {
        with(e, || {
            Query::select()
                .column((Alias::new("documents_document"), Alias::new("id")))
                .from(Alias::new("documents_document"))
                .and_where(pl_docfilter::perms::document_is_shared())
                .to_string(PostgresQueryBuilder)
        })
    };
    let (l, g) = (shared(Engine::Legacy), shared(Engine::Grants));
    let (a1, b1) = (ids(db, &l).await, ids(db, &g).await);
    stats.set_cmp += 1;
    if a1 != b1 {
        diff(
            stats,
            format!("document_is_shared: {} vs {}", a1.len(), b1.len()),
        );
    }
}

/// Single checks: guardian checker (pl-authz, pl-mail, pl-search), grantees/shared (owned),
/// user_can_change (pl-mail).
async fn compare_objects(
    db: &PgPool,
    us: &[User],
    rng: &mut StdRng,
    sample: usize,
    stats: &mut Stats,
) {
    for m in MODELS {
        let cands = candidates(db, m, rng, sample).await;
        for u in us {
            for id in &cands {
                for a in ACTIONS {
                    let perm = m.codename(a);
                    grants::set_override(Some(Engine::Legacy));
                    let x = perms::guardian_has_perm(db, u, m, *id, &perm)
                        .await
                        .unwrap();
                    grants::set_override(Some(Engine::Grants));
                    let y = perms::guardian_has_perm(db, u, m, *id, &perm)
                        .await
                        .unwrap();
                    stats.checker_cmp += 1;
                    if x != y {
                        diff(
                            stats,
                            format!(
                                "guardian_has_perm user={} {} {id} {perm}: {x} vs {y}",
                                u.id, m.model_name
                            ),
                        );
                    }
                }
            }
        }
        for chunk in cands.chunks(100) {
            grants::set_override(Some(Engine::Legacy));
            let gl = owned::get_permission_grantees(db, m, chunk).await.unwrap();
            let sl = owned::get_shared_object_pks(db, m, chunk).await.unwrap();
            grants::set_override(Some(Engine::Grants));
            let gg = owned::get_permission_grantees(db, m, chunk).await.unwrap();
            let sg = owned::get_shared_object_pks(db, m, chunk).await.unwrap();
            stats.grantee_cmp += chunk.len() as u64;
            if gl != gg {
                diff(stats, format!("get_permission_grantees {}", m.model_name));
            }
            if sl != sg {
                diff(stats, format!("get_shared_object_pks {}", m.model_name));
            }
        }
        // pl-search more_like (has_perms_owner_aware view_document)
        if *m == models::DOCUMENT {
            for u in us {
                let ru = pl_search::db::RequestUser {
                    id: u.id,
                    is_superuser: u.is_superuser,
                    is_active: u.is_active,
                    group_ids: u.groups.clone(),
                    perms: u.permissions.clone(),
                };
                for id in cands.iter().take(sample) {
                    grants::set_override(Some(Engine::Legacy));
                    let x = pl_search::db::more_like_lookup(db, *id, Some(&ru))
                        .await
                        .unwrap();
                    grants::set_override(Some(Engine::Grants));
                    let y = pl_search::db::more_like_lookup(db, *id, Some(&ru))
                        .await
                        .unwrap();
                    stats.search_cmp += 1;
                    if x != y {
                        diff(
                            stats,
                            format!("more_like_lookup user={} doc={id}: {x:?} vs {y:?}", u.id),
                        );
                    }
                }
            }
        }
    }
    // pl-mail single checks, grantees, user_can_change
    for mm in [pl_mail::perms::MAIL_ACCOUNT, pl_mail::perms::MAIL_RULE] {
        let ct = pl_mail::perms::content_type_id(db, mm).await.unwrap();
        let rows: Vec<(i32, Option<i32>)> = sqlx::query_as(&format!(
            "SELECT id, owner_id FROM {} ORDER BY id",
            mm.table
        ))
        .fetch_all(db)
        .await
        .unwrap();
        let pks: Vec<i32> = rows.iter().map(|r| r.0).collect();
        grants::set_override(Some(Engine::Legacy));
        let gl = pl_mail::perms::grantees(db, mm, ct, &pks).await.unwrap();
        grants::set_override(Some(Engine::Grants));
        let gg = pl_mail::perms::grantees(db, mm, ct, &pks).await.unwrap();
        stats.mail_cmp += 1;
        if gl != gg {
            diff(stats, format!("pl-mail grantees {}", mm.name));
        }
        for u in us {
            let mu = pl_mail::ctx::MailUser {
                id: u.id as i32,
                is_active: u.is_active,
                is_superuser: u.is_superuser,
                perms: None,
            };
            grants::set_override(Some(Engine::Legacy));
            let cl = pl_mail::perms::user_can_change(db, &mu, mm, ct, &rows)
                .await
                .unwrap();
            grants::set_override(Some(Engine::Grants));
            let cg = pl_mail::perms::user_can_change(db, &mu, mm, ct, &rows)
                .await
                .unwrap();
            stats.mail_cmp += 1;
            if cl != cg {
                diff(
                    stats,
                    format!("pl-mail user_can_change user={} {}", u.id, mm.name),
                );
            }
            for pk in &pks {
                for a in ["view", "change", "delete"] {
                    let code = format!("{a}_{}", mm.name);
                    grants::set_override(Some(Engine::Legacy));
                    let x = pl_mail::perms::guardian_has_perm(db, &mu, &code, ct, *pk)
                        .await
                        .unwrap();
                    grants::set_override(Some(Engine::Grants));
                    let y = pl_mail::perms::guardian_has_perm(db, &mu, &code, ct, *pk)
                        .await
                        .unwrap();
                    stats.mail_cmp += 1;
                    if x != y {
                        diff(
                            stats,
                            format!("pl-mail guardian_has_perm user={} {code} {pk}", u.id),
                        );
                    }
                }
            }
        }
    }
}

async fn check_index(db: &PgPool, stats: &mut Stats, ctx: &str) {
    let c = grants::check(db).await.unwrap();
    stats.checks += 1;
    if !c.ok() {
        diff(stats, format!("index check after {ctx}: {c:?}"));
    }
}

// ------------------------------------------------------------------------------ Mutations

async fn perm_ids(db: &PgPool, m: &ObjectModel) -> Vec<i32> {
    let r: Vec<(i32,)> = sqlx::query_as(
        "SELECT p.id FROM auth_permission p JOIN django_content_type c ON c.id = p.content_type_id
          WHERE c.app_label = $1 AND c.model = $2",
    )
    .bind(m.app_label)
    .bind(m.model_name)
    .fetch_all(db)
    .await
    .unwrap();
    r.into_iter().map(|x| x.0).collect()
}

async fn ct_of(db: &PgPool, m: &ObjectModel) -> i32 {
    sqlx::query_scalar("SELECT id FROM django_content_type WHERE app_label = $1 AND model = $2")
        .bind(m.app_label)
        .bind(m.model_name)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn mutate(db: &PgPool, rng: &mut StdRng, stats: &mut Stats) -> &'static str {
    let us: Vec<i64> = table_ids(db, "auth_user").await;
    let gs: Vec<i64> = table_ids(db, "auth_group").await;
    let pick_model = |rng: &mut StdRng| -> ObjectModel {
        // Documents and tags more often
        let w = [
            models::DOCUMENT,
            models::DOCUMENT,
            models::DOCUMENT,
            models::TAG,
            models::CORRESPONDENT,
            models::DOCUMENT_TYPE,
            models::STORAGE_PATH,
            models::SAVED_VIEW,
            models::MAIL_ACCOUNT,
            models::MAIL_RULE,
        ];
        *w.choose(rng).unwrap()
    };
    let op = rng.gen_range(0..100);
    let name: &'static str = match op {
        0..=29 => {
            // v1 set_permissions (same write path as Django)
            let m = pick_model(rng);
            let all = table_ids(db, m.table).await;
            if all.is_empty() {
                return "noop";
            }
            let n = rng.gen_range(1..=20.min(all.len()));
            let pks: Vec<i64> = all.choose_multiple(rng, n).copied().collect();
            let pick = |src: &Vec<i64>, rng: &mut StdRng| -> Option<Vec<i64>> {
                if rng.gen_bool(0.2) {
                    return None;
                }
                let k = rng.gen_range(0..=src.len().min(3));
                Some(src.choose_multiple(rng, k).copied().collect())
            };
            let spec = PermissionsSpec {
                view: if rng.gen_bool(0.8) {
                    Some(GranteesUpdate {
                        users: pick(&us, rng),
                        groups: pick(&gs, rng),
                    })
                } else {
                    None
                },
                change: if rng.gen_bool(0.6) {
                    Some(GranteesUpdate {
                        users: pick(&us, rng),
                        groups: pick(&gs, rng),
                    })
                } else {
                    None
                },
            };
            let merge = rng.gen_bool(0.3);
            let mut tx = db.begin().await.unwrap();
            owned::set_permissions_for_objects(&mut tx, &spec, &m, &pks, merge)
                .await
                .unwrap();
            if rng.gen_bool(0.1) {
                tx.rollback().await.unwrap();
                "set_permissions_rollback"
            } else {
                tx.commit().await.unwrap();
                "set_permissions"
            }
        }
        30..=39 => {
            // delete guardian rows directly (user or group table)
            let t = if rng.gen_bool(0.5) {
                "guardian_userobjectpermission"
            } else {
                "guardian_groupobjectpermission"
            };
            let k = rng.gen_range(1..=10);
            sqlx::query(&format!(
                "DELETE FROM {t} WHERE id IN (SELECT id FROM {t} ORDER BY random() LIMIT {k})"
            ))
            .execute(db)
            .await
            .unwrap();
            "guardian_delete"
        }
        40..=47 => {
            // change a guardian row: object_pk (also non-canonical), permission, content type
            let t = if rng.gen_bool(0.5) {
                "guardian_userobjectpermission"
            } else {
                "guardian_groupobjectpermission"
            };
            let set = match rng.gen_range(0..4) {
                0 => "object_pk = (object_pk::text)".to_string(),
                1 => format!("object_pk = '{}'", rng.gen_range(1..300)),
                2 => "object_pk = '0' || object_pk".to_string(),
                _ => {
                    let m = pick_model(rng);
                    let p = perm_ids(db, &m).await;
                    format!("permission_id = {}", p.choose(rng).unwrap())
                }
            };
            let _ = sqlx::query(&format!(
                "UPDATE {t} SET {set} WHERE id IN (SELECT id FROM {t} ORDER BY random() LIMIT {})",
                rng.gen_range(1..=5)
            ))
            .execute(db)
            .await; // unique violation possible → ignored (statement rolls back)
            "guardian_update"
        }
        48..=52 => {
            // row with a deviating row content type (permission of model A, ct of model B)
            let a = pick_model(rng);
            let b = pick_model(rng);
            let p = perm_ids(db, &a).await;
            let ct = ct_of(db, &b).await;
            let ids_a = table_ids(db, a.table).await;
            let (Some(pid), Some(oid)) = (p.choose(rng), ids_a.choose(rng)) else {
                return "noop";
            };
            let (t, col, who) = if rng.gen_bool(0.5) {
                (
                    "guardian_userobjectpermission",
                    "user_id",
                    *us.choose(rng).unwrap(),
                )
            } else if let Some(g) = gs.choose(rng) {
                ("guardian_groupobjectpermission", "group_id", *g)
            } else {
                return "noop";
            };
            let _ = sqlx::query(&format!(
                "INSERT INTO {t} (object_pk, content_type_id, permission_id, {col}) VALUES ($1, $2, $3, $4)"
            ))
            .bind(oid.to_string())
            .bind(ct)
            .bind(pid)
            .bind(who as i32)
            .execute(db)
            .await;
            "guardian_insert_ct_mismatch"
        }
        53..=65 => {
            // Group membership
            let (Some(u), Some(g)) = (us.choose(rng), gs.choose(rng)) else {
                return "noop";
            };
            if rng.gen_bool(0.5) {
                sqlx::query("INSERT INTO auth_user_groups (user_id, group_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                    .bind(*u as i32)
                    .bind(*g as i32)
                    .execute(db)
                    .await
                    .unwrap();
                "group_add"
            } else {
                sqlx::query("DELETE FROM auth_user_groups WHERE user_id = $1 AND group_id = $2")
                    .bind(*u as i32)
                    .bind(*g as i32)
                    .execute(db)
                    .await
                    .unwrap();
                "group_remove"
            }
        }
        66..=80 => {
            // Owner change (also to NULL)
            let m = pick_model(rng);
            let all = table_ids(db, m.table).await;
            if all.is_empty() {
                return "noop";
            }
            let n = rng.gen_range(1..=30.min(all.len()));
            let pks: Vec<i64> = all.choose_multiple(rng, n).copied().collect();
            let owner: Option<i32> = if rng.gen_bool(0.3) {
                None
            } else {
                us.choose(rng).map(|x| *x as i32)
            };
            sqlx::query(&format!(
                "UPDATE {} SET owner_id = $1 WHERE id = ANY($2)",
                m.table
            ))
            .bind(owner)
            .bind(&pks)
            .execute(db)
            .await
            .unwrap();
            "owner_change"
        }
        81..=88 => {
            // Toggle superuser/active (never admin = id 2, so that the run keeps a superuser)
            let cand: Vec<i64> = us.iter().copied().filter(|x| *x != 2).collect();
            let Some(u) = cand.choose(rng) else {
                return "noop";
            };
            let col = if rng.gen_bool(0.5) {
                "is_superuser"
            } else {
                "is_active"
            };
            sqlx::query(&format!(
                "UPDATE auth_user SET {col} = NOT {col} WHERE id = $1"
            ))
            .bind(*u as i32)
            .execute(db)
            .await
            .unwrap();
            if col == "is_superuser" {
                "superuser_flip"
            } else {
                "active_flip"
            }
        }
        89..=92 => {
            // Attach/detach a document version (root_document_id) – permissions via the root document
            let all = table_ids(db, "documents_document").await;
            let (Some(a), Some(b)) = (all.choose(rng), all.choose(rng)) else {
                return "noop";
            };
            if a == b {
                return "noop";
            }
            if rng.gen_bool(0.5) {
                sqlx::query("UPDATE documents_document SET root_document_id = $2 WHERE id = $1 AND NOT EXISTS (SELECT 1 FROM documents_document WHERE root_document_id = $1) AND (SELECT root_document_id FROM documents_document WHERE id = $2) IS NULL")
                    .bind(*a as i32).bind(*b as i32).execute(db).await.unwrap();
                "version_attach"
            } else {
                sqlx::query("UPDATE documents_document SET root_document_id = NULL WHERE id = $1")
                    .bind(*a as i32)
                    .execute(db)
                    .await
                    .unwrap();
                "version_detach"
            }
        }
        93..=95 => {
            // Toggle trash
            let all = table_ids(db, "documents_document").await;
            let Some(a) = all.choose(rng) else {
                return "noop";
            };
            sqlx::query("UPDATE documents_document SET deleted_at = CASE WHEN deleted_at IS NULL THEN now() ELSE NULL END WHERE id = $1")
                .bind(*a as i32).execute(db).await.unwrap();
            "trash_toggle"
        }
        96..=97 => {
            // Remove all permissions of a group (like deleting the group, without the group)
            let Some(g) = gs.choose(rng) else {
                return "noop";
            };
            sqlx::query("DELETE FROM guardian_groupobjectpermission WHERE group_id = $1")
                .bind(*g as i32)
                .execute(db)
                .await
                .unwrap();
            "group_grants_purge"
        }
        98 => {
            // TRUNCATE in a rolled-back transaction + real TRUNCATE rarely
            let mut tx = db.begin().await.unwrap();
            sqlx::query("TRUNCATE guardian_groupobjectpermission")
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
            "truncate_rollback"
        }
        _ => {
            if rng.gen_bool(0.3) {
                sqlx::query("TRUNCATE guardian_userobjectpermission")
                    .execute(db)
                    .await
                    .unwrap();
                "truncate_user"
            } else {
                "noop"
            }
        }
    };
    *stats.mutations.entry(name).or_default() += 1;
    name
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a disposable DB copy (PL_L5_DB), writes to it"]
async fn grants_differential() {
    let url = std::env::var("PL_L5_DB").expect("PL_L5_DB (DB-Kopie!)");
    assert!(
        !url.ends_with("/paperless"),
        "never against the shared reference DB"
    );
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    let rounds = env_num("PL_L5_ROUNDS", 300);
    let full_every = env_num("PL_L5_FULL_EVERY", 10).max(1);
    let sample = env_num("PL_L5_SAMPLE", 400) as usize;
    let mut rng = StdRng::seed_from_u64(env_num("PL_L5_SEED", 5));
    let mut stats = Stats::default();

    grants::migrations::apply(&db).await.unwrap();
    let st = grants::status(&db).await.unwrap();
    assert!(st.ready(), "index not ready after migration: {st:?}");
    check_index(&db, &mut stats, "migration").await;
    let t0 = std::time::Instant::now();

    // Phase A: existing data
    let us = users(&db).await;
    compare_sets(&db, &us, &mut stats).await;
    compare_objects(&db, &us, &mut rng, sample, &mut stats).await;
    stats.full_runs += 1;
    eprintln!("phase A done in {:?}: {stats:?}", t0.elapsed());

    // Phase B: random changes
    for r in 1..=rounds {
        let n = rng.gen_range(1..=4);
        let mut names = Vec::new();
        for _ in 0..n {
            names.push(mutate(&db, &mut rng, &mut stats).await);
        }
        check_index(&db, &mut stats, &format!("round {r} {names:?}")).await;
        if r % full_every == 0 || r == rounds {
            let us = users(&db).await;
            compare_sets(&db, &us, &mut stats).await;
            compare_objects(&db, &us, &mut rng, sample / 4 + 1, &mut stats).await;
            stats.full_runs += 1;
            eprintln!(
                "round {r}: {} diffs, {} set cmps, {} checker cmps",
                stats.diffs.len(),
                stats.set_cmp,
                stats.checker_cmp
            );
        }
        if stats.diffs.len() > 20 {
            break;
        }
    }
    // Final: rebuild = trigger state
    let before: Vec<(i32, i64, i64, i32)> = sqlx::query_as(
        "SELECT perm_id, subject, object_id, ct FROM paperrust_grant ORDER BY 1,2,3",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    grants::rebuild(&db).await.unwrap();
    let after: Vec<(i32, i64, i64, i32)> = sqlx::query_as(
        "SELECT perm_id, subject, object_id, ct FROM paperrust_grant ORDER BY 1,2,3",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    if before != after {
        diff(
            &mut stats,
            format!(
                "trigger state != rebuild ({} vs {} rows)",
                before.len(),
                after.len()
            ),
        );
    }
    grants::set_override(None);
    let gu: i64 = sqlx::query_scalar("SELECT count(*) FROM guardian_userobjectpermission")
        .fetch_one(&db)
        .await
        .unwrap();
    let gg: i64 = sqlx::query_scalar("SELECT count(*) FROM guardian_groupobjectpermission")
        .fetch_one(&db)
        .await
        .unwrap();
    eprintln!(
        "L5 RESULT {}: rounds={rounds} mutations={} checks={} full_runs={} set_cmp={} (rows {}) checker_cmp={} grantee_cmp={} mail_cmp={} search_cmp={} final_index_rows={} guardian={}+{} diffs={} elapsed={:?}\nmutations: {:?}",
        url.rsplit('/').next().unwrap_or(""),
        stats.mutations.values().sum::<u64>(),
        stats.checks,
        stats.full_runs,
        stats.set_cmp,
        stats.set_rows,
        stats.checker_cmp,
        stats.grantee_cmp,
        stats.mail_cmp,
        stats.search_cmp,
        after.len(),
        gu,
        gg,
        stats.diffs.len(),
        t0.elapsed(),
        stats.mutations
    );
    assert!(stats.diffs.is_empty(), "{:#?}", stats.diffs);
}

// ------------------------------------------------------------------------------ Measurement

async fn time_sql(db: &PgPool, sql: &str, reps: usize) -> (f64, i64) {
    let mut best = f64::MAX;
    let mut n = 0;
    for _ in 0..reps {
        let t = std::time::Instant::now();
        n = sqlx::query_scalar::<_, i64>(&format!("SELECT count(*)::int8 FROM ({sql}) s"))
            .fetch_one(db)
            .await
            .unwrap();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    (best, n)
}

/// Before/after per user and query (`PL_L5_DB`, read-only). Output: CSV lines `BENCH,…`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a DB copy (PL_L5_DB)"]
async fn grants_bench() {
    let url = std::env::var("PL_L5_DB").expect("PL_L5_DB");
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    let reps = env_num("PL_L5_REPS", 5) as usize;
    let names: Vec<String> = std::env::var("PL_L5_USERS")
        .unwrap_or_else(|_| "admin,alice,bob,carol".into())
        .split(',')
        .map(str::to_string)
        .collect();
    let ctx = pl_docfilter::FilterContext::new("Europe/Zurich".parse().unwrap());
    let mut rng = StdRng::seed_from_u64(1);
    let docs = table_ids(&db, "documents_document").await;
    let sample: Vec<i64> = docs.choose_multiple(&mut rng, 500).copied().collect();
    println!("BENCH,user,query,engine,ms,rows");
    let mut totals: BTreeMap<(String, &str), f64> = BTreeMap::new();
    for name in &names {
        let u = user::load_user_by_username(&db, name)
            .await
            .unwrap()
            .expect("user");
        for e in [Engine::Legacy, Engine::Grants] {
            grants::set_override(Some(e));
            let en = if e == Engine::Legacy {
                "legacy"
            } else {
                "grants"
            };
            let mut qs: Vec<(&str, String)> = Vec::new();
            for (label, pairs) in [
                ("list_default", vec![]),
                (
                    "list_is_tagged",
                    vec![("is_tagged".to_string(), "1".to_string())],
                ),
                (
                    "list_title_content",
                    vec![("title_content".to_string(), "rech".to_string())],
                ),
            ] {
                let p = pl_docfilter::QueryParams::from_pairs(pairs.into_iter());
                let (q, _) = pl_store::docsql::v1_list(&db, &ctx, &u, &p).await.unwrap();
                let s = q.to_string(PostgresQueryBuilder);
                qs.push((
                    if label == "list_default" {
                        "count_default"
                    } else {
                        label
                    },
                    s.clone(),
                ));
                if label == "list_default" {
                    qs.push(("page_default_50", format!("{s} LIMIT 50")));
                    qs.push(("page_offset_5000", format!("{s} LIMIT 50 OFFSET 5000")));
                }
            }
            let p = pl_docfilter::QueryParams::from_pairs(std::iter::empty::<(String, String)>());
            let sel = pl_store::docsql::v1_selection(&db, &ctx, &u, &p)
                .await
                .unwrap();
            qs.push(("selection_all", sel.to_string(PostgresQueryBuilder)));
            qs.push((
                "permitted_document_ids",
                pl_authz::objects::permitted_document_ids(Some(&u), "view_document", false)
                    .to_string(PostgresQueryBuilder),
            ));
            for (label, kind, last) in [
                ("tags_document_count", pl_objects::model::Kind::Tag, false),
                (
                    "correspondents_count_last",
                    pl_objects::model::Kind::Correspondent,
                    true,
                ),
                (
                    "doctypes_document_count",
                    pl_objects::model::Kind::DocumentType,
                    false,
                ),
            ] {
                qs.push((
                    label,
                    pl_objects::perms::document_count_source(&u, kind, None, last)
                        .to_string(PostgresQueryBuilder),
                ));
            }
            for (label, sql) in &qs {
                let (ms, n) = time_sql(&db, sql, reps).await;
                println!("BENCH,{name},{label},{en},{ms:.2},{n}");
                *totals.entry((en.to_string(), label)).or_default() += ms;
            }
            // Single checks: 500 × guardian_has_perm(change) and grantees 10 × 50
            let t = std::time::Instant::now();
            for id in &sample {
                perms::guardian_has_perm(&db, &u, &models::DOCUMENT, *id, "change_document")
                    .await
                    .unwrap();
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            println!("BENCH,{name},checker_500,{en},{ms:.2},500");
            *totals.entry((en.to_string(), "checker_500")).or_default() += ms;
            let t = std::time::Instant::now();
            for chunk in sample.chunks(50) {
                owned::OwnedInfo::load(
                    &db,
                    &models::DOCUMENT,
                    chunk,
                    owned::OwnedFields::new(false, true),
                    true,
                )
                .await
                .unwrap();
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            println!("BENCH,{name},owned_info_10x50,{en},{ms:.2},500");
            *totals
                .entry((en.to_string(), "owned_info_10x50"))
                .or_default() += ms;
        }
    }
    grants::set_override(None);
    for ((en, label), ms) in totals {
        println!("TOTAL,{label},{en},{ms:.2}");
    }
}
