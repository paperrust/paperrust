//! Workflow effects (e-mail/webhook) against local sinks: SMTP sink and HTTP receiver on
//! 127.0.0.1. No real mails, no third-party services.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pl_mail::effects::smtp::{self, EmailAttachment, EmailSettings};
use pl_mail::effects::webhook::{self, WebhookError, WebhookFile, WebhookRequest, WebhookSettings};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------------------------
// SMTP sink

#[derive(Debug, Default, Clone)]
struct SmtpLog {
    commands: Vec<String>,
    data: Vec<Vec<u8>>,
}

async fn smtp_sink(auth: &'static str) -> (u16, Arc<Mutex<SmtpLog>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(SmtpLog::default()));
    let log2 = log.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            let log = log2.clone();
            tokio::spawn(async move {
                let (r, mut w) = s.into_split();
                let mut r = BufReader::new(r);
                w.write_all(b"220 sink ESMTP\r\n").await.unwrap();
                let mut line = String::new();
                loop {
                    line.clear();
                    if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let cmd = line.trim_end().to_string();
                    log.lock().unwrap().commands.push(cmd.clone());
                    let up = cmd.to_uppercase();
                    let reply: String = if up.starts_with("EHLO") {
                        format!("250-sink\r\n250-SIZE 1000000\r\n250 AUTH {auth}\r\n")
                    } else if up.starts_with("AUTH PLAIN") {
                        "235 ok\r\n".into()
                    } else if up.starts_with("AUTH LOGIN") {
                        "334 UGFzc3dvcmQ6\r\n".into()
                    } else if up.starts_with("MAIL") || up.starts_with("RCPT") || up.starts_with("RSET") {
                        "250 ok\r\n".into()
                    } else if up == "DATA" {
                        w.write_all(b"354 go\r\n").await.unwrap();
                        let mut data = Vec::new();
                        loop {
                            let mut l = String::new();
                            r.read_line(&mut l).await.unwrap();
                            if l == ".\r\n" {
                                break;
                            }
                            data.extend_from_slice(l.as_bytes());
                        }
                        log.lock().unwrap().data.push(data);
                        "250 queued\r\n".into()
                    } else if up == "QUIT" {
                        w.write_all(b"221 bye\r\n").await.unwrap();
                        return;
                    } else {
                        // password line after AUTH LOGIN
                        "235 ok\r\n".into()
                    };
                    w.write_all(reply.as_bytes()).await.unwrap();
                }
            });
        }
    });
    (port, log)
}

fn email_settings(port: u16) -> EmailSettings {
    EmailSettings {
        host: "127.0.0.1".into(),
        port,
        host_user: "mailer".into(),
        host_password: "secret".into(),
        from_email: "paperless@example.org".into(),
        use_tls: false,
        use_ssl: false,
        timeout: Duration::from_secs(5),
        enabled: true,
        extra_roots: Vec::new(),
    }
}

#[tokio::test]
async fn email_with_attachment_reaches_sink() {
    let (port, log) = smtp_sink("PLAIN LOGIN").await;
    let s = email_settings(port);
    let dir = std::env::temp_dir().join(format!("pl-w6-smtp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("doc.pdf");
    std::fs::write(&f, b"%PDF-1.4 test").unwrap();
    let atts = vec![
        EmailAttachment { path: f.clone(), mime_type: "application/pdf".into(), friendly_name: "Rechnung März.pdf".into() },
        EmailAttachment { path: f.clone(), mime_type: "application/pdf".into(), friendly_name: "Rechnung März.pdf".into() },
    ];
    let to = vec!["a@example.org".to_string(), " b@example.org".to_string()];
    let mail = smtp::build_message(&s, "Dokument\nhinzugefügt", ".leading dot\nBody", &to, &atts).unwrap();
    assert_eq!(smtp::send(&s, &mail).await.unwrap(), 1);
    let l = log.lock().unwrap().clone();
    assert!(l.commands.iter().any(|c| c.starts_with("AUTH PLAIN ")), "{:?}", l.commands);
    assert!(l.commands.iter().any(|c| c.starts_with("mail FROM:<paperless@example.org> size=")));
    assert!(l.commands.contains(&"rcpt TO:<a@example.org>".to_string()));
    assert!(l.commands.contains(&"rcpt TO:<b@example.org>".to_string()));
    let data = String::from_utf8(l.data[0].clone()).unwrap();
    assert!(data.contains("Subject: =?utf-8?b?"), "{data}");
    assert!(data.contains("\r\n..leading dot\r\n"), "dot stuffing: {data}");
    assert!(data.contains("filename*=utf-8''Rechnung%20M%C3%A4rz.pdf"));
    // second attachment with the same name → _01
    assert!(data.contains("filename*=utf-8''Rechnung%20M%C3%A4rz_01.pdf"));
    assert!(data.contains("Content-Type: application/pdf"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn email_login_fallback_and_no_recipients() {
    let (port, log) = smtp_sink("LOGIN").await;
    let s = email_settings(port);
    let mail = smtp::build_message(&s, "s", "b", &["x@example.org".into()], &[]).unwrap();
    smtp::send(&s, &mail).await.unwrap();
    assert!(log.lock().unwrap().commands.iter().any(|c| c.starts_with("AUTH LOGIN ")));
    let empty = smtp::OutgoingMail { from: "a@b".into(), recipients: vec![], data: vec![] };
    assert_eq!(smtp::send(&s, &empty).await.unwrap(), 0);
}

// ---------------------------------------------------------------------------------------------
// HTTP receiver

#[derive(Debug, Clone, Default)]
struct HttpReq {
    head: String,
    body: Vec<u8>,
}

async fn http_receiver(status: u16) -> (u16, Arc<Mutex<Vec<HttpReq>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let reqs = Arc::new(Mutex::new(Vec::new()));
    let r2 = reqs.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let reqs = r2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, mut body) = loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break (String::from_utf8_lossy(&buf[..p]).into_owned(), buf[p + 4..].to_vec());
                    }
                };
                let len: usize = head
                    .lines()
                    .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                    .unwrap_or(0);
                while body.len() < len {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                reqs.lock().unwrap().push(HttpReq { head, body });
                let extra = if status == 302 { "Location: http://169.254.169.254/\r\n" } else { "" };
                let _ = s.write_all(format!("HTTP/1.1 {status} X\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await;
            });
        }
    });
    (port, reqs)
}

fn wh_settings(allow_internal: bool) -> WebhookSettings {
    WebhookSettings::from_env(&pl_core::config::Env::from_pairs([(
        "PAPERLESS_WEBHOOKS_ALLOW_INTERNAL_REQUESTS",
        if allow_internal { "true" } else { "false" },
    )]))
    .unwrap()
}

#[tokio::test]
async fn webhook_json_headers_and_file() {
    let (port, reqs) = http_receiver(200).await;
    let s = wh_settings(true);
    let req = WebhookRequest {
        url: format!("http://127.0.0.1:{port}/hook?x=1"),
        data: json!("{\"id\": \"7\"}"),
        headers: vec![("X-Test".into(), "1".into()), ("Host".into(), "evil.example".into())],
        file: None,
        as_json: true,
    };
    assert_eq!(webhook::send_webhook(&req, &s).await.unwrap(), 200);
    let r = reqs.lock().unwrap()[0].clone();
    assert!(r.head.starts_with("POST /hook?x=1 HTTP/1.1"), "{}", r.head);
    assert!(r.head.to_lowercase().contains("x-test: 1"));
    assert!(!r.head.to_lowercase().contains("evil.example"), "Host header must not be forwarded");
    assert!(r.head.to_lowercase().contains("content-type: application/json"));
    assert_eq!(r.body, b"\"{\\\"id\\\": \\\"7\\\"}\"");

    // form + file → multipart
    let req2 = WebhookRequest {
        url: format!("http://127.0.0.1:{port}/up"),
        data: json!({"title": "Hallo Welt"}),
        headers: vec![],
        file: Some(WebhookFile { name: "a.pdf".into(), content: b"%PDF".to_vec(), mime: "application/pdf".into() }),
        as_json: false,
    };
    webhook::send_webhook(&req2, &s).await.unwrap();
    let r = reqs.lock().unwrap()[1].clone();
    let body = String::from_utf8_lossy(&r.body).into_owned();
    assert!(r.head.to_lowercase().contains("content-type: multipart/form-data; boundary="));
    assert!(body.contains("name=\"title\"\r\n\r\nHallo Welt\r\n"));
    assert!(body.contains("name=\"file\"; filename=\"a.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF\r\n"));
}

#[tokio::test]
async fn webhook_ssrf_blocked_before_connect() {
    let (port, reqs) = http_receiver(200).await;
    let s = wh_settings(false);
    let req = WebhookRequest { url: format!("http://127.0.0.1:{port}/"), data: json!({}), headers: vec![], file: None, as_json: false };
    let e = webhook::send_webhook(&req, &s).await.unwrap_err();
    assert!(matches!(e, WebhookError::Connect(ref m) if m.contains("non-public")), "{e:?}");
    let req = WebhookRequest { url: format!("http://localhost:{port}/"), ..req };
    assert!(matches!(webhook::send_webhook(&req, &s).await, Err(WebhookError::Connect(_))));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(reqs.lock().unwrap().is_empty(), "no request may reach an internal address");
    // allowed ports
    let s2 = WebhookSettings::from_env(&pl_core::config::Env::from_pairs([("PAPERLESS_WEBHOOKS_ALLOWED_PORTS", "443")])).unwrap();
    let req = WebhookRequest { url: format!("http://127.0.0.1:{port}/"), data: json!({}), headers: vec![], file: None, as_json: false };
    assert_eq!(webhook::send_webhook(&req, &s2).await.unwrap_err(), WebhookError::Blocked("Destination port not permitted.".into()));
    let s3 = WebhookSettings::from_env(&pl_core::config::Env::from_pairs([("PAPERLESS_WEBHOOKS_ALLOWED_SCHEMES", "https")])).unwrap();
    assert_eq!(webhook::send_webhook(&req, &s3).await.unwrap_err(), WebhookError::Blocked("Invalid URL scheme or hostname.".into()));
}

#[tokio::test]
async fn webhook_redirect_not_followed_and_retries() {
    let (port, reqs) = http_receiver(302).await;
    let s = wh_settings(true);
    let req = WebhookRequest { url: format!("http://127.0.0.1:{port}/r"), data: json!({}), headers: vec![], file: None, as_json: false };
    assert_eq!(webhook::send_webhook(&req, &s).await.unwrap_err(), WebhookError::Status(302));
    assert_eq!(reqs.lock().unwrap().len(), 1, "redirect must not be followed");

    let (port, reqs) = http_receiver(500).await;
    let req = WebhookRequest { url: format!("http://127.0.0.1:{port}/e"), ..req };
    let t = std::time::Instant::now();
    assert_eq!(webhook::send_with_retries(&req, &s).await.unwrap_err(), WebhookError::Status(500));
    // 1 attempt + 3 retries (max_retries=3), backoff 0..1, 0..2, 0..4 s
    assert_eq!(reqs.lock().unwrap().len(), 4);
    assert!(t.elapsed() <= Duration::from_secs(9));
}

// ---------------------------------------------------------------------------------------------
// With DB (own copy pl_w6_test): actions from the seed (e-mail action 3, webhook action 4)

fn consumer_ctx(action: i64, action_type: i64, doc: Option<i64>, trigger: i64) -> pl_consumer::stages::ActionContext {
    pl_consumer::stages::ActionContext {
        workflow_action_id: action,
        action_type,
        document_id: doc,
        title: String::new(),
        original_file: std::path::PathBuf::from("does-not-exist.pdf"),
        trigger_type: trigger,
    }
}

#[tokio::test]
#[ignore = "braucht die eigene DB-Kopie pl_w6_test (PL_W6_TEST_DB=1, angelegt von worker_parity)"]
async fn workflow_effects_with_db() {
    if std::env::var("PL_W6_TEST_DB").is_err() {
        return;
    }
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect("postgres://paperless:paperless@127.0.0.1:54329/pl_w6_test")
        .await
        .unwrap();
    let (sport, slog) = smtp_sink("PLAIN").await;
    let (hport, reqs) = http_receiver(200).await;
    sqlx::query("UPDATE documents_workflowactionwebhook SET url = $1 WHERE id = 1")
        .bind(format!("http://127.0.0.1:{hport}/hook"))
        .execute(&pool)
        .await
        .unwrap();
    let core = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "pl-w6-tests-only-0123456789abcdefghijklmnopqrstuvwxyz"),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
        ("PAPERLESS_URL", "http://paperless.example"),
    ]))
    .unwrap();
    let mut cfg = pl_mail::effects::EffectsConfig::from_core(&core);
    cfg.commit_wait = Duration::from_secs(2);
    let fx = pl_mail::effects::MailWorkflowEffects {
        pool: pool.clone(),
        queue: None,
        email: Arc::new(email_settings(sport)),
        webhook: Arc::new(wh_settings(true)),
        config: Arc::new(cfg),
        password_removal: None,
        consumption_finished: None,
    };
    let ctx = consumer_ctx(3, 3, Some(1), pl_matching::workflow::TRIGGER_DOCUMENT_UPDATED);
    fx.execute_email(&ctx, None).await;
    let data = String::from_utf8(slog.lock().unwrap().data[0].clone()).unwrap();
    assert!(data.contains("Subject: Document {doc_title} added"), "{data}");
    assert!(data.contains("Body for {doc_url}"), "{data}");
    assert!(slog.lock().unwrap().commands.contains(&"rcpt TO:<ops@example.invalid>".to_string()));

    let wctx = consumer_ctx(4, 4, Some(1), pl_matching::workflow::TRIGGER_DOCUMENT_UPDATED);
    let req = fx.build_webhook(&wctx, None).await.unwrap().unwrap();
    // body {"id": "{doc_id}"} – single braces are plain text in Jinja
    assert_eq!(req.data, json!("{\"id\": \"{doc_id}\"}"));
    assert_eq!(req.headers, vec![("X-Test".to_string(), "1".to_string())]);
    fx.execute_webhook(&wctx, None).await;
    assert_eq!(reqs.lock().unwrap().len(), 1);

    // template with placeholders via a custom document field
    sqlx::query("UPDATE documents_workflowactionwebhook SET body = $1, as_json = false WHERE id = 1")
        .bind("{{ doc_title }}|{{ correspondent }}|{{ doc_url }}|{{ doc_id }}")
        .execute(&pool)
        .await
        .unwrap();
    let req = fx.build_webhook(&wctx, None).await.unwrap().unwrap();
    let title: (String,) = sqlx::query_as("SELECT title FROM documents_document WHERE id = 1").fetch_one(&pool).await.unwrap();
    let s = req.data.as_str().unwrap().to_string();
    assert!(s.starts_with(&format!("{}|", title.0)), "{s}");
    assert!(s.ends_with("|http://paperless.example/documents/1/|1"), "{s}");
    // uncommitted document: action is dropped after the wait time
    let missing = consumer_ctx(4, 4, Some(999_999), pl_matching::workflow::TRIGGER_DOCUMENT_ADDED);
    assert!(fx.build_webhook(&missing, None).await.unwrap().is_none());
}

/// tests/chaos C4: receiver briefly unreachable (port closed) → retry with backoff, the request
/// arrives as soon as it listens again (upstream: ConnectError without retry → webhook lost).
#[tokio::test]
async fn webhook_retries_until_receiver_is_back() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let s = wh_settings(true);
    let req = WebhookRequest { url: format!("http://127.0.0.1:{port}/late"), data: json!({"a": 1}), headers: vec![], file: None, as_json: true };
    assert!(matches!(webhook::send_webhook(&req, &s).await, Err(WebhookError::Connect(ref m)) if webhook::connect_retryable(m)));
    // start the receiver only after 1.5 s (same port)
    let hits = Arc::new(Mutex::new(0usize));
    let h2 = hits.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let l = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        while let Ok((mut sock, _)) = l.accept().await {
            *h2.lock().unwrap() += 1;
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await;
        }
    });
    // backoff 0..1, 0..2, 0..4, 0..8 s (random) – on average this is enough; retry several times if needed
    let mut ok = false;
    for _ in 0..4 {
        if webhook::send_with_retries(&req, &s).await.is_ok() {
            ok = true;
            break;
        }
    }
    assert!(ok, "webhook must arrive once the receiver is back");
    assert!(*hits.lock().unwrap() >= 1);
}

