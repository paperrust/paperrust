// upstream: src/documents/permissions.py@v3.4.2 (permitted_object_ids /
// permitted_document_ids, annotate_authorizing_fields, has_perms_owner_aware),
// src/documents/versioning.py (root_document_ids), django-softdelete
// (`Document.objects` excludes deleted_at IS NOT NULL).
//
// LOCAL STAND-INS until pl-authz offers `permitted_document_ids` and
// `has_perms_owner_aware` (CORE-REQUEST in docs/contract/pl-search.md).
// Raw SQL (no sea-query form for UNION ALL + CASE subqueries is clearer);
// every value is bound, nothing is concatenated.

use chrono::NaiveDate;
use sqlx::PgPool;

/// Request user as far as this crate needs it.
#[derive(Debug, Clone)]
pub struct RequestUser {
    pub id: i64,
    pub is_superuser: bool,
    pub is_active: bool,
    /// current group memberships (`user.groups`)
    pub group_ids: Vec<i64>,
    /// model permissions as "app_label.codename" (user + group perms);
    /// ignored for active superusers.
    pub perms: std::collections::HashSet<String>,
}

impl RequestUser {
    /// Django `User.has_perm`: inactive -> False, active superuser -> True.
    pub fn has_perm(&self, perm: &str) -> bool {
        if !self.is_active {
            return false;
        }
        self.is_superuser || self.perms.contains(perm)
    }

    /// The index permission filter user (`None` for superusers, as upstream).
    pub fn search_user(&self) -> Option<crate::SearchUser> {
        if self.is_superuser {
            None
        } else {
            Some(crate::SearchUser {
                id: self.id,
                group_ids: self.group_ids.clone(),
            })
        }
    }
}

/// `SELECT ... WHERE id IN permitted_document_ids(user)` as SQL fragment over
/// alias `d`; parameter `$1` is the user id. Inactive users see nothing,
/// active superusers everything (caller checks).
const PERMITTED_DOC_CONDITION: &str = r#"
(
  (CASE WHEN d.root_document_id IS NULL THEN d.owner_id ELSE r.owner_id END) = $1
  OR (CASE WHEN d.root_document_id IS NULL THEN d.owner_id ELSE r.owner_id END) IS NULL
  OR CAST(COALESCE(d.root_document_id, d.id) AS varchar) IN (
    SELECT uop.object_pk FROM guardian_userobjectpermission uop
      JOIN auth_permission p ON p.id = uop.permission_id
      JOIN django_content_type ct ON ct.id = p.content_type_id
     WHERE uop.user_id = $1 AND p.codename = 'view_document'
       AND ct.app_label = 'documents' AND ct.model = 'document'
    UNION ALL
    SELECT gop.object_pk FROM guardian_groupobjectpermission gop
      JOIN auth_permission p ON p.id = gop.permission_id
      JOIN django_content_type ct ON ct.id = p.content_type_id
     WHERE gop.group_id IN (SELECT ug.group_id FROM auth_user_groups ug WHERE ug.user_id = $1)
       AND p.codename = 'view_document'
       AND ct.app_label = 'documents' AND ct.model = 'document'
  )
)"#;

/// L5 (grant index ready): same condition via the grant index
/// `paperrust_grant` (contract pl-authz "L5").
const PERMITTED_DOC_CONDITION_GRANTS: &str = r#"
(
  (CASE WHEN d.root_document_id IS NULL THEN d.owner_id ELSE r.owner_id END) = $1
  OR (CASE WHEN d.root_document_id IS NULL THEN d.owner_id ELSE r.owner_id END) IS NULL
  OR COALESCE(d.root_document_id, d.id) IN (
    SELECT g.object_id FROM paperrust_grant g
     WHERE g.perm_id = ANY(ARRAY(SELECT p.id FROM auth_permission p
                                   JOIN django_content_type ct ON ct.id = p.content_type_id
                                  WHERE p.codename = 'view_document'
                                    AND ct.app_label = 'documents' AND ct.model = 'document'))
       AND g.subject = ANY(ARRAY(SELECT -ug.group_id::int8 FROM auth_user_groups ug WHERE ug.user_id = $1)
                           || ($1)::int8)
  )
)"#;

/// Use the grant index? (only with pl-authz, i.e. feature `routes`)
fn use_grants() -> bool {
    #[cfg(feature = "routes")]
    {
        pl_authz::grants::use_grants()
    }
    #[cfg(not(feature = "routes"))]
    {
        false
    }
}

/// One row of `DocumentSerializer(fields=("id", "title", "created"))`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DocTitleRow {
    pub id: i64,
    pub title: String,
    pub created: NaiveDate,
}

fn visibility_sql(user: &RequestUser) -> (&'static str, bool) {
    if !user.is_active {
        ("FALSE", false)
    } else if user.is_superuser {
        ("TRUE", false)
    } else {
        (if use_grants() { PERMITTED_DOC_CONDITION_GRANTS } else { PERMITTED_DOC_CONDITION }, true)
    }
}

/// Django `__icontains` LIKE pattern (escapes `\`, `%`, `_`).
pub fn icontains_pattern(q: &str) -> String {
    let mut s = String::with_capacity(q.len() + 2);
    s.push('%');
    for c in q.chars() {
        if matches!(c, '\\' | '%' | '_') {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('%');
    s
}

/// GlobalSearchView db_only branch:
/// `Document.objects.filter(id__in=permitted_document_ids(user), title__icontains=q)[:limit]`
/// (Meta ordering `-created`).
pub async fn docs_title_icontains(
    db: &PgPool,
    user: &RequestUser,
    query: &str,
    limit: i64,
) -> Result<Vec<DocTitleRow>, sqlx::Error> {
    let (cond, binds_user) = visibility_sql(user);
    let sql = format!(
        "SELECT d.id::bigint AS id, d.title, d.created FROM documents_document d \
         LEFT JOIN documents_document r ON r.id = d.root_document_id \
         WHERE d.deleted_at IS NULL AND {cond} \
         AND UPPER(d.title::text) LIKE UPPER({p}) \
         ORDER BY d.created DESC LIMIT {l}",
        p = if binds_user { "$2" } else { "$1" },
        l = if binds_user { "$3" } else { "$2" },
    );
    let mut q = sqlx::query_as::<_, DocTitleRow>(&sql);
    if binds_user {
        q = q.bind(user.id);
    }
    q.bind(icontains_pattern(query)).bind(limit).fetch_all(db).await
}

/// GlobalSearchView index branch: `all_docs.in_bulk(ids)`.
pub async fn docs_in_bulk(
    db: &PgPool,
    user: &RequestUser,
    ids: &[i64],
) -> Result<Vec<DocTitleRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let (cond, binds_user) = visibility_sql(user);
    let sql = format!(
        "SELECT d.id::bigint AS id, d.title, d.created FROM documents_document d \
         LEFT JOIN documents_document r ON r.id = d.root_document_id \
         WHERE d.deleted_at IS NULL AND {cond} AND d.id = ANY({p})",
        p = if binds_user { "$2" } else { "$1" },
    );
    let mut q = sqlx::query_as::<_, DocTitleRow>(&sql);
    if binds_user {
        q = q.bind(user.id);
    }
    q.bind(ids.to_vec()).fetch_all(db).await
}

/// Result of `_get_more_like_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoreLikeLookup {
    /// root document id to search with
    Ok(i64),
    /// "Invalid more_like_id"
    Invalid,
    /// "Insufficient permissions."
    Forbidden,
}

/// `_get_more_like_id(query_params, user)`; `user` is `None` when the caller
/// passes no user (list path for superusers).
pub async fn more_like_lookup(
    db: &PgPool,
    doc_id: i64,
    user: Option<&RequestUser>,
) -> Result<MoreLikeLookup, sqlx::Error> {
    // Document.objects (not deleted) .get(pk=...), with root owner.
    let row: Option<(Option<i64>, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT d.root_document_id::bigint, \
                (CASE WHEN d.root_document_id IS NULL THEN d.owner_id ELSE r.owner_id END)::bigint, \
                COALESCE(d.root_document_id, d.id)::bigint \
           FROM documents_document d \
           LEFT JOIN documents_document r ON r.id = d.root_document_id \
          WHERE d.deleted_at IS NULL AND d.id = $1",
    )
    .bind(doc_id)
    .fetch_optional(db)
    .await?;
    let Some((root_id, owner, authorizing_id)) = row else {
        return Ok(MoreLikeLookup::Invalid);
    };
    let target = root_id.unwrap_or(doc_id);
    if let Some(user) = user {
        // has_perms_owner_aware: owner is None or owner == user or guardian has_perm
        let allowed = match owner {
            None => true,
            Some(o) if o == user.id => true,
            Some(_) => {
                if !user.is_active {
                    false
                } else if user.is_superuser {
                    true
                } else {
                    guardian_has_view_document(db, user.id, authorizing_id.unwrap_or(target)).await?
                }
            }
        };
        if !allowed {
            return Ok(MoreLikeLookup::Forbidden);
        }
    }
    Ok(MoreLikeLookup::Ok(target))
}

/// guardian `ObjectPermissionChecker(user).has_perm("view_document", obj)`
/// (user + group object permissions).
async fn guardian_has_view_document(
    db: &PgPool,
    user_id: i64,
    object_id: i64,
) -> Result<bool, sqlx::Error> {
    if use_grants() {
        let found: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM paperrust_grant g                JOIN auth_permission p ON p.id = g.perm_id                JOIN django_content_type ct ON ct.id = p.content_type_id               WHERE g.object_id = $2 AND g.ct = ct.id AND p.codename = 'view_document'                 AND ct.app_label = 'documents' AND ct.model = 'document'                 AND g.subject = ANY(ARRAY(SELECT -ug.group_id::int8 FROM auth_user_groups ug                                           WHERE ug.user_id = $1) || ($1)::int8)               LIMIT 1",
        )
        .bind(user_id)
        .bind(object_id)
        .fetch_optional(db)
        .await?;
        return Ok(found.is_some());
    }
    let object_pk = object_id.to_string();
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM guardian_userobjectpermission uop \
           JOIN auth_permission p ON p.id = uop.permission_id \
           JOIN django_content_type ct ON ct.id = p.content_type_id \
          WHERE uop.user_id = $1 AND uop.object_pk = $2 AND p.codename = 'view_document' \
            AND ct.app_label = 'documents' AND ct.model = 'document' \
            AND uop.content_type_id = ct.id \
         UNION ALL \
         SELECT 1 FROM guardian_groupobjectpermission gop \
           JOIN auth_permission p ON p.id = gop.permission_id \
           JOIN django_content_type ct ON ct.id = p.content_type_id \
           JOIN auth_user_groups ug ON ug.group_id = gop.group_id \
          WHERE ug.user_id = $1 AND gop.object_pk = $2 AND p.codename = 'view_document' \
            AND ct.app_label = 'documents' AND ct.model = 'document' \
            AND gop.content_type_id = ct.id \
         LIMIT 1",
    )
    .bind(user_id)
    .bind(&object_pk)
    .fetch_optional(db)
    .await?;
    Ok(found.is_some())
}

/// `user.groups.values_list("pk", flat=True)`.
pub async fn user_group_ids(db: &PgPool, user_id: i64) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT group_id::bigint FROM auth_user_groups WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(db)
        .await
}

/// L3: Is the search index for permissions and trash up to date with the database? `true` if the
/// table `paperrust_search_dirty` (migration `pl_indexer.0001_paperrust_search_dirty`) exists
/// and is empty. If it is missing or the query fails → `false` (exact path).
pub async fn search_index_clean(db: &PgPool) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT to_regclass('public.paperrust_search_dirty') IS NOT NULL \
            AND NOT EXISTS (SELECT 1 FROM paperrust_search_dirty)",
    )
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_escape() {
        assert_eq!(icontains_pattern("a_b%c\\"), "%a\\_b\\%c\\\\%");
    }

    #[test]
    fn has_perm_rules() {
        let mut u = RequestUser {
            id: 1,
            is_superuser: true,
            is_active: false,
            group_ids: vec![],
            perms: Default::default(),
        };
        assert!(!u.has_perm("documents.view_document"));
        u.is_active = true;
        assert!(u.has_perm("documents.view_document"));
        assert!(u.search_user().is_none());
    }
}
