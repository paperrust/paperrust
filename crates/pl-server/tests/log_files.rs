//! Upstream log files for the Logs view: records of the consumer, the task worker and the mail
//! fetcher land in `PAPERLESS_LOGGING_DIR` in upstream's format and the `/api/logs/` core
//! (`pl_system::logs`) lists and reads them. Own test file: sets the global subscriber.

use pl_server::logfile::{self, LogFileConfig, LogFilesLayer};
use pl_system::logs::{LogResult, existing_logs, read_log};
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

fn upstream_line(line: &str, level: &str, name: &str, message: &str) -> bool {
    // [2026-11-09 10:00:00,123] [INFO] [paperless.consumer] message
    let re = regex::Regex::new(
        r"^\[\d{4}-\d\d-\d\d \d\d:\d\d:\d\d,\d{3}\] \[([A-Z]+)\] \[([^\]]+)\] (.*)$",
    )
    .unwrap();
    re.captures(line)
        .is_some_and(|c| &c[1] == level && &c[2] == name && &c[3] == message)
}

#[test]
fn consumer_worker_and_mail_records_reach_the_logs_api() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("log"); // created by the layer like LOGGING_DIR.mkdir()
    let cfg = LogFileConfig {
        dir: dir.clone(),
        max_bytes: 1024 * 1024,
        backups: 20,
        time_zone: Some(chrono_tz::Europe::Zurich),
    };
    let files = LogFilesLayer::new(&cfg).unwrap();
    // both files exist before the first record (upstream handlers open at configuration time)
    assert_eq!(existing_logs(&dir), vec!["paperrust", "mail"]);
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(tracing_subscriber::EnvFilter::new("warn")),
        )
        .with(files.with_filter(logfile::filter()))
        .init();

    tracing::info!(target: "paperless.consumer", "Consuming invoice.pdf");
    tracing::debug!(target: "paperless.tasks", task_id = "abc", "Task started");
    tracing::warn!(target: "paperless.handlers", "Not renaming file");
    tracing::info!(target: "paperless_mail", "Processing mail account Test");
    tracing::info!(target: "pl_consumer::pipeline", "module path target: console only");
    tracing::info!(target: "paperrust::access", "access log: console only");

    let LogResult::Lines(lines) = read_log(&dir, "paperrust", None) else {
        panic!("paperrust.log")
    };
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(
        upstream_line(
            &lines[0],
            "INFO",
            "paperrust.consumer",
            "Consuming invoice.pdf"
        ),
        "{}",
        lines[0]
    );
    assert!(
        upstream_line(
            &lines[1],
            "DEBUG",
            "paperrust.tasks",
            "Task started task_id=abc"
        ),
        "{}",
        lines[1]
    );
    assert!(
        upstream_line(
            &lines[2],
            "WARNING",
            "paperrust.handlers",
            "Not renaming file"
        ),
        "{}",
        lines[2]
    );
    let LogResult::Lines(mail) = read_log(&dir, "mail", None) else {
        panic!("mail.log")
    };
    assert_eq!(mail.len(), 1);
    assert!(
        upstream_line(
            &mail[0],
            "INFO",
            "paperrust_mail",
            "Processing mail account Test"
        ),
        "{}",
        mail[0]
    );
    // ?limit=2 like the UI's request
    let LogResult::Lines(last) = read_log(&dir, "paperrust", Some("2")) else {
        panic!()
    };
    assert_eq!(last.len(), 2);
    assert!(last[1].contains("Not renaming file"));
    // no celery.log without a celery record
    assert_eq!(existing_logs(&dir), vec!["paperrust", "mail"]);
}
