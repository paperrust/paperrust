"""Shared SQL state checks for the W2 write catalogues (cases-w2.json generators)."""
USERS = ("select u.id, u.username, u.email, u.first_name, u.last_name, u.is_staff, u.is_active, u.is_superuser, "
         "u.date_joined, u.last_login, case when u.password like '!%' then 'unusable' "
         "else split_part(u.password, '$', 1) || '$' || split_part(u.password, '$', 2) end as pw_kind, "
         "(select coalesce(array_agg(g.group_id order by g.group_id), '{}') from auth_user_groups g where g.user_id = u.id) as groups, "
         "(select coalesce(array_agg(p.codename order by p.codename), '{}') from auth_user_user_permissions up "
         "join auth_permission p on p.id = up.permission_id where up.user_id = u.id) as perms "
         "from auth_user u order by u.id")
GROUPS = ("select g.id, g.name, (select coalesce(array_agg(p.codename order by p.codename), '{}') "
          "from auth_group_permissions gp join auth_permission p on p.id = gp.permission_id where gp.group_id = g.id) as perms "
          "from auth_group g order by g.id")
TOKENS = "select user_id, length(key) as len, created from authtoken_token order by user_id"
UISETTINGS = "select id, user_id, settings from documents_uisettings order by id"
EMAIL = "select user_id, email, verified, \"primary\" from account_emailaddress order by id"
AUDIT = ("select l.id, l.action, ct.app_label || '.' || ct.model as model, l.object_pk, l.object_id, l.object_repr, "
         "l.changes, l.changes_text, l.actor_id, l.remote_addr, l.additional_data, l.timestamp, l.cid is not null as has_cid, l.actor_email, l.serialized_data "
         "from auditlog_logentry l join django_content_type ct on ct.id = l.content_type_id where l.id > 918 order by l.id")
GUARDIAN = ("select ct.model, o.object_pk, o.user_id as who, 'u' as kind, p.codename from guardian_userobjectpermission o "
            "join auth_permission p on p.id = o.permission_id join django_content_type ct on ct.id = o.content_type_id "
            "union all select ct.model, o.object_pk, o.group_id, 'g', p.codename from guardian_groupobjectpermission o "
            "join auth_permission p on p.id = o.permission_id join django_content_type ct on ct.id = o.content_type_id "
            "order by 1, 2, 4, 3, 5")


def c(id, route, steps, users=("admin",), versions=("10",), **kw):
    """One W2 write case (id gets the prefix w2.)."""
    d = {"id": f"w2.{id}", "route": route, "users": list(users), "api_versions": list(versions),
         "write": True, "steps": steps}
    d.update(kw)
    return d


def req(method, path, body=None, form=None, **kw):
    s = {"method": method, "path": path}
    if body is not None:
        s["json"] = body
    if form is not None:
        s["form"] = form
    s.update(kw)
    return s


def sql(q):
    return {"sql": q}
