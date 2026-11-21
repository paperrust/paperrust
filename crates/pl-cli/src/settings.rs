//! `paperrust manage settings list|get|set|reset|export|import|rekey` (docs/SETTINGS.md §2.7, WP2).
//!
//! Runs without a server (rescue path: `settings reset <KEY>`). Writes via `pl_settings::store`
//! (history `action=cli`, NOTIFY to running processes). Secrets are never printed,
//! except with `export --include-secrets`.

use pl_settings::crypto::SecretBox;
use pl_settings::layer::Source;
use pl_settings::migration;
use pl_settings::store::{self, Action, Actor, ApplyOptions, Input};

use crate::ctx::Ctx;
use crate::{CliError, CliResult};

const USAGE: &str = "settings list [--all] [--category C] | get KEY | set KEY VALUE [--if-unset] [--force] | reset KEY [--force] \
| export [--format json|env] [--include-secrets [--passphrase P]] | import FILE [--dry-run] [--passphrase P] | rekey --old-secret-key OLD | origin | adopt (--all | KEY...) | slim-env [FILE] | check-file FILE";

fn actor() -> Actor {
    Actor {
        user_name: Some(format!("cli:{}", std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default())),
        ..Actor::default()
    }
}

fn source_label(s: Source) -> &'static str {
    match s {
        Source::Env => "env",
        Source::File => "file",
        Source::Db => "db",
        Source::AppConfig => "appconfig",
        Source::Desktop => "desktop",
        Source::Default => "default",
    }
}

fn shown(e: &pl_settings::Entry) -> String {
    match (&e.value, e.def.secret) {
        (_, true) if e.unreadable => "<unreadable, please set again>".into(),
        (Some(v), true) if !v.is_empty() => "***".into(),
        (Some(v), _) => v.clone(),
        (None, _) => "<unset>".into(),
    }
}

fn store_err(e: store::StoreError) -> CliError {
    match e {
        store::StoreError::Unknown(_) | store::StoreError::NotWritable(_) | store::StoreError::Invalid(_) => CliError::Usage(e.to_string()),
        other => CliError::Command(other.to_string()),
    }
}

pub async fn command(args: Vec<String>) -> CliResult {
    let Some(sub) = args.first().map(String::as_str) else {
        return Err(CliError::Usage(USAGE.into()));
    };
    let rest = &args[1..];
    let flag = |f: &str| rest.iter().any(|a| a == f);
    let opt = |f: &str| rest.iter().position(|a| a == f).and_then(|i| rest.get(i + 1)).cloned();
    let positional: Vec<&String> = {
        let mut v = Vec::new();
        let mut skip = false;
        for a in rest {
            if skip {
                skip = false;
                continue;
            }
            if matches!(a.as_str(), "--format" | "--category" | "--old-secret-key" | "--passphrase") {
                skip = true;
                continue;
            }
            if !a.starts_with("--") {
                v.push(a);
            }
        }
        v
    };
    // Migration helper with a file (docs/SETTINGS.md §5): also works without a database.
    if matches!(sub, "slim-env" | "check-file") {
        let text = match positional.first() {
            Some(f) => std::fs::read_to_string(f.as_str()).map_err(|e| CliError::Command(format!("{f}: {e}")))?,
            None if sub == "slim-env" => {
                let mut t = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut t).map_err(|e| CliError::Command(e.to_string()))?;
                t
            }
            None => return Err(CliError::Usage("check-file FILE".into())),
        };
        let current = match Ctx::connect().await {
            Ok(c) => {
                let sb = SecretBox::from_secret_key(&c.cfg.secret_key);
                store::load_settings(&c.pool, pl_core::config::Env::from_process_raw(), &sb, None).await.ok()
            }
            Err(_) => None,
        };
        let parsed = migration::parse_file(&text);
        let items = migration::check_pairs(&parsed.pairs, current.as_ref());
        let others: Vec<(String, String)> = parsed.pairs.iter().filter(|(k, _)| !migration::is_ours(k)).cloned().collect();
        if sub == "slim-env" {
            print!("{}", migration::slim_env(&items, &others, false));
        } else {
            if let Some(svc) = &parsed.service {
                println!("# compose service {svc}; env_file: {}", parsed.env_files.join(", "));
            }
            for i in &items {
                println!("{:<48} {:<10} {}", i.name, format!("{:?}", i.verdict).to_lowercase(), i.reason.clone().unwrap_or_default());
            }
        }
        return Ok(());
    }
    let c = Ctx::connect().await?;
    let sb = SecretBox::from_secret_key(&c.cfg.secret_key);
    let env = c.cfg.env.clone();
    match sub {
        "list" | "get" => {
            let s = store::load_settings(&c.pool, env, &sb, None).await.map_err(|e| CliError::Command(e.to_string()))?;
            if sub == "get" {
                let key = positional.first().ok_or_else(|| CliError::Usage("get KEY".into()))?;
                let e = s.get(key).ok_or_else(|| CliError::Usage(format!("unknown setting: {key}")))?;
                println!("{}={}", e.def.key, shown(e));
                println!(
                    "  class {} · source {}{} · {}",
                    e.def.class.as_str(),
                    source_label(e.source),
                    e.source_detail.as_ref().map(|d| format!(" ({d})")).unwrap_or_default(),
                    if e.locked { "locked" } else { "editable" }
                );
                return Ok(());
            }
            let cat = opt("--category");
            for e in s.entries() {
                if e.def.internal || (!flag("--all") && e.source == Source::Default) {
                    continue;
                }
                if cat.as_deref().is_some_and(|c| c != e.def.category.as_str()) {
                    continue;
                }
                println!("{:<48} {:<10} {} {}", e.def.key, source_label(e.source), e.def.class.as_str(), shown(e));
            }
            println!("# rev {} · restart pending: {}", s.rev, s.restart_pending_count());
            Ok(())
        }
        "set" | "reset" => {
            let key = positional.first().ok_or_else(|| CliError::Usage(USAGE.into()))?.to_string();
            let input = if sub == "set" {
                Input::Raw(positional.get(1).ok_or_else(|| CliError::Usage("set KEY VALUE".into()))?.to_string())
            } else {
                Input::Reset
            };
            let o = ApplyOptions { env: (!flag("--force")).then_some(&env), if_unset: flag("--if-unset"), ..ApplyOptions::default() };
            let rev = store::apply(&c.pool, &sb, &[(key.clone(), input)], &o, &actor(), Action::Cli).await.map_err(store_err)?;
            println!("{sub} {key} (rev {rev})");
            Ok(())
        }
        "export" => {
            let inc = flag("--include-secrets");
            match opt("--format").as_deref().unwrap_or("json") {
                "json" => {
                    let mut conn = c.pool.acquire().await.map_err(|e| CliError::Command(e.to_string()))?;
                    let v = store::export(&mut conn, &sb, inc).await.map_err(|e| CliError::Command(e.to_string()))?;
                    let v = match (inc, opt("--passphrase")) {
                        (true, Some(pass)) => pl_settings::transfer::encrypt(&v, &pass).map_err(CliError::Usage)?,
                        _ => v,
                    };
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                }
                "env" => {
                    let s = store::load_settings(&c.pool, env, &sb, None).await.map_err(|e| CliError::Command(e.to_string()))?;
                    print!("{}", store::export_env(&s, inc));
                }
                f => return Err(CliError::Usage(format!("unknown format {f} (json|env)"))),
            }
            if inc && opt("--passphrase").is_none() {
                eprintln!("warning: the export contains secrets in plain text (use --passphrase)");
            }
            Ok(())
        }
        "import" => {
            let file = positional.first().ok_or_else(|| CliError::Usage("import FILE [--dry-run]".into()))?;
            let text = std::fs::read_to_string(file.as_str()).map_err(|e| CliError::Command(format!("{file}: {e}")))?;
            let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| CliError::Command(format!("{file}: {e}")))?;
            let doc = if pl_settings::transfer::is_encrypted(&doc) {
                let pass = opt("--passphrase").ok_or_else(|| CliError::Usage("the file is encrypted: --passphrase P".into()))?;
                pl_settings::transfer::decrypt(&doc, &pass).map_err(CliError::Command)?
            } else {
                doc
            };
            let changes = store::import_changes(&doc).map_err(CliError::Command)?;
            if flag("--dry-run") {
                let s = store::load_settings(&c.pool, env, &sb, None).await.map_err(|e| CliError::Command(e.to_string()))?;
                for (k, v) in &changes {
                    let cur = s.get(k).map(shown).unwrap_or_default();
                    let new = match v {
                        Input::Raw(_) if pl_settings::schema::find(k).is_some_and(|d| d.secret) => "***".to_string(),
                        Input::Raw(r) => r.clone(),
                        Input::Json(j) => j.to_string(),
                        Input::Reset => "<reset>".into(),
                    };
                    println!("{k}: {cur} -> {new}");
                }
                println!("# dry run, {} change(s), nothing written", changes.len());
                return Ok(());
            }
            let rev = store::apply(&c.pool, &sb, &changes, &ApplyOptions::default(), &actor(), Action::Import).await.map_err(store_err)?;
            println!("imported {} setting(s) (rev {rev})", changes.len());
            Ok(())
        }
        "rekey" => {
            let old = opt("--old-secret-key").ok_or_else(|| CliError::Usage("rekey --old-secret-key OLD".into()))?;
            let n = store::rekey(&c.pool, &SecretBox::from_secret_key(&old), &sb, &actor()).await.map_err(store_err)?;
            println!("re-encrypted {n} secret(s)");
            Ok(())
        }
        "origin" | "adopt" => {
            let s = store::load_settings(&c.pool, pl_core::config::Env::from_process_raw(), &sb, None)
                .await
                .map_err(|e| CliError::Command(e.to_string()))?;
            let items = migration::origin(&s);
            if sub == "origin" {
                for i in &items {
                    let v = if i.secret { "***".to_string() } else { i.raw.clone() };
                    println!("{:<48} {:<8} {:<10} {v}", i.name, i.source, format!("{:?}", i.verdict).to_lowercase());
                }
                return Ok(());
            }
            let all = flag("--all");
            let wanted: Vec<&str> = positional.iter().map(|s| s.as_str()).collect();
            if !all && wanted.is_empty() {
                return Err(CliError::Usage("adopt (--all | KEY...)".into()));
            }
            let changes: Vec<(String, Input)> = items
                .iter()
                .filter(|i| i.verdict == migration::Verdict::Adoptable)
                .filter(|i| all || wanted.iter().any(|w| *w == i.name || pl_settings::schema::find(w).map(|d| d.key) == i.key))
                .filter_map(|i| i.key.map(|k| (k.to_string(), Input::Raw(i.raw.clone()))))
                .collect();
            if changes.is_empty() {
                println!("nothing to adopt");
                return Ok(());
            }
            let o = ApplyOptions { expected_rev: None, env: None, if_unset: false };
            let rev = store::apply(&c.pool, &sb, &changes, &o, &actor(), Action::Adopt).await.map_err(store_err)?;
            for (k, _) in &changes {
                println!("adopted {k}");
            }
            println!("# rev {rev} – the variables can now be removed from the environment (see `settings slim-env`)");
            Ok(())
        }
        other => Err(CliError::Usage(format!("unknown subcommand '{other}'. {USAGE}"))),
    }
}
