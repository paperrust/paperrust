//! Enqueue the `process_mail_accounts` task via `pl_core::tasks::send_task` (PostgreSQL queue
//! mounted by pl-server; `PaperlessTask` row like `before_task_publish`).
//!
//! upstream: src/paperless_mail/views.py@v3.4.2 (MailAccountViewSet.process):
//! `process_mail_accounts.apply_async(kwargs={"account_ids": [account.pk]},
//!  headers={"trigger_source": PaperlessTask.TriggerSource.MANUAL})`.

use pl_core::AppState;
use pl_core::tasks::TaskRequest;
use serde_json::json;

use crate::error::ApiResult;

pub const TASK_PROCESS_MAIL_ACCOUNTS: &str = "paperless_mail.tasks.process_mail_accounts";

pub fn process_request(account_ids: &[i32]) -> TaskRequest {
    TaskRequest::new(TASK_PROCESS_MAIL_ACCOUNTS)
        .kwarg("account_ids", json!(account_ids))
        .trigger_source("manual")
}

pub async fn send_process_mail_accounts(state: &AppState, account_ids: &[i32]) -> ApiResult<String> {
    let id = pl_core::tasks::send_task(state, process_request(account_ids)).await?;
    Ok(id.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use pl_core::tasks::{DispatchFuture, Dispatcher, TaskDispatch};
    use uuid::Uuid;

    use super::*;

    #[derive(Default)]
    struct Rec(Mutex<Vec<TaskRequest>>);

    impl TaskDispatch for Rec {
        fn dispatch(&self, req: TaskRequest) -> DispatchFuture<'_> {
            self.0.lock().unwrap().push(req);
            Box::pin(async { Ok(Uuid::nil()) })
        }
    }

    fn state() -> AppState {
        let cfg = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs([(
            "PAPERLESS_SECRET_KEY",
            "pl-mail-tests-only-0123456789abcdefghijklmnopqrstuvwxyz",
        )]))
        .unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://u:p@127.0.0.1:1/x").unwrap();
        AppState::with_pool(cfg, pool).unwrap()
    }

    #[tokio::test]
    async fn goes_to_dispatcher() {
        let st = state();
        let rec = Arc::new(Rec::default());
        st.set_service(Dispatcher(rec.clone()));
        assert_eq!(send_process_mail_accounts(&st, &[3]).await.unwrap(), Uuid::nil().to_string());
        let got = rec.0.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, TASK_PROCESS_MAIL_ACCOUNTS);
        assert_eq!(got[0].kwargs["account_ids"], json!([3]));
        assert_eq!(got[0].headers["trigger_source"], json!("manual"));
    }

    #[test]
    fn request_shape() {
        let r = process_request(&[1, 2]);
        assert_eq!(r.name, TASK_PROCESS_MAIL_ACCOUNTS);
        assert_eq!(r.kwargs["account_ids"], json!([1, 2]));
        assert_eq!(r.headers["trigger_source"], json!("manual"));
        assert!(r.args.is_empty());
    }
}
