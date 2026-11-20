//! Status messages for the WebSocket (`/ws/status/`) – counterpart of the Channels group
//! `status_updates` in paperless-ngx (there via Redis; here without Redis).
//!
//! upstream: src/documents/plugins/helpers.py (`ProgressStatusOptions`, `ProgressManager.send_progress`,
//! `DocumentsStatusManager`, Payload-TypedDicts), src/paperless/consumers.py (`StatusConsumer`:
//! `_can_view`, Heartbeat, `json.dumps(event)`).
//!
//! In-process: [`EventBus`] (tokio `broadcast`). Across process boundaries (worker ≠ web server):
//! [`notify`] writes via `pg_notify('paperrust_status', <json>)`, [`relay_notifications`]
//! reads via `LISTEN` and feeds the local bus.

use serde_json::{Map, Value, json};
use tokio::sync::broadcast;

/// Channel name for `LISTEN/NOTIFY`.
pub const NOTIFY_CHANNEL: &str = "paperrust_status";
/// Channels group upstream.
pub const GROUP: &str = "status_updates";
/// `HEARTBEAT_INTERVAL` (seconds) of the StatusConsumer.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 30;
/// `HEARTBEAT_MESSAGE = json.dumps({"type": "heartbeat"})`.
pub const HEARTBEAT_MESSAGE: &str = "{\"type\": \"heartbeat\"}";

/// `ProgressStatusOptions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStatus {
    Started,
    Working,
    Success,
    Failed,
}

impl ProgressStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "STARTED",
            Self::Working => "WORKING",
            Self::Success => "SUCCESS",
            Self::Failed => "FAILED",
        }
    }
}

/// Permission fields (`PermissionsData`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewPermissions {
    pub owner_id: Option<i64>,
    pub users_can_view: Vec<i64>,
    pub groups_can_view: Vec<i64>,
}

/// `WebsocketPayload` (one of the three forms).
#[derive(Debug, Clone, PartialEq)]
pub enum WsPayload {
    /// `{"type": "status_update", "data": ProgressUpdateData}`
    StatusUpdate {
        filename: Option<String>,
        task_id: Option<String>,
        current_progress: i64,
        max_progress: i64,
        status: ProgressStatus,
        message: String,
        document_id: Option<i64>,
        perms: ViewPermissions,
    },
    /// `{"type": "documents_deleted", "data": {"documents": [...]}}`
    DocumentsDeleted { documents: Vec<i64> },
    /// `{"type": "document_updated", "data": DocumentUpdatedData}`
    DocumentUpdated { document_id: i64, modified: String, perms: ViewPermissions },
}

impl WsPayload {
    /// JSON with upstream key order.
    pub fn to_value(&self) -> Value {
        match self {
            Self::StatusUpdate { filename, task_id, current_progress, max_progress, status, message, document_id, perms } => {
                json!({
                    "type": "status_update",
                    "data": {
                        "filename": filename,
                        "task_id": task_id,
                        "current_progress": current_progress,
                        "max_progress": max_progress,
                        "status": status.as_str(),
                        "message": message,
                        "document_id": document_id,
                        "owner_id": perms.owner_id,
                        "users_can_view": perms.users_can_view,
                        "groups_can_view": perms.groups_can_view,
                    }
                })
            }
            Self::DocumentsDeleted { documents } => json!({"type": "documents_deleted", "data": {"documents": documents}}),
            Self::DocumentUpdated { document_id, modified, perms } => json!({
                "type": "document_updated",
                "data": {
                    "document_id": document_id,
                    "modified": modified,
                    "owner_id": perms.owner_id,
                    "users_can_view": perms.users_can_view,
                    "groups_can_view": perms.groups_can_view,
                }
            }),
        }
    }

    /// From JSON (e.g. from `NOTIFY`).
    pub fn from_value(v: &Value) -> Option<Self> {
        let data = v.get("data")?;
        let ids = |k: &str| -> Vec<i64> { data.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default() };
        let perms = || ViewPermissions {
            owner_id: data.get("owner_id").and_then(Value::as_i64),
            users_can_view: ids("users_can_view"),
            groups_can_view: ids("groups_can_view"),
        };
        match v.get("type")?.as_str()? {
            "status_update" => Some(Self::StatusUpdate {
                filename: data.get("filename").and_then(Value::as_str).map(String::from),
                task_id: data.get("task_id").and_then(Value::as_str).map(String::from),
                current_progress: data.get("current_progress").and_then(Value::as_i64).unwrap_or(0),
                max_progress: data.get("max_progress").and_then(Value::as_i64).unwrap_or(0),
                status: match data.get("status").and_then(Value::as_str)? {
                    "STARTED" => ProgressStatus::Started,
                    "WORKING" => ProgressStatus::Working,
                    "SUCCESS" => ProgressStatus::Success,
                    "FAILED" => ProgressStatus::Failed,
                    _ => return None,
                },
                message: data.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                document_id: data.get("document_id").and_then(Value::as_i64),
                perms: perms(),
            }),
            "documents_deleted" => Some(Self::DocumentsDeleted { documents: ids("documents") }),
            "document_updated" => Some(Self::DocumentUpdated {
                document_id: data.get("document_id")?.as_i64()?,
                modified: data.get("modified").and_then(Value::as_str).unwrap_or("").to_string(),
                perms: perms(),
            }),
            _ => None,
        }
    }

    /// Text the StatusConsumer sends: Python `json.dumps(event)` (separators `", "`/`": "`,
    /// `ensure_ascii=True`).
    pub fn to_ws_text(&self) -> String {
        python_json_dumps(&self.to_value())
    }

    /// Permission fields, if the message is filtered (`documents_deleted` goes to everyone).
    pub fn permissions(&self) -> Option<&ViewPermissions> {
        match self {
            Self::StatusUpdate { perms, .. } | Self::DocumentUpdated { perms, .. } => Some(perms),
            Self::DocumentsDeleted { .. } => None,
        }
    }
}

/// `StatusConsumer._can_view(data)`: superuser, owner, user in `users_can_view` or
/// member of a group from `groups_can_view`.
pub fn can_view(user_id: i64, is_superuser: bool, user_group_ids: &[i64], perms: &ViewPermissions) -> bool {
    if is_superuser || Some(user_id) == perms.owner_id || perms.users_can_view.contains(&user_id) {
        return true;
    }
    user_group_ids.iter().any(|g| perms.groups_can_view.contains(g))
}

/// May the user see the message? (`documents_deleted` always.)
pub fn visible_to(payload: &WsPayload, user_id: i64, is_superuser: bool, user_group_ids: &[i64]) -> bool {
    payload.permissions().is_none_or(|p| can_view(user_id, is_superuser, user_group_ids, p))
}

/// Python `json.dumps(v)` with default options.
pub fn python_json_dumps(v: &Value) -> String {
    let mut out = String::new();
    write_py(v, &mut out);
    out
}

fn write_py(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                out.push_str(&pl_core::json::python_float_repr(f));
            } else {
                out.push_str(&n.to_string());
            }
        }
        Value::String(s) => write_py_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_py(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => write_py_obj(m, out),
    }
}

fn write_py_obj(m: &Map<String, Value>, out: &mut String) {
    out.push('{');
    for (i, (k, x)) in m.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_py_str(k, out);
        out.push_str(": ");
        write_py(x, out);
    }
    out.push('}');
}

fn write_py_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// In-process event channel (several receivers, e.g. one per WebSocket connection).
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<WsPayload>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl EventBus {
    /// New bus with buffer `capacity` (slow receivers lose old messages – like a
    /// full Channels channel).
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self { tx }
    }

    /// `group_send("status_updates", payload)`; without receivers it is dropped (like Channels).
    pub fn send(&self, payload: WsPayload) {
        let _ = self.tx.send(payload);
    }

    /// New receiver (corresponds to `group_add`).
    pub fn subscribe(&self) -> broadcast::Receiver<WsPayload> {
        self.tx.subscribe()
    }
}

/// `ProgressManager(filename, task_id)`.
#[derive(Debug, Clone)]
pub struct ProgressManager {
    bus: EventBus,
    pub filename: Option<String>,
    pub task_id: Option<String>,
}

impl ProgressManager {
    pub fn new(bus: EventBus, filename: Option<String>, task_id: Option<String>) -> Self {
        Self { bus, filename, task_id }
    }

    /// `send_progress(status, message, current, max, document_id=, owner_id=, users_can_view=, groups_can_view=)`.
    pub fn send_progress(&self, status: ProgressStatus, message: &str, current_progress: i64, max_progress: i64, document_id: Option<i64>, perms: ViewPermissions) {
        self.bus.send(WsPayload::StatusUpdate {
            filename: self.filename.clone(),
            task_id: self.task_id.clone(),
            current_progress,
            max_progress,
            status,
            message: message.to_string(),
            document_id,
            perms,
        });
    }
}

/// `DocumentsStatusManager`.
#[derive(Debug, Clone)]
pub struct DocumentsStatusManager {
    bus: EventBus,
}

impl DocumentsStatusManager {
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }
    pub fn send_documents_deleted(&self, documents: Vec<i64>) {
        self.bus.send(WsPayload::DocumentsDeleted { documents });
    }
    pub fn send_document_updated(&self, document_id: i64, modified: String, perms: ViewPermissions) {
        self.bus.send(WsPayload::DocumentUpdated { document_id, modified, perms });
    }
}

/// Send a message across processes (`pg_notify`, payload < 8000 bytes; larger ones are
/// only logged and dropped).
pub async fn notify<'e, E: sqlx::PgExecutor<'e>>(executor: E, payload: &WsPayload) -> Result<(), sqlx::Error> {
    let text = payload.to_value().to_string();
    if text.len() >= 8000 {
        tracing::warn!(len = text.len(), "status payload too large for NOTIFY, dropped");
        return Ok(());
    }
    // PG-specific (pg_notify); SQLite operation only uses the in-process bus.
    sqlx::query("SELECT pg_notify($1, $2)").bind(NOTIFY_CHANNEL).bind(text).execute(executor).await?;
    Ok(())
}

/// Relay wait times between connection attempts (exponential, capped).
#[derive(Debug, Clone, Copy)]
pub struct RelayBackoff {
    pub min: std::time::Duration,
    pub max: std::time::Duration,
}

impl Default for RelayBackoff {
    fn default() -> Self {
        Self { min: std::time::Duration::from_millis(500), max: std::time::Duration::from_secs(30) }
    }
}

/// `LISTEN paperrust_status` and feed every message into `bus` until `shutdown` becomes true.
///
/// If the connection drops (network, server restart, `pg_terminate_backend` → FATAL 57P01, which
/// sqlx does not reconnect on its own), a new listener is built with backoff and `LISTEN`
/// is issued again. The old listener is already discarded then: PostgreSQL does not redeliver
/// NOTIFY, so there is no double delivery; messages during the gap are lost
/// (like Channels/Redis without a subscriber). Returns only on `shutdown` (Ok) or a closed pool.
pub async fn relay_notifications(pool: sqlx::PgPool, bus: EventBus, shutdown: tokio::sync::watch::Receiver<bool>) -> Result<(), sqlx::Error> {
    relay_notifications_with(pool, bus, shutdown, RelayBackoff::default()).await
}

/// [`relay_notifications`] with custom wait times (tests).
pub async fn relay_notifications_with(
    pool: sqlx::PgPool,
    bus: EventBus,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    backoff: RelayBackoff,
) -> Result<(), sqlx::Error> {
    let mut wait = backoff.min;
    let mut connected_once = false;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let err = match relay_once(&pool, &bus, &mut shutdown, &mut connected_once, &mut wait, backoff.min).await {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        if pool.is_closed() {
            return Err(err);
        }
        tracing::warn!(error = %err, retry_in_ms = wait.as_millis() as u64, "status relay (LISTEN paperrust_status) lost, reconnecting");
        tokio::select! {
            r = shutdown.changed() => {
                if r.is_err() || *shutdown.borrow() { return Ok(()); }
            }
            _ = tokio::time::sleep(wait) => {}
        }
        wait = (wait * 2).min(backoff.max);
    }
}

/// One listener connection until error (Err) or `shutdown` (Ok).
async fn relay_once(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    connected_once: &mut bool,
    wait: &mut std::time::Duration,
    min: std::time::Duration,
) -> Result<(), sqlx::Error> {
    // FINDINGS B2: own connection, not from the general pool
    let mut listener = pl_core::dbconn::listener(pool).await?;
    // Report connection drops as errors instead of silently reconnecting inside `recv` – this way every
    // reconnect goes through the same path (fresh LISTEN, backoff, log)
    listener.eager_reconnect(false);
    listener.listen(NOTIFY_CHANNEL).await?;
    if *connected_once {
        tracing::info!("status relay (LISTEN paperrust_status) reconnected");
    }
    *connected_once = true;
    *wait = min;
    loop {
        tokio::select! {
            r = shutdown.changed() => {
                if r.is_err() || *shutdown.borrow() { return Ok(()); }
            }
            n = listener.try_recv() => {
                let Some(n) = n? else {
                    return Err(sqlx::Error::Io(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "listener connection closed")));
                };
                match serde_json::from_str::<Value>(n.payload()).ok().as_ref().and_then(WsPayload::from_value) {
                    Some(p) => bus.send(p),
                    None => tracing::warn!("invalid status notification ignored"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_update_shape_and_text() {
        let p = WsPayload::StatusUpdate {
            filename: Some("Grüße.pdf".into()),
            task_id: Some("abc".into()),
            current_progress: 20,
            max_progress: 100,
            status: ProgressStatus::Working,
            message: "Parsing document...".into(),
            document_id: None,
            perms: ViewPermissions { owner_id: Some(3), users_can_view: vec![], groups_can_view: vec![1] },
        };
        assert_eq!(
            p.to_ws_text(),
            "{\"type\": \"status_update\", \"data\": {\"filename\": \"Gr\\u00fc\\u00dfe.pdf\", \"task_id\": \"abc\", \
             \"current_progress\": 20, \"max_progress\": 100, \"status\": \"WORKING\", \"message\": \"Parsing document...\", \
             \"document_id\": null, \"owner_id\": 3, \"users_can_view\": [], \"groups_can_view\": [1]}}"
        );
        assert_eq!(WsPayload::from_value(&p.to_value()), Some(p.clone()));
        assert!(visible_to(&p, 3, false, &[]));
        assert!(visible_to(&p, 9, false, &[1]));
        assert!(!visible_to(&p, 9, false, &[2]));
        assert!(visible_to(&p, 9, true, &[]));
        let d = WsPayload::DocumentsDeleted { documents: vec![1, 2] };
        assert_eq!(d.to_ws_text(), "{\"type\": \"documents_deleted\", \"data\": {\"documents\": [1, 2]}}");
        assert!(visible_to(&d, 9, false, &[]));
    }

    #[tokio::test]
    async fn bus_fanout() {
        let bus = EventBus::new(8);
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        ProgressManager::new(bus.clone(), None, Some("t".into())).send_progress(ProgressStatus::Started, "x", 0, 100, None, ViewPermissions::default());
        assert!(matches!(a.recv().await.unwrap(), WsPayload::StatusUpdate { status: ProgressStatus::Started, .. }));
        assert!(matches!(b.recv().await.unwrap(), WsPayload::StatusUpdate { .. }));
    }
}
