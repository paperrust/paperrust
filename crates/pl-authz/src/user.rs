//! The Django `auth.User`, its groups and model permissions (ModelBackend semantics).
//!
//! upstream: django/contrib/auth/models.py@5.2 (User, PermissionsMixin.has_perm/has_perms),
//!           django/contrib/auth/backends.py@5.2 (ModelBackend.get_all_permissions,
//!           _get_user_permissions, _get_group_permissions, user_can_authenticate),
//!           allauth/mfa/adapter.py@65 (DefaultMFAAdapter.is_mfa_enabled).
//!
//! SQL: these loaders use hand-written SQL because sea-query has no form for
//! `ARRAY(subquery)`; all values are bound parameters, nothing is concatenated.
//!
//! O1 (every authenticated API request runs this once): superusers get a plain `ARRAY` of all
//! permissions (no `IN`-subquery per permission row), and there is no `DISTINCT` – the result
//! is collected into a `HashSet`, which removes duplicate `app_label.codename` pairs exactly as
//! the SQL `DISTINCT` did.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row as _};
use std::collections::HashSet;

/// A Django user with everything permission checks need, loaded in one query.
#[derive(Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    /// Encoded password hash (`auth_user.password`). Never serialize or log it.
    pub password: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub is_active: bool,
    pub is_staff: bool,
    pub is_superuser: bool,
    pub last_login: Option<DateTime<Utc>>,
    pub date_joined: DateTime<Utc>,
    /// `user.groups` ids, ascending.
    pub groups: Vec<i64>,
    /// ModelBackend permissions `"<app_label>.<codename>"` (user ∪ group permissions;
    /// for superusers all permissions, like `ModelBackend._get_permissions`).
    pub permissions: HashSet<String>,
}

impl std::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("is_active", &self.is_active)
            .field("is_staff", &self.is_staff)
            .field("is_superuser", &self.is_superuser)
            .field("groups", &self.groups)
            .finish_non_exhaustive()
    }
}

impl PartialEq for User {
    /// Django model equality: same pk.
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl User {
    /// `user.has_perm(perm)` without object (ModelBackend; guardian answers False without obj).
    ///
    /// Active superusers have every permission; inactive users have none.
    pub fn has_perm(&self, perm: &str) -> bool {
        if !self.is_active {
            return false;
        }
        self.is_superuser || self.permissions.contains(perm)
    }

    /// `user.has_perms(perms)`: all of them.
    pub fn has_perms(&self, perms: &[&str]) -> bool {
        perms.iter().all(|p| self.has_perm(p))
    }

    /// `user.get_full_name()`: `"%s %s" % (first_name, last_name)` stripped.
    pub fn full_name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name).trim().to_string()
    }

    /// `user.has_usable_password()`.
    pub fn has_usable_password(&self) -> bool {
        crate::hashers::is_password_usable(Some(&self.password))
    }

    /// `PermissionsMixin.get_all_permissions()` as a sorted list.
    pub fn all_permissions_sorted(&self) -> Vec<String> {
        if !self.is_active {
            return Vec::new();
        }
        let mut v: Vec<String> = self.permissions.iter().cloned().collect();
        v.sort();
        v
    }

    /// `get_session_auth_hash()`.
    pub fn session_auth_hash(&self, secret_key: &str) -> String {
        crate::signing::session_auth_hash(&self.password, secret_key)
    }
}

const USER_SELECT: &str = r#"
SELECT u.id::int8 AS id, u.password, u.last_login, u.is_superuser, u.username, u.first_name,
       u.last_name, u.email, u.is_staff, u.is_active, u.date_joined,
       ARRAY(SELECT ug.group_id::int8 FROM auth_user_groups ug
             WHERE ug.user_id = u.id ORDER BY ug.group_id) AS groups,
       CASE WHEN u.is_superuser
            THEN ARRAY(SELECT ct.app_label || '.' || p.codename
                       FROM auth_permission p JOIN django_content_type ct ON ct.id = p.content_type_id)
            ELSE ARRAY(SELECT ct.app_label || '.' || p.codename
                       FROM auth_permission p JOIN django_content_type ct ON ct.id = p.content_type_id
                       WHERE p.id IN (SELECT up.permission_id FROM auth_user_user_permissions up
                                      WHERE up.user_id = u.id
                                      UNION
                                      SELECT gp.permission_id FROM auth_group_permissions gp
                                      JOIN auth_user_groups ug2 ON ug2.group_id = gp.group_id
                                      WHERE ug2.user_id = u.id))
       END AS perms
FROM auth_user u"#;

fn row_to_user(r: &sqlx::postgres::PgRow) -> Result<User, sqlx::Error> {
    let perms: Vec<String> = r.try_get("perms")?;
    Ok(User {
        id: r.try_get("id")?,
        username: r.try_get("username")?,
        password: r.try_get("password")?,
        first_name: r.try_get("first_name")?,
        last_name: r.try_get("last_name")?,
        email: r.try_get("email")?,
        is_active: r.try_get("is_active")?,
        is_staff: r.try_get("is_staff")?,
        is_superuser: r.try_get("is_superuser")?,
        last_login: r.try_get("last_login")?,
        date_joined: r.try_get("date_joined")?,
        groups: r.try_get("groups")?,
        permissions: perms.into_iter().collect(),
    })
}

/// `User.objects.get(pk=id)` incl. groups and permissions.
/// Takes a pool or a held connection (`&mut *conn`): whoever already holds a connection should not fetch a
/// second one from the pool (BEFUNDE B2).
pub async fn load_user_by_id<'e, E: sqlx::PgExecutor<'e>>(db: E, id: i64) -> Result<Option<User>, sqlx::Error> {
    let sql = format!("{USER_SELECT} WHERE u.id = $1");
    let row = sqlx::query(&sql).bind(id).fetch_optional(db).await?;
    row.as_ref().map(row_to_user).transpose()
}

/// `User.objects.get_by_natural_key(username)` (exact match).
pub async fn load_user_by_username(db: &PgPool, username: &str) -> Result<Option<User>, sqlx::Error> {
    let sql = format!("{USER_SELECT} WHERE u.username = $1");
    let row = sqlx::query(&sql).bind(username).fetch_optional(db).await?;
    row.as_ref().map(row_to_user).transpose()
}

/// allauth `get_user_by_username(username)`: candidates by `username__iexact`, then those whose
/// stored username `.lower()` equals `username.lower()`; exactly one → that user, else `None`.
pub async fn load_user_by_username_iexact(
    db: &PgPool,
    username: &str,
) -> Result<Option<User>, sqlx::Error> {
    let sql = format!("{USER_SELECT} WHERE UPPER(u.username::text) = UPPER($1)");
    let rows = sqlx::query(&sql).bind(username).fetch_all(db).await?;
    let wanted = username.to_lowercase();
    let mut matches = Vec::new();
    for r in &rows {
        let u = row_to_user(r)?;
        if u.username.to_lowercase() == wanted {
            matches.push(u);
        }
    }
    if matches.len() == 1 { Ok(matches.pop()) } else { Ok(None) }
}

/// `Token.objects.select_related("user").get(key=key)` → the token's user.
pub async fn load_user_by_token(db: &PgPool, key: &str) -> Result<Option<User>, sqlx::Error> {
    let sql = format!("{USER_SELECT}{BY_TOKEN}");
    let row = sqlx::query(&sql).bind(key).fetch_optional(db).await?;
    row.as_ref().map(row_to_user).transpose()
}

const BY_TOKEN: &str = " WHERE u.id = (SELECT t.user_id FROM authtoken_token t WHERE t.key = $1)";

/// Pool warm-up (O1): run the per-request user queries (token and session/id form) once on
/// `conn` with keys that cannot exist. Prepares the statements on this connection and loads the
/// catalog entries of the auth tables into the backend, so the first real requests on a fresh
/// connection do not pay for it. Reads nothing, writes nothing.
pub async fn warm_up(conn: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    let sql = format!("{USER_SELECT}{BY_TOKEN}");
    sqlx::query(&sql).bind("").fetch_optional(&mut *conn).await?;
    let sql = format!("{USER_SELECT} WHERE u.id = $1");
    sqlx::query(&sql).bind(-1i64).fetch_optional(&mut *conn).await?;
    Ok(())
}

/// allauth `DefaultMFAAdapter.is_mfa_enabled(user)`: any `mfa_authenticator` row.
pub async fn is_mfa_enabled(db: &PgPool, user_id: i64) -> Result<bool, sqlx::Error> {
    let (b,): (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM mfa_authenticator WHERE user_id = $1)")
            .bind(user_id)
            .fetch_one(db)
            .await?;
    Ok(b)
}

/// Persist a new password hash (Django's `check_password` setter after `must_update`).
pub async fn update_password(db: &PgPool, user_id: i64, encoded: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE auth_user SET password = $1 WHERE id = $2")
        .bind(encoded)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// `RemoteUserBackend.authenticate`: `User.objects.get_or_create(username=...)`.
/// A created user has `password=''`, all flags false except `is_active`, `date_joined=now()`.
pub async fn get_or_create_user(db: &PgPool, username: &str) -> Result<Option<User>, sqlx::Error> {
    if let Some(u) = load_user_by_username(db, username).await? {
        return Ok(Some(u));
    }
    sqlx::query(
        "INSERT INTO auth_user (password, last_login, is_superuser, username, first_name, \
         last_name, email, is_staff, is_active, date_joined) \
         VALUES ('', NULL, false, $1, '', '', '', false, true, now()) \
         ON CONFLICT (username) DO NOTHING",
    )
    .bind(username)
    .execute(db)
    .await?;
    load_user_by_username(db, username).await
}

/// `User.objects.filter(is_staff=True).first()` (ordered by pk).
pub async fn first_staff_user(db: &PgPool) -> Result<Option<User>, sqlx::Error> {
    let sql = format!("{USER_SELECT} WHERE u.is_staff ORDER BY u.id LIMIT 1");
    let row = sqlx::query(&sql).fetch_optional(db).await?;
    row.as_ref().map(row_to_user).transpose()
}

#[cfg(test)]
pub(crate) fn test_user(id: i64) -> User {
    User {
        id,
        username: format!("user{id}"),
        password: "!".into(),
        first_name: String::new(),
        last_name: String::new(),
        email: String::new(),
        is_active: true,
        is_staff: false,
        is_superuser: false,
        last_login: None,
        date_joined: Utc::now(),
        groups: Vec::new(),
        permissions: HashSet::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perms_semantics() {
        let mut u = test_user(1);
        u.permissions.insert("documents.view_document".into());
        assert!(u.has_perm("documents.view_document"));
        assert!(!u.has_perm("documents.change_document"));
        assert!(u.has_perms(&[]));
        u.is_superuser = true;
        assert!(u.has_perm("documents.change_document"));
        u.is_active = false;
        assert!(!u.has_perm("documents.view_document"), "inactive superuser has nothing");
        assert!(u.all_permissions_sorted().is_empty());
    }

    #[test]
    fn debug_hides_password() {
        let mut u = test_user(1);
        u.password = "pbkdf2_sha256$secret".into();
        assert!(!format!("{u:?}").contains("secret"));
    }
}
