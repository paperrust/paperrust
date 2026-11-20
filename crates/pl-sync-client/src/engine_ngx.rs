//! Reconciliation against paperless-ngx (v1, restricted; docs/SYNC.md §7, ROADMAP-NEXT P6).
//!
//! Without a change feed. Per run (continuous loop: every 60 s):
//! 1. `ui_settings` (check login, user/permissions/settings for offline);
//! 2. master data complete (`?page_size=100000`) – at most every 5 min, otherwise on demand;
//! 3. resolve selection: document IDs, tags (`tags__id__in`), saved views (filter rules →
//!    query parameters, [`crate::filter_rules`]) – each via the field `all` (API version 9);
//! 4. Deleted/revoked: `all` without filter = all visible IDs; remove local surplus;
//! 5. Changed: `modified__gt=<watermark − 2 s>`; additionally every [`FINGERPRINT_EVERY`] a
//!    fingerprint comparison of the selection (`id__in` ≤ 100, `fields=…`), because bulk tags etc. do not
//!    change `modified` (`bulk_edit.add_tag`);
//! 6. New/changed documents completely (`id__in`, with content + notes, metadata for the
//!    checksums), files via `download/`/`thumb/`.
//!
//! Limits: bytes are unknown before loading (v1 lists return no sizes) → only the
//! document count is checked up front, the file cache limits the space.

use std::collections::{HashMap, HashSet};

use serde_json::json;
use sqlx::Row;

use super::*;
use crate::filter_rules;
use crate::remote::ngx::{FINGERPRINT_FIELDS, Users, code_of, doc_v2, fingerprint, ngx_uuid, obj_v2};
use crate::remote::{ResolvedDoc, ResolvedSize};

/// Fingerprint comparison of the selection (SYNC.md §7).
pub const FINGERPRINT_EVERY: Duration = Duration::from_secs(15 * 60);
/// Master data completely at most this often.
pub const META_EVERY: chrono::Duration = chrono::Duration::minutes(5);

/// v1 lists per metadata kind.
const META: [(&str, &str); 8] = [
    ("tag", "tags"),
    ("correspondent", "correspondents"),
    ("document_type", "document_types"),
    ("storage_path", "storage_paths"),
    ("custom_field", "custom_fields"),
    ("saved_view", "saved_views"),
    ("user", "users"),
    ("group", "groups"),
];

/// v1 ID from a synthetic UUID (only for the expected kind).
pub fn ngx_id(uuid: &str, kind: &str) -> Option<i64> {
    let u = uuid::Uuid::parse_str(uuid).ok()?.to_string();
    let code = code_of(kind);
    let prefix = format!("00000000-0000-4000-8{:03x}-", code as u16 & 0xfff);
    i64::from_str_radix(u.strip_prefix(&prefix)?, 16).ok()
}

fn stale(ts: Option<String>, every: chrono::Duration) -> bool {
    ts.and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
        .is_none_or(|t| chrono::Utc::now().signed_duration_since(t.with_timezone(&chrono::Utc)) > every)
}

impl SyncClient {
    fn ngx(&self) -> Result<&crate::remote::NgxRemote> {
        self.inner.ngx.as_ref().ok_or_else(|| Error::Invalid("not a paperless-ngx profile".into()))
    }

    /// Interval of the fingerprint comparison (tests: 0).
    pub fn set_fingerprint_interval(&self, d: Duration) {
        if let Ok(mut f) = self.inner.fingerprint_every.lock() {
            *f = d;
        }
    }

    pub(super) async fn login_ngx(&self, username: &str, password: Zeroizing<String>, code: Option<&str>) -> Result<()> {
        let ngx = self.ngx()?;
        let token = ngx.token(username, &password, code).await;
        drop(password);
        let token = token?;
        self.inner.secrets.set(&target_for(&self.inner.cfg.profile_id), &token)?;
        ngx.set_token(Some(token.clone()));
        self.inner.remote.set_token(Some(token));
        self.inner.replica.set_remote_field("user_name", Some(username)).await?;
        self.set_status(|s| {
            s.logged_in = true;
            s.online = true;
            if s.state == "auth_required" {
                s.state = "idle".into();
            }
        });
        self.wake();
        Ok(())
    }

    /// IDs of a selection entry on the server.
    async fn ngx_ids_of(&self, it: &SelectionItem) -> Result<Vec<i64>> {
        let ngx = self.ngx()?;
        match it.kind.as_str() {
            "document" => Ok(ngx_id(&it.key, "document").into_iter().collect()),
            "tag" => {
                let id = ngx_id(&it.key, "tag").ok_or_else(|| Error::Invalid(format!("tag {}", it.key)))?;
                ngx.doc_ids(&[("tags__id__in".into(), id.to_string())]).await
            }
            "saved_view" => {
                let v = self.inner.replica.object("saved_view", &it.key).await?.ok_or(Error::NotFound)?;
                let rules: Vec<(i64, Option<String>)> = v["filter_rules"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|r| (r["rule_type"].as_i64().unwrap_or(-1), r["value"].as_str().map(str::to_string).or_else(|| (!r["value"].is_null()).then(|| r["value"].to_string()))))
                            .collect()
                    })
                    .unwrap_or_default();
                let params = filter_rules::query_params(&rules).map_err(Error::Invalid)?;
                ngx.doc_ids(&params).await
            }
            other => Err(Error::Invalid(format!("selection type {other}"))),
        }
    }

    pub(super) async fn preview_selection_ngx(&self, items: &[SelectionItem]) -> Result<Plan> {
        let mut resolved = Vec::new();
        for it in items {
            if !matches!(it.files.as_str(), "preferred" | "original" | "both" | "none") {
                return Err(Error::Invalid(format!("files mode {}", it.files)));
            }
            let ids = self.ngx_ids_of(it).await?;
            // Sizes are not known up front via v1 (only document count checked)
            let docs = ids.into_iter().map(|id| ResolvedDoc { uuid: ngx_uuid(1, id), rev: 0, size: ResolvedSize::default() }).collect();
            resolved.push((it.clone(), docs));
        }
        Ok(plan(resolved, self.limits().await?, None))
    }

    /// Mirror master data completely. Returns: user names + IDs of the document-link fields.
    async fn ngx_metadata(&self, me: &Value, rep: &mut SyncReport) -> Result<(Users, Vec<i64>)> {
        let ngx = self.ngx()?;
        let r = &self.inner.replica;
        let mut users: Users = HashMap::new();
        if let (Some(id), Some(name)) = (me["id"].as_i64(), me["username"].as_str()) {
            users.insert(id, name.to_string());
        }
        let mut lists: Vec<(&str, Vec<Value>)> = Vec::new();
        for (kind, ep) in META {
            match ngx.list_all(ep).await {
                Ok(l) => lists.push((kind, l)),
                // without permission (e.g. user list) → skip the kind, do not delete the local state
                Err(Error::Api { status: 403 | 404, .. }) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some((_, l)) = lists.iter().find(|(k, _)| *k == "user") {
            for u in l {
                if let (Some(id), Some(n)) = (u["id"].as_i64(), u["username"].as_str()) {
                    users.insert(id, n.to_string());
                }
            }
        }
        let mut doclink = Vec::new();
        for (kind, l) in &lists {
            let objs: Vec<Value> = l.iter().map(|o| obj_v2(kind, o, &users)).collect();
            if *kind == "custom_field" {
                doclink = l.iter().filter(|o| o["data_type"] == "documentlink").filter_map(|o| o["id"].as_i64()).collect();
            }
            let keep: HashSet<String> = objs.iter().filter_map(|o| o["uuid"].as_str().map(str::to_string)).collect();
            rep.objects_upserted += objs.len() as u64;
            r.upsert_objects(kind, &objs).await?;
            let gone: Vec<String> = r.object_revs(kind).await?.into_iter().map(|(k, _)| k).filter(|k| !keep.contains(k)).collect();
            for k in gone {
                r.delete_object(kind, &k).await?;
                rep.objects_deleted += 1;
            }
        }
        r.set_setting("ngx_users", &serde_json::to_string(&users)?).await?;
        r.set_setting("ngx_doclink", &serde_json::to_string(&doclink)?).await?;
        r.set_setting("ngx_meta_at", &now()).await?;
        Ok((users, doclink))
    }

    pub(super) async fn sync_ngx(&self) -> Result<SyncReport> {
        let ngx = self.ngx()?;
        let r = &self.inner.replica;
        let mut rep = SyncReport::default();
        // no feed: continuous loop in query mode (POLL_INTERVAL)
        self.inner.has_feed.store(false, Ordering::SeqCst);
        // 1. login/framework
        let ui = ngx.get_json("api/ui_settings/").await?;
        let u = &ui["user"];
        let me = json!({"uuid": u["id"].as_i64().map(|i| ngx_uuid(8, i)), "legacy_id": u["id"], "username": u["username"],
                        "first_name": u["first_name"], "last_name": u["last_name"], "email": "", "is_superuser": u["is_superuser"],
                        "is_staff": u["is_staff"], "groups": [], "permissions": ui["permissions"], "auth": "token"});
        r.set_remote_field("me_json", Some(&me.to_string())).await?;
        r.set_remote_field("ui_settings", Some(&json!({"settings": ui["settings"]}).to_string())).await?;
        let _ = r.cache_put("/api/ui_settings/", 200, Some("application/json"), ui.to_string().as_bytes()).await;
        // 2. master data
        self.set_status(|s| s.phase = Some("metadata".into()));
        let (users, doclink) = if stale(r.setting("ngx_meta_at").await?, META_EVERY) || r.count_objects("tag").await? == 0 {
            rep.full = true;
            self.ngx_metadata(u, &mut rep).await?
        } else {
            let users: Users = r.setting("ngx_users").await?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            let doclink: Vec<i64> = r.setting("ngx_doclink").await?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            (users, doclink)
        };
        // 3. selection
        self.set_status(|s| s.phase = Some("resolve".into()));
        rep.resolved = true;
        let items = r.selection().await?;
        let p = self.preview_selection_ngx(&items).await?;
        if !p.within_limits {
            return Err(Error::LimitExceeded(p.message.unwrap_or_default()));
        }
        // 4. deleted / permission revoked (all visible IDs)
        let visible: HashSet<i64> = ngx.doc_ids(&[]).await?.into_iter().collect();
        let states = r.document_states().await?;
        let at = now();
        let mut local: HashMap<String, (bool, bool)> = HashMap::new();
        for (key, _rev, sel, det) in states {
            let id = ngx_id(&key, "document");
            if id.is_none_or(|i| !visible.contains(&i)) {
                let paths = r.delete_document(&key).await?;
                FileCache::remove_paths(&paths).await;
                rep.documents_removed += 1;
                continue;
            }
            if sel && !p.docs.contains_key(&key) {
                r.mark_unselected(&key, &at).await?;
                self.inner.files.remove_doc(r, &key).await?;
                continue;
            }
            local.insert(key, (sel, det));
        }
        // 5. changed: new/without content, modified__gt, fingerprint. IDs that `all` reports but
        // the list with `id__in` does not return (e.g. document versions) are not fetched again every time.
        let unfetchable: HashSet<i64> = r.setting("ngx_unfetchable").await?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let mut need: HashSet<i64> = HashSet::new();
        for key in p.docs.keys() {
            match local.get(key) {
                Some((true, true)) => {}
                _ => {
                    if let Some(id) = ngx_id(key, "document").filter(|i| !unfetchable.contains(i)) {
                        need.insert(id);
                    }
                }
            }
        }
        let row = r.remote().await?.unwrap_or_default();
        let watermark: Option<String> = sqlx::query_scalar("SELECT ngx_watermark FROM remote WHERE id = 1").fetch_optional(r.pool()).await?.flatten();
        let mut new_mark = watermark.clone();
        if let Some(w) = &watermark {
            let since = chrono::DateTime::parse_from_rfc3339(w).map(|t| (t - chrono::Duration::seconds(2)).to_rfc3339()).unwrap_or_else(|_| w.clone());
            for (id, m) in ngx.modified_since(&since).await? {
                if p.docs.contains_key(&ngx_uuid(1, id)) {
                    need.insert(id);
                }
                if new_mark.as_deref().is_none_or(|x| m.as_str() > x) {
                    new_mark = Some(m);
                }
            }
        }
        let fp_every = self.inner.fingerprint_every.lock().map(|d| *d).unwrap_or(FINGERPRINT_EVERY);
        let fp_due = stale(r.setting("ngx_fp_at").await?, chrono::Duration::from_std(fp_every).unwrap_or(META_EVERY));
        let _ = row;
        if fp_due {
            self.set_status(|s| s.phase = Some("fingerprint".into()));
            let check: Vec<i64> = p.docs.keys().filter_map(|k| ngx_id(k, "document")).filter(|i| !need.contains(i)).collect();
            let fps: HashMap<String, Option<String>> = sqlx::query("SELECT key, fingerprint FROM document")
                .fetch_all(r.pool())
                .await?
                .into_iter()
                .map(|row| (row.get::<String, _>(0), row.get::<Option<String>, _>(1)))
                .collect();
            for d in ngx.docs_by_id(&check, Some(FINGERPRINT_FIELDS)).await? {
                let Some(id) = d["id"].as_i64() else { continue };
                if fps.get(&ngx_uuid(1, id)).cloned().flatten().as_deref() != Some(fingerprint(&d).as_str()) {
                    need.insert(id);
                }
            }
            r.set_setting("ngx_fp_at", &now()).await?;
        }
        // 6. fetch completely (+ metadata for checksums if files are wanted)
        let wants_files: HashSet<String> = p.docs.iter().filter(|(_, d)| d.wants.any()).map(|(k, _)| k.clone()).collect();
        let mut need: Vec<i64> = need.into_iter().collect();
        need.sort();
        self.set_status(|s| {
            s.phase = Some("documents".into());
            s.done = 0;
            s.total = need.len() as u64;
        });
        let mut fetched_keys = Vec::new();
        let mut missing: HashSet<i64> = unfetchable.iter().copied().filter(|i| p.docs.contains_key(&ngx_uuid(1, *i))).collect();
        for chunk in need.chunks(100) {
            let docs = ngx.docs_by_id(chunk, None).await?;
            let got: HashSet<i64> = docs.iter().filter_map(|d| d["id"].as_i64()).collect();
            for d in &docs {
                let id = d["id"].as_i64().unwrap_or(0);
                let key = ngx_uuid(1, id);
                let meta = if wants_files.contains(&key) { ngx.metadata(id).await.ok() } else { None };
                let v2 = doc_v2(d, meta.as_ref(), &users, &doclink);
                r.upsert_documents(std::slice::from_ref(&v2), true, true).await?;
                r.set_fingerprint(&key, &fingerprint(d)).await?;
                if let Some(m) = d["modified"].as_str()
                    && new_mark.as_deref().is_none_or(|x| m > x)
                {
                    new_mark = Some(m.to_string());
                }
                fetched_keys.push(key);
                rep.documents_fetched += 1;
            }
            for id in chunk.iter().filter(|i| !got.contains(i)) {
                let paths = r.delete_document(&ngx_uuid(1, *id)).await?;
                FileCache::remove_paths(&paths).await;
                missing.insert(*id);
            }
            self.set_status(|s| s.done += chunk.len() as u64);
        }
        if missing != unfetchable {
            r.set_setting("ngx_unfetchable", &serde_json::to_string(&missing)?).await?;
        }
        if new_mark != watermark {
            r.set_remote_field("ngx_watermark", new_mark.as_deref()).await?;
        }
        // 7. files
        let mut jobs = Vec::new();
        for (key, d) in &p.docs {
            let Some(doc) = r.document_json(key).await? else { continue };
            let has_archive = !doc["files"]["archive"].is_null();
            for v in d.wants.variants(has_archive) {
                jobs.push((doc.clone(), v.to_string()));
            }
            jobs.push((doc, "thumb".to_string()));
        }
        self.run_file_jobs(jobs, &mut rep).await?;
        r.set_remote_field("last_full_sync", Some(&now())).await?;
        self.housekeeping().await?;
        Ok(rep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_synthetic_uuids() {
        assert_eq!(ngx_id(&ngx_uuid(1, 4711), "document"), Some(4711));
        assert_eq!(ngx_id(&ngx_uuid(2, 5), "document"), None);
        assert_eq!(ngx_id("not-a-uuid", "tag"), None);
    }
}
