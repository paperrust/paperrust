# Read-only: computes expected permission results with upstream paperless code on the reference DB.
import json, sys, os
import django
django.setup()
from django.contrib.auth.models import User, AnonymousUser
from documents.models import Tag, Correspondent, DocumentType, StoragePath, Document, SavedView
from paperless_mail.models import MailAccount, MailRule
from documents.permissions import permitted_object_ids, permitted_document_ids, get_permission_grantees, has_perms_owner_aware
from documents.serialisers import OwnedObjectSerializer
from guardian.core import ObjectPermissionChecker
out = {"users": {}, "permitted": {}, "grantees": {}, "shared": {}, "obj_perm": {}}
models = {"tag": Tag, "correspondent": Correspondent, "documenttype": DocumentType, "storagepath": StoragePath,
          "savedview": SavedView, "mailaccount": MailAccount, "mailrule": MailRule}
users = list(User.objects.order_by("id"))
for u in users:
    out["users"][u.username] = {"id": u.id, "groups": sorted(u.groups.values_list("id", flat=True)),
        "perms": sorted(u.get_all_permissions()) if u.is_active else [], "is_superuser": u.is_superuser, "is_active": u.is_active}
for who in users + [None]:
    name = who.username if who else "<anon>"
    d = {}
    for key, m in models.items():
        for action in ("view", "change"):
            d[f"{key}.{action}"] = sorted(permitted_object_ids(who if who else AnonymousUser(), m, f"{action}_{key}"))
    d["document.view"] = sorted(permitted_document_ids(who if who else AnonymousUser()))
    d["document.change"] = sorted(permitted_document_ids(who if who else AnonymousUser(), perm="change_document"))
    out["permitted"][name] = d
for key, m in list(models.items()) + [("document", Document)]:
    pks = list(m.objects.values_list("id", flat=True)) if key != "document" else list(Document.global_objects.values_list("id", flat=True))
    g = get_permission_grantees(m, pks)
    out["grantees"][key] = {str(k): v for k, v in g.items()}
    objs = list(m.objects.all()) if key != "document" else list(Document.global_objects.all())
    out["shared"][key] = sorted(OwnedObjectSerializer.get_shared_object_pks(objs)) if objs else []
    for u in users:
        c = ObjectPermissionChecker(u)
        for o in objs:
            for action in ("view", "change", "delete"):
                out["obj_perm"].setdefault(key, {}).setdefault(u.username, {}).setdefault(action, [])
                if c.has_perm(f"{action}_{key}", o):
                    out["obj_perm"][key][u.username][action].append(o.id)
json.dump(out, open(sys.argv[1], "w"), indent=1, sort_keys=True)
print("ok", len(users))
