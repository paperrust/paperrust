//! Connection test against a local fake IMAP server (127.0.0.1 only, no real servers).
//!
//! Checks the error classes like upstream: success / MailError (→ 400) / other exception (→ 500).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use pl_mail::imap::{self, BeforeLogin, ConnectOptions, TestAccount, TestError};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Clone)]
struct Behaviour {
    greeting: &'static str,
    capability: &'static str,
    user: &'static str,
    pass: &'static str,
    select: &'static str,
    send_bye: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Behaviour {
            greeting: "* OK fake IMAP ready",
            capability: "* CAPABILITY IMAP4rev1 AUTH=PLAIN AUTH=XOAUTH2",
            user: "admin",
            pass: "secret",
            select: "OK [READ-WRITE] SELECT completed",
            send_bye: true,
        }
    }
}

/// Starts a single-connection server and returns the port + recording of the commands.
async fn serve(b: Behaviour) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    tokio::spawn(async move {
        let Ok((sock, _)) = listener.accept().await else { return };
        let (r, mut w) = sock.into_split();
        let mut lines = BufReader::new(r).lines();
        w.write_all(format!("{}\r\n", b.greeting).as_bytes()).await.unwrap();
        while let Ok(Some(line)) = lines.next_line().await {
            log2.lock().unwrap().push(line.clone());
            let mut parts = line.splitn(3, ' ');
            let tag = parts.next().unwrap_or("").to_string();
            let cmd = parts.next().unwrap_or("").to_ascii_uppercase();
            let rest = parts.next().unwrap_or("").to_string();
            let reply = match cmd.as_str() {
                "CAPABILITY" => format!("{}\r\n{tag} OK CAPABILITY completed\r\n", b.capability),
                "LOGIN" => {
                    let expect = format!("\"{}\" \"{}\"", b.user, b.pass);
                    if rest == expect {
                        format!("{tag} OK LOGIN completed\r\n")
                    } else {
                        format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")
                    }
                }
                "AUTHENTICATE" => {
                    w.write_all(b"+ \r\n").await.unwrap();
                    let Ok(Some(resp)) = lines.next_line().await else { return };
                    log2.lock().unwrap().push(format!("<auth> {resp}"));
                    let raw = base64::engine::general_purpose::STANDARD.decode(resp.trim()).unwrap_or_default();
                    let ok = match rest.to_ascii_uppercase().as_str() {
                        "PLAIN" => raw == [b"\0".as_slice(), b.user.as_bytes(), b"\0", b.pass.as_bytes()].concat(),
                        "XOAUTH2" => raw == format!("user={}\x01auth=Bearer {}\x01\x01", b.user, b.pass).into_bytes(),
                        _ => false,
                    };
                    if ok {
                        format!("{tag} OK AUTHENTICATE completed\r\n")
                    } else {
                        format!("{tag} NO AUTHENTICATE failed\r\n")
                    }
                }
                "SELECT" => format!("* 0 EXISTS\r\n* 0 RECENT\r\n{tag} {}\r\n", b.select),
                "STARTTLS" => format!("{tag} BAD not supported here\r\n"),
                "LOGOUT" => {
                    let bye = if b.send_bye { "* BYE logging out\r\n" } else { "" };
                    w.write_all(format!("{bye}{tag} OK LOGOUT completed\r\n").as_bytes()).await.unwrap();
                    return;
                }
                _ => format!("{tag} BAD unknown command\r\n"),
            };
            if w.write_all(reply.as_bytes()).await.is_err() {
                return;
            }
        }
    });
    (port, log)
}

fn account(port: u16, user: &str, pass: &str) -> TestAccount {
    TestAccount {
        imap_server: "127.0.0.1".into(),
        imap_port: port,
        imap_security: imap::SECURITY_NONE,
        username: user.into(),
        password: pass.into(),
        is_token: false,
    }
}

fn opts() -> ConnectOptions {
    ConnectOptions {
        allow_internal_hosts: true,
        certificate_file: None,
        timeout: Duration::from_secs(5),
        extra_roots: Vec::new(),
    }
}

async fn run(acc: TestAccount, o: &ConnectOptions) -> Result<(), TestError> {
    imap::test_connection(acc, o, |a| async move { BeforeLogin::Proceed(a) }).await
}

#[tokio::test]
async fn login_success_selects_inbox_and_logs_out() {
    let (port, log) = serve(Behaviour::default()).await;
    assert_eq!(run(account(port, "admin", "secret"), &opts()).await, Ok(()));
    let log = log.lock().unwrap().clone();
    let cmds: Vec<String> = log.iter().map(|l| l.split_once(' ').map(|x| x.1).unwrap_or("").to_string()).collect();
    assert_eq!(cmds, vec!["CAPABILITY", "LOGIN \"admin\" \"secret\"", "SELECT \"INBOX\"", "LOGOUT"]);
}

#[tokio::test]
async fn wrong_password_is_mail_error() {
    let (port, _) = serve(Behaviour::default()).await;
    assert!(matches!(run(account(port, "admin", "notcorrect"), &opts()).await, Err(TestError::Mail(_))));
}

#[tokio::test]
async fn non_ascii_password_uses_auth_plain() {
    let b = Behaviour { pass: "gëheim", ..Behaviour::default() };
    let (port, log) = serve(b).await;
    assert_eq!(run(account(port, "admin", "gëheim"), &opts()).await, Ok(()));
    assert!(log.lock().unwrap().iter().any(|l| l.ends_with("AUTHENTICATE PLAIN")));
}

#[tokio::test]
async fn token_uses_xoauth2() {
    let (port, log) = serve(Behaviour { pass: "tok123", ..Behaviour::default() }).await;
    let mut acc = account(port, "admin", "tok123");
    acc.is_token = true;
    assert_eq!(run(acc, &opts()).await, Ok(()));
    assert!(log.lock().unwrap().iter().any(|l| l.ends_with("AUTHENTICATE XOAUTH2")));
}

#[tokio::test]
async fn read_only_inbox_is_mail_error() {
    let b = Behaviour { select: "OK [READ-ONLY] SELECT completed", ..Behaviour::default() };
    let (port, _) = serve(b).await;
    assert!(matches!(run(account(port, "admin", "secret"), &opts()).await, Err(TestError::Mail(_))));
}

#[tokio::test]
async fn select_failure_is_mail_error() {
    let b = Behaviour { select: "NO no such mailbox", ..Behaviour::default() };
    let (port, _) = serve(b).await;
    assert!(matches!(run(account(port, "admin", "secret"), &opts()).await, Err(TestError::Mail(_))));
}

#[tokio::test]
async fn logout_without_bye_is_server_error() {
    let b = Behaviour { send_bye: false, ..Behaviour::default() };
    let (port, _) = serve(b).await;
    assert!(matches!(run(account(port, "admin", "secret"), &opts()).await, Err(TestError::Other(_))));
}

#[tokio::test]
async fn preauth_greeting_makes_login_fail() {
    let b = Behaviour { greeting: "* PREAUTH already authenticated", ..Behaviour::default() };
    let (port, _) = serve(b).await;
    assert!(matches!(run(account(port, "admin", "secret"), &opts()).await, Err(TestError::Mail(_))));
}

#[tokio::test]
async fn non_imap4_server_is_server_error() {
    let b = Behaviour { capability: "* CAPABILITY FOO", ..Behaviour::default() };
    let (port, _) = serve(b).await;
    assert!(matches!(run(account(port, "admin", "secret"), &opts()).await, Err(TestError::Other(_))));
}

#[tokio::test]
async fn bad_greeting_is_server_error() {
    let b = Behaviour { greeting: "* BYE go away", ..Behaviour::default() };
    let (port, _) = serve(b).await;
    assert!(matches!(run(account(port, "admin", "secret"), &opts()).await, Err(TestError::Other(_))));
}

#[tokio::test]
async fn starttls_without_capability_is_server_error() {
    let (port, _) = serve(Behaviour::default()).await;
    let mut acc = account(port, "admin", "secret");
    acc.imap_security = imap::SECURITY_STARTTLS;
    assert!(matches!(run(acc, &opts()).await, Err(TestError::Other(_))));
}

#[tokio::test]
async fn starttls_on_993_is_server_error_without_connecting() {
    let mut acc = account(993, "admin", "secret");
    acc.imap_security = imap::SECURITY_STARTTLS;
    assert!(matches!(run(acc, &opts()).await, Err(TestError::Other(_))));
}

#[tokio::test]
async fn connection_refused_is_server_error() {
    // occupy the port and release it → very likely closed
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    assert!(matches!(run(account(port, "admin", "secret"), &opts()).await, Err(TestError::Other(_))));
}

#[tokio::test]
async fn internal_host_blocked_when_not_allowed() {
    let (port, _) = serve(Behaviour::default()).await;
    let mut o = opts();
    o.allow_internal_hosts = false;
    assert!(matches!(run(account(port, "admin", "secret"), &o).await, Err(TestError::Mail(_))));
}

#[tokio::test]
async fn failed_refresh_is_mail_error_and_still_logs_out() {
    let (port, log) = serve(Behaviour::default()).await;
    let r = imap::test_connection(account(port, "admin", "secret"), &opts(), |_| async {
        BeforeLogin::Fail("Unable to refresh oauth token".into())
    })
    .await;
    assert!(matches!(r, Err(TestError::Mail(_))));
    assert!(log.lock().unwrap().iter().any(|l| l.ends_with("LOGOUT")));
}

#[tokio::test]
async fn tls_against_plain_server_is_server_error() {
    let (port, _) = serve(Behaviour::default()).await;
    let mut acc = account(port, "admin", "secret");
    acc.imap_security = imap::SECURITY_SSL;
    assert!(matches!(run(acc, &opts()).await, Err(TestError::Other(_))));
}
