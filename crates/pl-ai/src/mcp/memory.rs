//! Storage backend for tests and demonstrations (without DB). Permissions like upstream
//! (`permitted_object_ids`, owner-aware `change`), see [`crate::taxonomy::Principal::can`].

use std::collections::BTreeSet;
use std::sync::Mutex;

use serde_json::{Value, json};

use super::backend::*;
use crate::alias::MemoryAliasStore;
use crate::taxonomy::{Alias, Grant, Principal, TaxonomyEntry, TaxonomyKind};

/// A user.
#[derive(Debug, Clone)]
pub struct MemUser {
    pub id: i64,
    pub username: String,
    pub token: String,
    pub is_superuser: bool,
    pub is_active: bool,
    pub groups: Vec<i64>,
    pub perms: BTreeSet<String>,
}

/// A document.
#[derive(Debug, Clone)]
pub struct MemDocument {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub owner_id: Option<i64>,
    pub created: String,
    pub correspondent_id: Option<i64>,
    pub document_type_id: Option<i64>,
    pub storage_path_id: Option<i64>,
    pub tag_ids: Vec<i64>,
    pub custom_fields: Vec<(i64, Value)>,
    pub original_filename: Option<String>,
    pub deleted: bool,
    /// Object permissions on the document: (user_id, codename).
    pub grants: Vec<(i64, String)>,
}

#[derive(Default)]
struct Data {
    users: Vec<MemUser>,
    entries: Vec<TaxonomyEntry>,
    grants: Vec<Grant>,
    docs: Vec<MemDocument>,
    audit: Vec<AuditEntry>,
    next_id: i64,
    notes: Vec<(i64, NoteEntry)>,
    views: Vec<(Option<i64>, SavedViewInfo)>,
    /// (task_id, user_id, request)
    submissions: Vec<(String, Option<i64>, IngestRequest)>,
    scopes: Vec<(String, Vec<String>)>,
}

/// In-memory backend.
#[derive(Default)]
pub struct MemoryBackend {
    data: Mutex<Data>,
    aliases: MemoryAliasStore,
}

fn principal(u: &MemUser) -> Principal {
    Principal { user_id: Some(u.id), is_active: u.is_active, is_superuser: u.is_superuser, group_ids: u.groups.clone() }
}

fn doc_can(p: &Principal, d: &MemDocument, action: &str) -> bool {
    let Some(uid) = p.user_id else { return d.owner_id.is_none() };
    if !p.is_active {
        return false;
    }
    if p.is_superuser || d.owner_id.is_none() || d.owner_id == Some(uid) {
        return true;
    }
    d.grants.iter().any(|(u, c)| *u == uid && c == &format!("{action}_document"))
}

fn summary(d: &MemDocument) -> DocumentSummary {
    DocumentSummary {
        id: d.id,
        title: d.title.clone(),
        created: Some(d.created.clone()),
        correspondent_id: d.correspondent_id,
        document_type_id: d.document_type_id,
        storage_path_id: d.storage_path_id,
        tag_ids: d.tag_ids.clone(),
        archive_serial_number: None,
        added: None,
    }
}

impl MemoryBackend {
    pub fn add_user(&self, u: MemUser) {
        if let Ok(mut g) = self.data.lock() {
            g.users.push(u);
        }
    }
    pub fn add_entry(&self, e: TaxonomyEntry) {
        if let Ok(mut g) = self.data.lock() {
            g.next_id = g.next_id.max(e.id);
            g.entries.push(e);
        }
    }
    pub fn add_grant(&self, gr: Grant) {
        if let Ok(mut g) = self.data.lock() {
            g.grants.push(gr);
        }
    }
    pub fn add_document(&self, d: MemDocument) {
        if let Ok(mut g) = self.data.lock() {
            g.docs.push(d);
        }
    }
    pub fn audit_log(&self) -> Vec<AuditEntry> {
        self.data.lock().map(|g| g.audit.clone()).unwrap_or_default()
    }
    pub fn entries(&self) -> Vec<TaxonomyEntry> {
        self.data.lock().map(|g| g.entries.clone()).unwrap_or_default()
    }
    /// Set scopes for a token (like a row in `paperrust_ai_mcp_scope`).
    pub fn set_token_scopes(&self, token: &str, scopes: &[&str]) {
        if let Ok(mut g) = self.data.lock() {
            g.scopes.retain(|(t, _)| t != token);
            g.scopes.push((token.to_string(), scopes.iter().map(|s| s.to_string()).collect()));
        }
    }
    /// Saved view (owner `None` = visible to everyone).
    pub fn add_saved_view(&self, owner: Option<i64>, v: SavedViewInfo) {
        if let Ok(mut g) = self.data.lock() {
            g.views.push((owner, v));
        }
    }
    /// Submitted files (task_id, user_id, request).
    pub fn submissions(&self) -> Vec<(String, Option<i64>, IngestRequest)> {
        self.data.lock().map(|g| g.submissions.clone()).unwrap_or_default()
    }
    pub fn document(&self, id: i64) -> Option<MemDocument> {
        self.data.lock().ok()?.docs.iter().find(|d| d.id == id).cloned()
    }

    fn visible(&self, actor: &Actor, kind: TaxonomyKind) -> Vec<TaxonomyEntry> {
        let Ok(g) = self.data.lock() else { return Vec::new() };
        let mut v: Vec<TaxonomyEntry> =
            g.entries.iter().filter(|e| e.kind == kind && actor.principal.can_view(e, &g.grants)).cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

fn doc_json(d: &MemDocument) -> Value {
    json!({
        "title": d.title, "created": d.created, "correspondent_id": d.correspondent_id,
        "document_type_id": d.document_type_id, "storage_path_id": d.storage_path_id,
        "tag_ids": d.tag_ids, "custom_fields": d.custom_fields.iter().map(|(f, v)| json!({"field_id": f, "value": v})).collect::<Vec<_>>(),
    })
}

impl Backend for MemoryBackend {
    async fn authenticate(&self, token: &str) -> BResult<Option<Actor>> {
        let g = self.data.lock().map_err(|_| BackendError::Internal)?;
        Ok(g.users.iter().find(|u| u.token == token).map(|u| Actor {
            principal: principal(u),
            username: u.username.clone(),
            perms: u.perms.clone(),
            token_hint: format!("{}…", token.chars().take(4).collect::<String>()),
            rate_key: format!("u{}", u.id),
            credential: Credential::new(token),
        }))
    }

    async fn list_taxonomy(&self, actor: &Actor, kind: TaxonomyKind) -> BResult<Vec<TaxonomyEntry>> {
        Ok(self.visible(actor, kind))
    }

    async fn aliases(&self, actor: &Actor, kind: TaxonomyKind) -> BResult<Vec<Alias>> {
        Ok(self.aliases.visible(kind, &self.visible(actor, kind)))
    }

    async fn aliases_within(&self, kind: TaxonomyKind, visible: &[TaxonomyEntry]) -> BResult<Vec<Alias>> {
        Ok(self.aliases.visible(kind, visible))
    }

    async fn can_object(&self, actor: &Actor, kind: TaxonomyKind, id: i64, action: &str) -> BResult<bool> {
        let g = self.data.lock().map_err(|_| BackendError::Internal)?;
        Ok(g.entries.iter().find(|e| e.kind == kind && e.id == id).is_some_and(|e| actor.principal.can(e, action, &g.grants)))
    }

    async fn create_taxonomy(&self, actor: &Actor, spec: &CreateSpec) -> BResult<TaxonomyEntry> {
        let mut g = self.data.lock().map_err(|_| BackendError::Internal)?;
        // MatchingModel: unique (name, owner)
        if g.entries.iter().any(|e| e.kind == spec.kind && e.name == spec.name && e.owner_id == actor.user_id()) {
            return Err(BackendError::Conflict("an object with this name and owner already exists".into()));
        }
        g.next_id += 1;
        let e = TaxonomyEntry {
            kind: spec.kind,
            id: g.next_id,
            name: spec.name.clone(),
            owner_id: actor.user_id(),
            extra: spec.extra.clone(),
            document_count: Some(0),
        };
        g.entries.push(e.clone());
        Ok(e)
    }

    async fn add_alias(&self, _actor: &Actor, alias: &Alias) -> BResult<bool> {
        self.aliases.add(alias.clone()).map_err(|_| BackendError::Internal)
    }

    async fn search_documents(&self, actor: &Actor, p: &SearchParams) -> BResult<(i64, Vec<DocumentSummary>)> {
        let g = self.data.lock().map_err(|_| BackendError::Internal)?;
        let q = p.query.as_deref().map(str::to_lowercase);
        let mut hits: Vec<&MemDocument> = g
            .docs
            .iter()
            .filter(|d| !d.deleted && doc_can(&actor.principal, d, "view"))
            .filter(|d| q.as_deref().is_none_or(|q| d.title.to_lowercase().contains(q) || d.content.to_lowercase().contains(q)))
            .filter(|d| p.correspondent_id.is_none_or(|c| d.correspondent_id == Some(c)))
            .filter(|d| p.document_type_id.is_none_or(|c| d.document_type_id == Some(c)))
            .filter(|d| p.storage_path_id.is_none_or(|c| d.storage_path_id == Some(c)))
            .filter(|d| p.tag_ids.iter().all(|t| d.tag_ids.contains(t)))
            .collect();
        if p.ordering.as_deref() == Some("-added") {
            hits.sort_by_key(|d| std::cmp::Reverse(d.id));
        }
        let count = hits.len() as i64;
        let off = p.offset.unwrap_or(0) as usize;
        let lim = p.limit.unwrap_or(25) as usize;
        Ok((count, hits.into_iter().skip(off).take(lim).map(summary).collect()))
    }

    async fn get_document(&self, actor: &Actor, id: i64) -> BResult<Option<DocumentDetail>> {
        let g = self.data.lock().map_err(|_| BackendError::Internal)?;
        Ok(g.docs.iter().find(|d| d.id == id && !d.deleted && doc_can(&actor.principal, d, "view")).map(|d| DocumentDetail {
            summary: summary(d),
            original_filename: d.original_filename.clone(),
            owner_id: d.owner_id,
            content: d.content.clone(),
            custom_fields: d.custom_fields.clone(),
            notes_count: g.notes.iter().filter(|(x, _)| *x == d.id).count() as i64,
        }))
    }

    async fn can_change_document(&self, actor: &Actor, id: i64) -> BResult<bool> {
        let g = self.data.lock().map_err(|_| BackendError::Internal)?;
        Ok(g.docs.iter().find(|d| d.id == id && !d.deleted).is_some_and(|d| doc_can(&actor.principal, d, "change")))
    }

    async fn update_document(&self, _actor: &Actor, id: i64, p: &DocumentPatch) -> BResult<(Value, Value)> {
        let mut g = self.data.lock().map_err(|_| BackendError::Internal)?;
        let d = g.docs.iter_mut().find(|d| d.id == id).ok_or(BackendError::NotFound)?;
        let before = doc_json(d);
        if let Some(t) = &p.title {
            d.title = t.clone();
        }
        if let Some(c) = &p.created {
            d.created = c.clone();
        }
        if let Some(v) = p.correspondent_id {
            d.correspondent_id = v;
        }
        if let Some(v) = p.document_type_id {
            d.document_type_id = v;
        }
        if let Some(v) = p.storage_path_id {
            d.storage_path_id = v;
        }
        for t in &p.add_tag_ids {
            if !d.tag_ids.contains(t) {
                d.tag_ids.push(*t);
            }
        }
        d.tag_ids.retain(|t| !p.remove_tag_ids.contains(t));
        for (f, v) in &p.custom_fields {
            d.custom_fields.retain(|(x, _)| x != f);
            if !v.is_null() {
                d.custom_fields.push((*f, v.clone()));
            }
        }
        Ok((before, doc_json(d)))
    }

    async fn audit(&self, entry: &AuditEntry) -> BResult<()> {
        self.data.lock().map_err(|_| BackendError::Internal)?.audit.push(entry.clone());
        Ok(())
    }

    async fn submit_document(&self, actor: &Actor, req: &IngestRequest) -> BResult<String> {
        if !actor.has_perm("documents.add_document") {
            return Err(BackendError::PermissionDenied("Insufficient permissions".into()));
        }
        let mut g = self.data.lock().map_err(|_| BackendError::Internal)?;
        let id = uuid::Uuid::new_v4().to_string();
        g.submissions.push((id.clone(), actor.user_id(), req.clone()));
        Ok(id)
    }

    async fn task_status(&self, actor: &Actor, task_id: &str) -> BResult<Option<Value>> {
        let g = self.data.lock().map_err(|_| BackendError::Internal)?;
        Ok(g.submissions.iter().find(|(t, u, _)| t == task_id && (actor.principal.is_superuser || *u == actor.user_id())).map(
            |(t, _, r)| json!({"task_id": t, "task_file_name": r.filename, "status": "PENDING", "result": null, "related_document": null}),
        ))
    }

    async fn list_notes(&self, actor: &Actor, document_id: i64) -> BResult<Vec<NoteEntry>> {
        if !actor.has_perm("documents.view_note") {
            return Err(BackendError::PermissionDenied("Insufficient permissions to view notes".into()));
        }
        let g = self.data.lock().map_err(|_| BackendError::Internal)?;
        if !g.docs.iter().any(|d| d.id == document_id && !d.deleted && doc_can(&actor.principal, d, "view")) {
            return Err(BackendError::NotFound);
        }
        Ok(g.notes.iter().filter(|(d, _)| *d == document_id).map(|(_, n)| n.clone()).rev().collect())
    }

    async fn add_note(&self, actor: &Actor, document_id: i64, note: &str) -> BResult<Vec<NoteEntry>> {
        if !actor.has_perm("documents.add_note") {
            return Err(BackendError::PermissionDenied("Insufficient permissions to create notes".into()));
        }
        {
            let mut g = self.data.lock().map_err(|_| BackendError::Internal)?;
            let Some(d) = g.docs.iter().find(|d| d.id == document_id && !d.deleted) else { return Err(BackendError::NotFound) };
            if !doc_can(&actor.principal, d, "view") {
                return Err(BackendError::NotFound);
            }
            if !doc_can(&actor.principal, d, "change") {
                return Err(BackendError::PermissionDenied("Insufficient permissions to create notes".into()));
            }
            let id = g.notes.len() as i64 + 1;
            g.notes.push((document_id, NoteEntry { id, note: note.to_string(), created: None, user: Some(actor.username.clone()) }));
        }
        self.list_notes(actor, document_id).await
    }

    async fn list_saved_views(&self, actor: &Actor) -> BResult<Vec<SavedViewInfo>> {
        let g = self.data.lock().map_err(|_| BackendError::Internal)?;
        Ok(g.views
            .iter()
            .filter(|(o, _)| o.is_none() || *o == actor.user_id() || actor.principal.is_superuser)
            .map(|(_, v)| v.clone())
            .collect())
    }

    /// Subset of the v1 filters: `title_content`/`text`, `correspondent__id`, `document_type__id`,
    /// `tags__id__all`, `ordering` (`-added`/`-created`, storage: by id).
    async fn query_documents(
        &self,
        actor: &Actor,
        params: &[(String, String)],
        limit: u32,
        offset: u32,
    ) -> BResult<(i64, Vec<DocumentSummary>)> {
        let g = self.data.lock().map_err(|_| BackendError::Internal)?;
        let get = |k: &str| params.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
        let text = get("title_content").or_else(|| get("text")).map(|t| t.to_lowercase());
        let tags: Vec<i64> = get("tags__id__all").map(|t| t.split(',').filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_default();
        let corr = get("correspondent__id").and_then(|x| x.parse::<i64>().ok());
        let dt = get("document_type__id").and_then(|x| x.parse::<i64>().ok());
        let mut hits: Vec<&MemDocument> = g
            .docs
            .iter()
            .filter(|d| !d.deleted && doc_can(&actor.principal, d, "view"))
            .filter(|d| text.as_deref().is_none_or(|q| d.title.to_lowercase().contains(q) || d.content.to_lowercase().contains(q)))
            .filter(|d| corr.is_none_or(|c| d.correspondent_id == Some(c)))
            .filter(|d| dt.is_none_or(|c| d.document_type_id == Some(c)))
            .filter(|d| tags.iter().all(|t| d.tag_ids.contains(t)))
            .collect();
        match get("ordering").as_deref() {
            Some("-added") | Some("-id") => hits.sort_by_key(|d| std::cmp::Reverse(d.id)),
            Some("-created") => hits.sort_by(|a, b| b.created.cmp(&a.created)),
            Some("created") => hits.sort_by(|a, b| a.created.cmp(&b.created)),
            _ => {}
        }
        let count = hits.len() as i64;
        Ok((count, hits.into_iter().skip(offset as usize).take(limit as usize).map(summary).collect()))
    }

    async fn token_scopes(&self, actor: &Actor) -> BResult<Option<Vec<String>>> {
        let g = self.data.lock().map_err(|_| BackendError::Internal)?;
        Ok(g.scopes.iter().find(|(t, _)| t == actor.credential.expose()).map(|(_, s)| s.clone()))
    }
}
