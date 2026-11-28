//! Mail actions and fetch error reporting against the local fake IMAP server (127.0.0.1 only,
//! no database: the pool is lazy and never connected).
//!
//! Regression tests for review findings: no folder-wide EXPUNGE after flag actions, UID EXPUNGE
//! with UIDPLUS, UIDVALIDITY/folder check before the delayed action, Gmail label NO = error,
//! account failures reported by `process_mail_accounts`.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pl_mail::fetch::client::{ConnectOptions, ImapError, MailBox};
use pl_mail::fetch::criteria::{ACTION_DELETE, ACTION_MARK_READ, ACTION_MOVE, ACTION_TAG};
use pl_mail::fetch::db::{Account, Rule};
use pl_mail::fetch::tasks::{AccountOutcome, ActionOutcome, ActionTarget, FetchError, run_mail_action, summarize};
use pl_mail::fetch::{AccountRun, MailWorker, MailWorkerConfig};
use support::fake_imap;

const MAIL: &[u8] = b"From: a@example.org\r\nTo: b@example.org\r\nSubject: Hi\r\nDate: Mon, 6 Jan 2025 08:00:00 +0100\r\n\r\nbody\r\n";

fn opts() -> ConnectOptions {
    ConnectOptions { allow_internal_hosts: true, certificate_file: None, timeout: Duration::from_secs(5), extra_roots: Vec::new() }
}

fn account(port: u16) -> Account {
    Account {
        id: 1,
        name: "fake".into(),
        imap_server: "127.0.0.1".into(),
        imap_port: Some(i32::from(port)),
        imap_security: pl_mail::imap::SECURITY_NONE,
        username: "user".into(),
        password: "pass".into(),
        character_set: "UTF-8".into(),
        is_token: false,
        account_type: 1,
        refresh_token: None,
        expiration: None,
    }
}

fn rule(action: i16, parameter: &str) -> Rule {
    Rule {
        id: 10,
        name: "r".into(),
        account_id: 1,
        account_name: "fake".into(),
        enabled: true,
        folder: "INBOX".into(),
        filter_from: None,
        filter_to: None,
        filter_subject: None,
        filter_body: None,
        filter_attachment_filename_include: None,
        filter_attachment_filename_exclude: None,
        maximum_age: 30,
        action,
        action_parameter: Some(parameter.into()),
        assign_title_from: 1,
        assign_correspondent_from: 1,
        assign_correspondent_id: None,
        assign_document_type_id: None,
        assign_owner_from_rule: false,
        order: 0,
        attachment_type: 1,
        consumption_scope: 1,
        owner_id: None,
        stop_processing: false,
    }
}

fn target(uid: u32, uid_validity: Option<&str>) -> ActionTarget {
    ActionTarget { uid: uid.to_string(), folder: Some("INBOX".into()), account_id: Some(1), uid_validity: uid_validity.map(str::to_string) }
}

/// INBOX (UIDVALIDITY 7) with the imported mail (uid 1) and a mail another client marked
/// `\Deleted` (uid 2, waiting for that client's own expunge); folder `Done` (UIDVALIDITY 9).
fn mailbox() -> fake_imap::Shared {
    let mut s = fake_imap::State::default();
    s.add_folder("INBOX", 7);
    s.add_folder("Done", 9);
    s.append("INBOX", MAIL.to_vec(), &[]);
    s.append("INBOX", MAIL.to_vec(), &["\\Deleted"]);
    Arc::new(Mutex::new(s))
}

fn inbox_uids(st: &fake_imap::Shared) -> Vec<u32> {
    st.lock().unwrap().folders["INBOX"].msgs.iter().map(|m| m.uid).collect()
}

fn commands(st: &fake_imap::Shared) -> Vec<String> {
    st.lock().unwrap().log.iter().map(|(_, c)| c.clone()).collect()
}

fn cfg(capabilities: &str) -> fake_imap::Config {
    fake_imap::Config { capabilities: capabilities.into(), ..Default::default() }
}

// ---------------------------------------------------------------------------------------------
// 1. No folder-wide EXPUNGE after flag actions; UID EXPUNGE for delete/move with UIDPLUS

#[tokio::test]
async fn mark_read_does_not_expunge_foreign_deleted_mail() {
    let st = mailbox();
    let port = fake_imap::start(st.clone(), fake_imap::Config::default()).await;
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_MARK_READ, ""), &target(1, Some("7"))).await;
    assert_eq!(r, Ok(ActionOutcome::Success));
    assert_eq!(inbox_uids(&st), vec![1, 2], "the other client's \\Deleted mail must survive");
    assert!(!commands(&st).iter().any(|c| c.contains("EXPUNGE")), "{:?}", commands(&st));
    let flags = &st.lock().unwrap().folders["INBOX"].msgs[0].flags;
    assert!(flags.contains("\\Seen"));
}

#[tokio::test]
async fn flag_call_sends_no_expunge() {
    let st = mailbox();
    let port = fake_imap::start(st.clone(), fake_imap::Config::default()).await;
    let mut mb = MailBox::connect("127.0.0.1", port, pl_mail::imap::SECURITY_NONE, &opts()).await.unwrap();
    mb.login("user", "pass", false).await.unwrap();
    mb.folder_set("INBOX").await.unwrap();
    mb.flag("1", &["\\Flagged".into()], true).await.unwrap();
    assert!(mb.logout().await);
    assert_eq!(inbox_uids(&st), vec![1, 2]);
    assert!(!commands(&st).iter().any(|c| c.contains("EXPUNGE")));
}

#[tokio::test]
async fn delete_with_uidplus_expunges_only_this_uid() {
    let st = mailbox();
    let port = fake_imap::start(st.clone(), cfg("IMAP4rev1 AUTH=PLAIN UIDPLUS")).await;
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_DELETE, ""), &target(1, Some("7"))).await;
    assert_eq!(r, Ok(ActionOutcome::Success));
    assert_eq!(inbox_uids(&st), vec![2]);
    let cmds = commands(&st);
    assert!(cmds.iter().any(|c| c == "UID EXPUNGE 1"), "{cmds:?}");
    assert!(!cmds.iter().any(|c| c == "EXPUNGE"), "{cmds:?}");
}

#[tokio::test]
async fn delete_uses_uidplus_announced_after_login() {
    let st = mailbox();
    let c = fake_imap::Config {
        capabilities: "IMAP4rev1 AUTH=PLAIN".into(),
        login_capabilities: Some("IMAP4rev1 UIDPLUS MOVE".into()),
        ..Default::default()
    };
    let port = fake_imap::start(st.clone(), c).await;
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_DELETE, ""), &target(1, Some("7"))).await;
    assert_eq!(r, Ok(ActionOutcome::Success));
    assert_eq!(inbox_uids(&st), vec![2]);
    assert!(commands(&st).iter().any(|c| c == "UID EXPUNGE 1"));
}

#[tokio::test]
async fn move_without_move_capability_uses_uid_expunge() {
    let st = mailbox();
    let port = fake_imap::start(st.clone(), cfg("IMAP4rev1 AUTH=PLAIN UIDPLUS")).await;
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_MOVE, "Done"), &target(1, Some("7"))).await;
    assert_eq!(r, Ok(ActionOutcome::Success));
    assert_eq!(inbox_uids(&st), vec![2]);
    assert_eq!(st.lock().unwrap().folders["Done"].msgs.len(), 1);
    assert!(!commands(&st).iter().any(|c| c == "EXPUNGE"));
}

#[tokio::test]
async fn delete_without_uidplus_falls_back_to_folder_expunge() {
    // Documented fallback (Known differences): only EXPUNGE exists without UIDPLUS.
    let st = mailbox();
    let port = fake_imap::start(st.clone(), cfg("IMAP4rev1 AUTH=PLAIN")).await;
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_DELETE, ""), &target(1, Some("7"))).await;
    assert_eq!(r, Ok(ActionOutcome::Success));
    assert!(commands(&st).iter().any(|c| c == "EXPUNGE"));
    assert_eq!(inbox_uids(&st), Vec::<u32>::new());
}

// ---------------------------------------------------------------------------------------------
// 2. Delayed action must not hit another message

#[tokio::test]
async fn changed_uidvalidity_skips_the_action() {
    let st = mailbox();
    let port = fake_imap::start(st.clone(), cfg("IMAP4rev1 AUTH=PLAIN UIDPLUS")).await;
    // imported under UIDVALIDITY 5; the folder was recreated since (now 7), UID 1 is another mail
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_DELETE, ""), &target(1, Some("5"))).await;
    assert!(matches!(r, Ok(ActionOutcome::Skipped { ref reason }) if reason.contains("UIDVALIDITY")), "{r:?}");
    assert_eq!(inbox_uids(&st), vec![1, 2]);
    let cmds = commands(&st);
    assert!(!cmds.iter().any(|c| c.starts_with("UID STORE")), "{cmds:?}");
    assert!(cmds.iter().any(|c| c == "LOGOUT"));
}

#[tokio::test]
async fn same_uidvalidity_runs_the_action() {
    let st = mailbox();
    let port = fake_imap::start(st.clone(), cfg("IMAP4rev1 AUTH=PLAIN UIDPLUS")).await;
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_DELETE, ""), &target(1, Some("7"))).await;
    assert_eq!(r, Ok(ActionOutcome::Success));
}

#[tokio::test]
async fn unknown_stored_uidvalidity_keeps_upstream_behaviour() {
    let st = mailbox();
    let port = fake_imap::start(st.clone(), cfg("IMAP4rev1 AUTH=PLAIN UIDPLUS")).await;
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_DELETE, ""), &target(1, None)).await;
    assert_eq!(r, Ok(ActionOutcome::Success));
}

#[tokio::test]
async fn changed_rule_folder_skips_without_connecting() {
    let st = mailbox();
    let port = fake_imap::start(st.clone(), fake_imap::Config::default()).await;
    let mut t = target(1, Some("7"));
    t.folder = Some("Old".into());
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_DELETE, ""), &t).await;
    assert!(matches!(r, Ok(ActionOutcome::Skipped { ref reason }) if reason.contains("folder changed")), "{r:?}");
    assert_eq!(st.lock().unwrap().connections, 0);
}

#[tokio::test]
async fn changed_rule_account_skips_without_connecting() {
    let st = mailbox();
    let port = fake_imap::start(st.clone(), fake_imap::Config::default()).await;
    let mut t = target(1, Some("7"));
    t.account_id = Some(2);
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_DELETE, ""), &t).await;
    assert!(matches!(r, Ok(ActionOutcome::Skipped { .. })), "{r:?}");
    assert_eq!(st.lock().unwrap().connections, 0);
}

#[test]
fn target_from_kwargs_reads_paperrust_fields() {
    let kw = serde_json::json!({"rule_id": 10, "message_uid": "4", "uid_validity": "7", "folder": "INBOX", "account_id": 1});
    let t = ActionTarget::from_kwargs(kw.as_object().unwrap());
    assert_eq!(t, ActionTarget { uid: "4".into(), folder: Some("INBOX".into()), account_id: Some(1), uid_validity: Some("7".into()) });
    // tasks queued by older versions: no folder/account → no check
    let old = serde_json::json!({"rule_id": 10, "message_uid": "4", "uid_validity": null});
    let t = ActionTarget::from_kwargs(old.as_object().unwrap());
    assert_eq!((t.folder, t.account_id, t.uid_validity), (None, None, None));
}

// ---------------------------------------------------------------------------------------------
// 5. Gmail label rejected by the server

#[tokio::test]
async fn rejected_gmail_label_is_an_error() {
    let st = mailbox();
    let c = fake_imap::Config { capabilities: "IMAP4rev1 AUTH=PLAIN X-GM-EXT-1".into(), reject_gmail_labels: true, ..Default::default() };
    let port = fake_imap::start(st.clone(), c).await;
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_TAG, "paperless"), &target(1, Some("7"))).await;
    assert_eq!(r, Err(("MailboxTaggingError".to_string(), "\"NO\" received".to_string())));
}

#[tokio::test]
async fn accepted_gmail_label_succeeds() {
    let st = mailbox();
    let port = fake_imap::start(st.clone(), cfg("IMAP4rev1 AUTH=PLAIN X-GM-EXT-1")).await;
    let r = run_mail_action(&opts(), &account(port), &rule(ACTION_TAG, "paperless"), &target(1, Some("7"))).await;
    assert_eq!(r, Ok(ActionOutcome::Success));
    assert!(st.lock().unwrap().folders["INBOX"].msgs[0].labels.contains("paperless"));
}

#[tokio::test]
async fn gmail_label_no_from_mailbox_call() {
    let st = mailbox();
    let c = fake_imap::Config { capabilities: "IMAP4rev1 AUTH=PLAIN X-GM-EXT-1".into(), reject_gmail_labels: true, ..Default::default() };
    let port = fake_imap::start(st.clone(), c).await;
    let mut mb = MailBox::connect("127.0.0.1", port, pl_mail::imap::SECURITY_NONE, &opts()).await.unwrap();
    mb.login("user", "pass", false).await.unwrap();
    mb.folder_set("INBOX").await.unwrap();
    assert_eq!(mb.selected_uidvalidity, Some(7));
    let r = mb.gmail_label("1", Some("paperless")).await;
    assert!(matches!(r, Err(ImapError::Status { kind: "MailboxTaggingError", .. })), "{r:?}");
    mb.logout().await;
}

// ---------------------------------------------------------------------------------------------
// 4. Account failures reach the fetch result

struct NoQueue;

impl pl_queue::TaskQueue for NoQueue {
    fn enqueue<'a>(
        &'a self,
        _task_name: &'a str,
        _kwargs: serde_json::Map<String, serde_json::Value>,
        _opts: pl_queue::EnqueueOptions,
    ) -> futures::future::BoxFuture<'a, Result<uuid::Uuid, pl_queue::QueueError>> {
        Box::pin(async { Err(pl_queue::QueueError::Unsupported("test".into())) })
    }

    fn status(&self, _task_id: uuid::Uuid) -> futures::future::BoxFuture<'_, Result<Option<pl_queue::TaskInfo>, pl_queue::QueueError>> {
        Box::pin(async { Ok(None) })
    }
}

fn worker() -> MailWorker {
    let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://nobody:none@127.0.0.1:1/none").unwrap();
    let tz: chrono_tz::Tz = "Europe/Berlin".parse().unwrap();
    let mail = pl_mail::ctx::MailConfig {
        time_zone: tz,
        allow_internal_hosts: true,
        certificate_file: None,
        gmail_client_id: None,
        gmail_client_secret: None,
        outlook_client_id: None,
        outlook_client_secret: None,
        gmail_token_url: "http://127.0.0.1:1/".into(),
        outlook_token_url: "http://127.0.0.1:1/".into(),
        imap_timeout: Duration::from_secs(5),
        proxy_ssl_header: None,
    };
    let config = MailWorkerConfig {
        time_zone: tz,
        scratch_dir: std::env::temp_dir().join("paperrust-mail-actions-test"),
        connect: opts(),
        mail,
        audit_log_enabled: false,
        gpg: Default::default(),
    };
    MailWorker::new(pool, Arc::new(NoQueue), config)
}

#[tokio::test]
async fn unreachable_account_is_reported_as_failure() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let w = worker();
    let mut run = AccountRun::new(&w);
    assert_eq!(run.handle_mail_account(account(port)).await, Ok(0));
    assert!(run.failure().is_some_and(|f| f.starts_with("Error while retrieving mailbox")), "{:?}", run.failure());
}

#[test]
fn all_accounts_failed_is_a_task_failure() {
    let outcomes = vec![
        AccountOutcome { name: "a".into(), error: Some("Error while authenticating account".into()) },
        AccountOutcome { name: "b".into(), error: Some("Error while retrieving mailbox: connect error".into()) },
    ];
    let r = summarize(0, &outcomes);
    assert!(matches!(&r, Err(FetchError::AllAccountsFailed(m)) if m.starts_with("All 2 mail account(s) failed: a: ")), "{r:?}");
}

#[test]
fn partial_failure_keeps_result_and_names_the_account() {
    let outcomes = vec![
        AccountOutcome { name: "a".into(), error: None },
        AccountOutcome { name: "b".into(), error: Some("Error while authenticating account".into()) },
    ];
    assert_eq!(
        summarize(2, &outcomes),
        Ok("Added 2 document(s). 1 of 2 mail account(s) failed: b: Error while authenticating account".into())
    );
}

#[test]
fn no_failure_keeps_upstream_texts() {
    let ok = vec![AccountOutcome { name: "a".into(), error: None }];
    assert_eq!(summarize(0, &ok), Ok("No new documents were added.".into()));
    assert_eq!(summarize(3, &ok), Ok("Added 3 document(s).".into()));
    assert_eq!(summarize(0, &[]), Ok("No new documents were added.".into()));
}
