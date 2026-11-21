//! Sample via API v1 of the running target: title, content hash, tags and the checksum of the
//! downloaded original of some documents against the source. GET calls only.
//!
//! Login: token (`Authorization: Token …`). `--api-temp-token`: the tool takes the token
//! of an active superuser from the target DB or creates one for the duration of the sample and
//! deletes it afterwards (only this one new record is written).
//!
//! upstream: src/documents/views.py (`DocumentViewSet.retrieve`, `download` with `original=true`),
//! rest_framework.authtoken (table authtoken_token: key, created, user_id).

use crate::export::DocRow;
use crate::verify::{Spot, SpotRow};
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::time::Duration;

/// Sample: evenly distributed over the (non-deleted) root documents (API v1 does not show
/// versions individually), first and last always included.
pub fn pick_sample(docs: &[DocRow], n: usize) -> Vec<&DocRow> {
    let live: Vec<&DocRow> = docs.iter().filter(|d| !d.deleted && d.root.is_none()).collect();
    if n == 0 || live.is_empty() {
        return vec![];
    }
    if live.len() <= n {
        return live;
    }
    let mut idx: Vec<usize> = (0..n).map(|i| if n == 1 { 0 } else { i * (live.len() - 1) / (n - 1) }).collect();
    idx.dedup();
    idx.into_iter().map(|i| live[i]).collect()
}

pub struct TempToken {
    pub key: String,
    /// Token was created by us → delete afterwards
    pub created: bool,
}

/// Fetch or create a token of an active superuser.
pub async fn temp_token(target_url: &str) -> Result<TempToken> {
    let mut c = crate::migrate::connect_target(target_url).await?;
    let uid: Option<i32> =
        sqlx::query_scalar("SELECT id FROM auth_user WHERE is_superuser AND is_active ORDER BY id LIMIT 1").fetch_optional(&mut c).await?;
    let uid = uid.ok_or_else(|| anyhow!("kein aktiver Superuser im Ziel – Stichprobe braucht --api-token"))?;
    if let Some(key) =
        sqlx::query_scalar::<_, String>("SELECT key FROM authtoken_token WHERE user_id = $1").bind(uid).fetch_optional(&mut c).await?
    {
        return Ok(TempToken { key, created: false });
    }
    let key = crate::value::hex(&rand::random::<[u8; 20]>());
    sqlx::query("INSERT INTO authtoken_token (key, created, user_id) VALUES ($1, now(), $2)")
        .bind(&key)
        .bind(uid)
        .execute(&mut c)
        .await
        .context("vorübergehenden API-Token anlegen")?;
    Ok(TempToken { key, created: true })
}

pub async fn drop_temp_token(target_url: &str, t: &TempToken) -> Result<()> {
    if t.created {
        let mut c = crate::migrate::connect_target(target_url).await?;
        sqlx::query("DELETE FROM authtoken_token WHERE key = $1").bind(&t.key).execute(&mut c).await?;
    }
    Ok(())
}

fn hash_bytes(b: &[u8], expected: &str) -> String {
    use sha2::Digest;
    if expected.len() == 32 { crate::value::hex(&md5::Md5::digest(b)) } else { crate::value::hex(&sha2::Sha256::digest(b)) }
}

/// Run the sample. `download`: additionally load the original and compute the checksum.
/// What API v1 shows for a root document: content and original of the newest version
/// (upstream `versions_newest_first`: version_index descending, NULL last, then id descending).
pub fn effective<'a>(docs: &'a [DocRow], root: &'a DocRow) -> &'a DocRow {
    docs.iter()
        .filter(|v| v.root == Some(root.id) && !v.deleted)
        .max_by_key(|v| (v.version_index.is_some(), v.version_index, v.id))
        .unwrap_or(root)
}

/// Run the sample. `docs` = all documents of the source (for versions).
pub async fn spot_check(base: &str, token: &str, docs: &[DocRow], sample: &[&DocRow], download: bool) -> Spot {
    let base = base.trim_end_matches('/').to_string();
    let mut spot = Spot { status: "done".into(), api_url: Some(base.clone()), ..Default::default() };
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(120)).build() {
        Ok(c) => c,
        Err(e) => {
            spot.status = "error".into();
            spot.message = Some(e.to_string());
            return spot;
        }
    };
    let auth = format!("Token {token}");
    // Reachability + login once up front (clear message instead of N identical errors)
    match client.get(format!("{base}/api/documents/?page_size=1")).header("Authorization", &auth).send().await {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => {
            spot.status = "error".into();
            spot.message = Some(format!("{base}/api/documents/ antwortet {}", r.status()));
            return spot;
        }
        Err(e) => {
            spot.status = "error".into();
            spot.message = Some(format!("{base} nicht erreichbar: {e}"));
            return spot;
        }
    }
    for d in sample {
        let mut row = SpotRow { id: d.id, title: d.title.clone(), problems: vec![] };
        if let Err(e) = check_doc(&client, &base, &auth, d, effective(docs, d), download, &mut row.problems).await {
            row.problems.push(format!("{e:#}"));
        }
        spot.rows.push(row);
    }
    spot
}

async fn check_doc(
    client: &reqwest::Client,
    base: &str,
    auth: &str,
    d: &DocRow,
    eff: &DocRow,
    download: bool,
    problems: &mut Vec<String>,
) -> Result<()> {
    let r = client.get(format!("{base}/api/documents/{}/", d.id)).header("Authorization", auth).send().await?;
    if !r.status().is_success() {
        problems.push(format!("API antwortet {}", r.status()));
        return Ok(());
    }
    let v: Value = r.json().await.context("Antwort ist kein JSON")?;
    if v.get("title").and_then(Value::as_str) != Some(d.title.as_str()) {
        problems.push(format!("Titel: Quelle {:?}, API {:?}", d.title, v.get("title").and_then(Value::as_str).unwrap_or("")));
    }
    let content = v.get("content").and_then(Value::as_str).unwrap_or("");
    if crate::export::sha256_hex(content.as_bytes()) != eff.content_sha256 {
        problems.push("Inhalt (SHA-256 des Texts) abweichend".into());
    }
    let mut tags: Vec<i64> =
        v.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    tags.sort_unstable();
    if tags != d.tags {
        problems.push(format!("Tags: Quelle {:?}, API {:?}", d.tags, tags));
    }
    if download && !eff.checksum.is_empty() {
        let r = client.get(format!("{base}/api/documents/{}/download/?original=true", d.id)).header("Authorization", auth).send().await?;
        if !r.status().is_success() {
            problems.push(format!("Download des Originals: {}", r.status()));
        } else {
            let body = r.bytes().await?;
            let h = hash_bytes(&body, &eff.checksum);
            if !h.eq_ignore_ascii_case(&eff.checksum) {
                problems.push(format!("Original über API: Prüfsumme {h}, Quelle {}", eff.checksum));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::sha256_hex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn d(id: i64, deleted: bool) -> DocRow {
        DocRow { id, deleted, ..Default::default() }
    }

    #[test]
    fn sample_spread() {
        let docs: Vec<DocRow> = (1..=100).map(|i| d(i, i == 100)).collect();
        let s: Vec<i64> = pick_sample(&docs, 5).iter().map(|d| d.id).collect();
        assert_eq!(s, vec![1, 25, 50, 74, 99]);
        assert_eq!(pick_sample(&docs[..3], 10).len(), 3);
        assert!(pick_sample(&docs, 0).is_empty());
        assert_eq!(pick_sample(&docs, 1).len(), 1);
    }

    #[test]
    fn versions_resolve_like_api() {
        let v = |id, idx: Option<i64>, deleted| DocRow { id, root: Some(1), version_index: idx, deleted, ..Default::default() };
        let docs = vec![d(1, false), v(5, Some(1), false), v(6, Some(2), false), v(7, None, false), v(8, Some(3), true)];
        assert_eq!(effective(&docs, &docs[0]).id, 6, "höchster version_index, gelöschte nicht, NULL zuletzt");
        assert_eq!(effective(&docs[..1], &docs[0]).id, 1);
        assert_eq!(pick_sample(&docs, 10).len(), 1, "Versionen nicht in der Stichprobe");
    }

    /// Mini HTTP server: document 1 correct, document 2 with wrong title/tags/original.
    async fn serve(listener: tokio::net::TcpListener) {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                let authed = req.contains("Authorization: Token good") || req.contains("authorization: Token good");
                let (status, body): (&str, Vec<u8>) = if !authed {
                    ("401 Unauthorized", b"{}".to_vec())
                } else if path.starts_with("/api/documents/?") {
                    ("200 OK", br#"{"count":2,"results":[]}"#.to_vec())
                } else if path == "/api/documents/1/" {
                    ("200 OK", br#"{"id":1,"title":"Eins","content":"hallo","tags":[3,1]}"#.to_vec())
                } else if path == "/api/documents/2/" {
                    ("200 OK", br#"{"id":2,"title":"Falsch","content":"","tags":[]}"#.to_vec())
                } else if path == "/api/documents/1/download/?original=true" {
                    ("200 OK", b"PDF-1".to_vec())
                } else if path == "/api/documents/2/download/?original=true" {
                    ("200 OK", b"kaputt".to_vec())
                } else {
                    ("404 Not Found", b"{}".to_vec())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&body).await;
            });
        }
    }

    #[tokio::test]
    async fn spot_check_against_mock_api() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(serve(l));
        let one = DocRow {
            id: 1,
            title: "Eins".into(),
            content_sha256: sha256_hex(b"hallo"),
            checksum: sha256_hex(b"PDF-1"),
            tags: vec![1, 3],
            ..Default::default()
        };
        let two = DocRow {
            id: 2,
            title: "Zwei".into(),
            content_sha256: sha256_hex(b""),
            checksum: "0".repeat(32),
            tags: vec![4],
            ..Default::default()
        };
        let spot = spot_check(&base, "good", &[], &[&one, &two], true).await;
        assert_eq!(spot.status, "done");
        assert!(spot.rows[0].problems.is_empty(), "{:?}", spot.rows[0].problems);
        let p = spot.rows[1].problems.join(" | ");
        assert!(p.contains("Titel") && p.contains("Tags") && p.contains("Original über API"), "{p}");
        let spot = spot_check(&base, "bad", &[], &[&one], false).await;
        assert_eq!(spot.status, "error");
        assert!(spot.message.unwrap().contains("401"));
        let spot = spot_check("http://127.0.0.1:9", "good", &[], &[&one], false).await;
        assert_eq!(spot.status, "error");
    }
}
