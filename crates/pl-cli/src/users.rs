//! `createsuperuser`, `manage_superuser`, `changepassword`.
//!
//! upstream: django/contrib/auth/management/commands/createsuperuser.py, changepassword.py
//! (Django 5.2), django/contrib/auth/password_validation.py (`AUTH_PASSWORD_VALIDATORS` from
//! src/paperless/settings/__init__.py: UserAttributeSimilarity, MinimumLength(8), CommonPassword,
//! Numeric), django/contrib/auth/base_user.py (`normalize_email`),
//! src/documents/management/commands/manage_superuser.py (v3.4.2).

use std::io::{BufRead, Write};

use sqlx::{PgPool, Row};

use crate::args::{self, flag, value};
use crate::ctx::Ctx;
use crate::{CliError, CliResult};

static COMMON_PASSWORDS_GZ: &[u8] = include_bytes!("../baseline/common-passwords.txt.gz");

fn common_passwords() -> &'static std::collections::HashSet<String> {
    static SET: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();
    SET.get_or_init(|| {
        use std::io::Read;
        let mut s = String::new();
        let _ = flate2::read::GzDecoder::new(COMMON_PASSWORDS_GZ).read_to_string(&mut s);
        s.lines().map(|l| l.trim().to_string()).collect()
    })
}

/// Fields read by the similarity validator.
#[derive(Debug, Default, Clone)]
pub struct UserAttrs {
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
}

/// `difflib.SequenceMatcher(a, b).quick_ratio()`.
fn quick_ratio(a: &[char], b: &[char]) -> f64 {
    use std::collections::HashMap;
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let mut full: HashMap<char, i64> = HashMap::new();
    for c in b {
        *full.entry(*c).or_default() += 1;
    }
    let mut avail: HashMap<char, i64> = HashMap::new();
    let mut matches = 0usize;
    for c in a {
        let numb = *avail.get(c).unwrap_or_else(|| full.get(c).unwrap_or(&0));
        avail.insert(*c, numb - 1);
        if numb > 0 {
            matches += 1;
        }
    }
    2.0 * matches as f64 / total as f64
}

/// Python `re.split(r"\W+", s)` (Unicode `\w` = letters, digits, `_`).
fn split_nonword(s: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut in_sep = false;
    for c in s.chars() {
        if c.is_alphanumeric() || c == '_' {
            if in_sep {
                out.push(String::new());
                in_sep = false;
            }
            if let Some(l) = out.last_mut() {
                l.push(c);
            }
        } else {
            in_sep = true;
        }
    }
    if in_sep {
        out.push(String::new());
    }
    out
}

/// `validate_password(password, user)` with the paperless validators; returns the error messages.
pub fn validate_password(password: &str, user: Option<&UserAttrs>) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(u) = user {
        let pw: Vec<char> = password.to_lowercase().chars().collect();
        'attrs: for (value, verbose) in
            [(&u.username, "username"), (&u.first_name, "first name"), (&u.last_name, "last name"), (&u.email, "email address")]
        {
            if value.is_empty() {
                continue;
            }
            let lower = value.to_lowercase();
            let mut parts = split_nonword(&lower);
            parts.push(lower.clone());
            for part in parts {
                let part: Vec<char> = part.chars().collect();
                let pwd_len = pw.len() as f64;
                if pw.len() >= 10 * part.len() && (part.len() as f64) < 0.7 / 2.0 * pwd_len {
                    continue;
                }
                if quick_ratio(&pw, &part) >= 0.7 {
                    errors.push(format!("The password is too similar to the {verbose}."));
                    break 'attrs;
                }
            }
        }
    }
    if password.chars().count() < 8 {
        errors.push("This password is too short. It must contain at least 8 characters.".into());
    }
    if common_passwords().contains(password.to_lowercase().trim()) {
        errors.push("This password is too common.".into());
    }
    if py_isdigit(password) {
        errors.push("This password is entirely numeric.".into());
    }
    errors
}

/// Python `str.isdigit()` (approximately: `char::is_numeric` without fractions and Roman
/// numerals; known difference: rare No characters).
fn py_isdigit(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_numeric() && !matches!(c, '\u{00BC}'..='\u{00BE}' | '\u{2150}'..='\u{218B}'))
}

/// `BaseUserManager.normalize_email`.
pub fn normalize_email(email: &str) -> String {
    let e = email.trim();
    match e.rsplit_once('@') {
        Some((name, domain)) => format!("{name}@{}", domain.to_lowercase()),
        None => email.to_string(),
    }
}

/// `UnicodeUsernameValidator` (`^[\w.@+-]+\Z`) + `max_length=150`.
pub fn validate_username(username: &str) -> Result<(), String> {
    if username.chars().count() > 150 {
        return Err(format!("Ensure this value has at most 150 characters (it has {}).", username.chars().count()));
    }
    if username.is_empty() || !username.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '@' | '+' | '-')) {
        return Err("Enter a valid username. This value may contain only letters, numbers, and @/./+/-/_ characters.".into());
    }
    Ok(())
}

async fn username_exists(pool: &PgPool, username: &str) -> CliResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM auth_user WHERE username = $1)").bind(username).fetch_one(pool).await?)
}

/// `User.objects.create_superuser(username, email, password)` (password `None` → unusable).
pub async fn create_superuser(pool: &PgPool, username: &str, email: &str, password: Option<&str>) -> CliResult<i64> {
    let hash = match password {
        Some(p) => pl_authz::hashers::make_password(p),
        None => pl_authz::hashers::make_unusable_password(),
    };
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, last_login, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
         VALUES ($1, NULL, true, $2, '', '', $3, true, true, now()) RETURNING id",
    )
    .bind(&hash)
    .bind(username)
    .bind(normalize_email(email))
    .fetch_one(pool)
    .await?;
    Ok(i64::from(id))
}

fn read_line(prompt: &str) -> CliResult<Option<String>> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let n = std::io::stdin().lock().read_line(&mut s)?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(s.trim_end_matches(['\r', '\n']).to_string()))
}

const CSU_OPTS: &[args::Opt] = &[
    value(&["--username"], "username"),
    value(&["--email"], "email"),
    flag(&["--no-input", "--noinput"], "no_input"),
    value(&["--database"], "database"),
];

/// `createsuperuser`.
pub async fn createsuperuser(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, CSU_OPTS, 0)?;
    let c = Ctx::connect().await?;
    if a.flag("no_input") {
        let username = a
            .value("username")
            .map(str::to_string)
            .or_else(|| std::env::var("DJANGO_SUPERUSER_USERNAME").ok())
            .ok_or_else(|| CliError::Command("You must use --username with --noinput.".into()))?;
        validate_username(&username).map_err(|e| CliError::Command(format!("username: {e}")))?;
        if username_exists(&c.pool, &username).await? {
            return Err(CliError::Command("Error: That username is already taken.".into()));
        }
        let email = a.value("email").map(str::to_string).or_else(|| std::env::var("DJANGO_SUPERUSER_EMAIL").ok()).unwrap_or_default();
        let password = std::env::var("DJANGO_SUPERUSER_PASSWORD").ok();
        create_superuser(&c.pool, &username, &email, password.as_deref()).await?;
        println!("Superuser created successfully.");
        return Ok(());
    }
    // interactive
    let default_user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default().to_lowercase();
    let default_ok =
        !default_user.is_empty() && validate_username(&default_user).is_ok() && !username_exists(&c.pool, &default_user).await?;
    let username = loop {
        let prompt = if default_ok { format!("Username (leave blank to use '{default_user}'): ") } else { "Username: ".to_string() };
        let Some(mut u) = (match a.value("username") {
            Some(u) => Some(u.to_string()),
            None => read_line(&prompt)?,
        }) else {
            return Err(CliError::Command("Operation cancelled.".into()));
        };
        if u.is_empty() && default_ok {
            u = default_user.clone();
        }
        if u.is_empty() {
            eprintln!("Error: This field cannot be blank.");
            continue;
        }
        if let Err(e) = validate_username(&u) {
            eprintln!("Error: {e}");
            if a.value("username").is_some() {
                return Err(CliError::Command(e));
            }
            continue;
        }
        if username_exists(&c.pool, &u).await? {
            eprintln!("Error: That username is already taken.");
            if a.value("username").is_some() {
                return Err(CliError::Command("Error: That username is already taken.".into()));
            }
            continue;
        }
        break u;
    };
    let email = match a.value("email") {
        Some(e) => e.to_string(),
        None => read_line("Email address: ")?.unwrap_or_default(),
    };
    let attrs = UserAttrs { username: username.clone(), email: normalize_email(&email), ..Default::default() };
    let password = loop {
        let Some(p1) = read_line("Password: ")? else { return Err(CliError::Command("Operation cancelled.".into())) };
        let Some(p2) = read_line("Password (again): ")? else { return Err(CliError::Command("Operation cancelled.".into())) };
        if p1 != p2 {
            eprintln!("Error: Your passwords didn't match.");
            continue;
        }
        if p1.trim().is_empty() {
            eprintln!("Error: Blank passwords aren't allowed.");
            continue;
        }
        let errs = validate_password(&p1, Some(&attrs));
        if !errs.is_empty() {
            eprintln!("{}", errs.join("\n"));
            let answer = read_line("Bypass password validation and create user anyway? [y/N]: ")?.unwrap_or_default();
            if !answer.to_lowercase().starts_with('y') {
                continue;
            }
        }
        break p1;
    };
    create_superuser(&c.pool, &username, &email, Some(&password)).await?;
    println!("Superuser created successfully.");
    Ok(())
}

/// `manage_superuser` (container start): PAPERLESS_ADMIN_USER (admin), _MAIL (root@localhost), _PASSWORD.
pub async fn manage_superuser(argv: Vec<String>) -> CliResult {
    args::parse(&argv, &[], 0)?;
    let c = Ctx::connect().await?;
    let username = pl_core::config::var("PAPERLESS_ADMIN_USER").unwrap_or_else(|| "admin".into());
    let mail = pl_core::config::var("PAPERLESS_ADMIN_MAIL").unwrap_or_else(|| "root@localhost".into());
    let password = pl_core::config::var("PAPERLESS_ADMIN_PASSWORD");
    if username_exists(&c.pool, &username).await? {
        println!("Did not create superuser, a user {username} already exists");
        return Ok(());
    }
    let supers: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_user WHERE is_superuser").fetch_one(&c.pool).await?;
    if supers > 0 {
        println!("Did not create superuser, the DB already contains superusers");
        return Ok(());
    }
    match password {
        None => println!("Please check if PAPERRUST_ADMIN_PASSWORD has been set in the environment"),
        Some(p) => {
            create_superuser(&c.pool, &username, &mail, Some(&p)).await?;
            println!("Created superuser \"{username}\" with provided password.");
        }
    }
    Ok(())
}

const CPW_OPTS: &[args::Opt] = &[value(&["--database"], "database")];

/// `changepassword [username]` (3 attempts, validators without bypass).
pub async fn changepassword(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, CPW_OPTS, 1)?;
    let c = Ctx::connect().await?;
    let username = match a.positionals.first() {
        Some(u) => u.clone(),
        None => std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default(),
    };
    let row = sqlx::query("SELECT id, username, first_name, last_name, email FROM auth_user WHERE username = $1")
        .bind(&username)
        .fetch_optional(&c.pool)
        .await?;
    let Some(row) = row else {
        return Err(CliError::Command(format!("user '{username}' does not exist")));
    };
    let id: i32 = row.try_get(0)?;
    let attrs = UserAttrs { username: row.try_get(1)?, first_name: row.try_get(2)?, last_name: row.try_get(3)?, email: row.try_get(4)? };
    println!("Changing password for user '{}'", attrs.username);
    let max_tries = 3;
    let mut count = 0;
    let mut password = None;
    while count < max_tries {
        let Some(p1) = read_line("Password: ")? else { break };
        let Some(p2) = read_line("Password (again): ")? else { break };
        if p1 != p2 {
            println!("Passwords do not match. Please try again.");
            count += 1;
            continue;
        }
        let errs = validate_password(&p2, Some(&attrs));
        if !errs.is_empty() {
            eprintln!("{}", errs.join("\n"));
            count += 1;
            continue;
        }
        password = Some(p2);
        break;
    }
    let Some(p) = password else {
        return Err(CliError::Command(format!("Aborting password change for user '{}' after {count} attempts", attrs.username)));
    };
    set_password_and_revoke(&c.pool, id, &pl_authz::hashers::make_password(&p)).await?;
    println!("Password changed successfully for user '{}'", attrs.username);
    Ok(())
}

/// Store the new (encoded) password and revoke the user's device tokens in **one** transaction
/// (security review 2026-11-19 F13, like `pl_auth_api::db::set_password_and_revoke` for reset
/// and profile): the new password never becomes visible while old device tokens still work, and
/// a failed revocation leaves the old password in place. A missing `paperrust_device_token`
/// table (migration not applied) counts as "nothing to revoke"; that statement runs in a
/// savepoint so its error does not abort the outer transaction (`pl_authz::device::revoke_all_for_user_tx`).
async fn set_password_and_revoke(pool: &PgPool, user_id: i32, encoded: &str) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE auth_user SET password = $1 WHERE id = $2").bind(encoded).bind(user_id).execute(&mut *tx).await?;
    pl_authz::device::revoke_all_for_user_tx(&mut tx, i64::from(user_id)).await?;
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pool with one connection whose `search_path` is a fresh schema `schema` (created here),
    /// so the test tables never touch real data. Needs `PAPERRUST_CLI_USERS_TEST_DATABASE_URL`.
    async fn scratch_pool(schema: &str) -> Option<PgPool> {
        let url = std::env::var("PAPERRUST_CLI_USERS_TEST_DATABASE_URL").ok()?;
        let pool = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
        sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).execute(&pool).await.unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}")).execute(&pool).await.unwrap();
        sqlx::query(&format!("SET search_path TO {schema}")).execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE auth_user (id int4 PRIMARY KEY, password text NOT NULL)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO auth_user VALUES (7, 'old')").execute(&pool).await.unwrap();
        Some(pool)
    }

    async fn drop_schema(pool: &PgPool, schema: &str) {
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE")).execute(pool).await.unwrap();
    }

    async fn password(pool: &PgPool) -> String {
        sqlx::query_scalar("SELECT password FROM auth_user WHERE id = 7").fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    #[ignore = "needs PAPERRUST_CLI_USERS_TEST_DATABASE_URL (any PostgreSQL; works in its own schema)"]
    async fn password_change_revokes_device_tokens_atomically() {
        let schema = "paperrust_cli_users_test_a";
        let Some(pool) = scratch_pool(schema).await else { return };
        sqlx::query("CREATE TABLE paperrust_device_token (id serial PRIMARY KEY, user_id int4 NOT NULL, revoked_at timestamptz)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO paperrust_device_token (user_id) VALUES (7), (7), (8)").execute(&pool).await.unwrap();
        set_password_and_revoke(&pool, 7, "new").await.unwrap();
        assert_eq!(password(&pool).await, "new");
        let open: Vec<i32> =
            sqlx::query_scalar("SELECT user_id FROM paperrust_device_token WHERE revoked_at IS NULL").fetch_all(&pool).await.unwrap();
        assert_eq!(open, vec![8], "only the other user's token stays active");

        // A failing revocation (here: a trigger raising an error) keeps the old password.
        sqlx::query("CREATE FUNCTION boom() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'boom'; END $$")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER t BEFORE UPDATE ON paperrust_device_token FOR EACH ROW EXECUTE FUNCTION boom()")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO paperrust_device_token (user_id) VALUES (7)").execute(&pool).await.unwrap();
        assert!(set_password_and_revoke(&pool, 7, "newer").await.is_err());
        assert_eq!(password(&pool).await, "new", "password change rolled back with the failed revocation");
        drop_schema(&pool, schema).await;
    }

    #[tokio::test]
    #[ignore = "needs PAPERRUST_CLI_USERS_TEST_DATABASE_URL (any PostgreSQL; works in its own schema)"]
    async fn password_change_without_device_token_table() {
        let schema = "paperrust_cli_users_test_b";
        let Some(pool) = scratch_pool(schema).await else { return };
        // Only the scratch schema is searched, so `paperrust_device_token` is missing (42P01).
        set_password_and_revoke(&pool, 7, "new").await.unwrap();
        assert_eq!(password(&pool).await, "new");
        drop_schema(&pool, schema).await;
    }

    #[test]
    fn validators_like_django() {
        let u = UserAttrs { username: "alice".into(), email: "alice@example.com".into(), ..Default::default() };
        assert_eq!(
            validate_password("alice", Some(&u)),
            vec![
                "The password is too similar to the username.".to_string(),
                "This password is too short. It must contain at least 8 characters.".to_string(),
                "This password is too common.".to_string(),
            ]
        );
        assert_eq!(
            validate_password("12345678", None),
            vec!["This password is too common.".to_string(), "This password is entirely numeric.".to_string(),]
        );
        assert!(validate_password("correct horse battery staple 9", Some(&u)).is_empty());
        assert_eq!(validate_password("98127346501", None), vec!["This password is entirely numeric.".to_string()]);
    }

    #[test]
    fn helpers() {
        assert_eq!(normalize_email(" Foo@ExAmple.COM "), "Foo@example.com");
        assert_eq!(normalize_email("nomail"), "nomail");
        assert!(validate_username("a.b@c+d-e_f").is_ok());
        assert!(validate_username("a b").is_err());
        assert_eq!(split_nonword("a.b c"), vec!["a", "b", "c"]);
        assert!((quick_ratio(&['a', 'b'], &['b', 'a']) - 1.0).abs() < 1e-9);
    }
}
