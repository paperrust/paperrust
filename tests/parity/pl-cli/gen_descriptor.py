"""Generates crates/pl-cli/baseline/export-descriptor.json from Django _meta (upstream v3.4.2, unchanged).

For every manifest key of document_exporter.py the query (WHERE/ORDER BY like the ORM),
per model the serialized fields in serializer order (local_fields with serialize=True,
then local_many_to_many) and the m2m query of the related manager (incl. default manager filter
and Meta.ordering of the target model). Runs no write queries.

Invocation (environment from reference/env.sh, PAPERLESS_DBNAME on a fresh DB):
    python -X utf8 tests/parity/pl-cli/gen_descriptor.py > crates/pl-cli/baseline/export-descriptor.json
"""

import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "..", "upstream", "src"))
import django  # noqa: E402

django.setup()

from django.apps import apps  # noqa: E402
from django.db import connection  # noqa: E402
from django.db import models as dm  # noqa: E402
from django.db.models import GeneratedField  # noqa: E402
from django.db.models import NOT_PROVIDED  # noqa: E402

from documents.management.commands import document_exporter as exp  # noqa: E402
from documents.management.commands.mixins import CryptMixin  # noqa: E402
from documents.models import CustomFieldInstance, Document, DocumentBarcode, Note  # noqa: E402
from paperless import version  # noqa: E402


def pg_sql(qs):
    sql, params = qs.query.sql_with_params()
    out, n = [], 0
    i = 0
    while i < len(sql):
        if sql.startswith("%s", i):
            n += 1
            out.append(f"${n}")
            i += 2
        elif sql.startswith("%%", i):
            out.append("%")
            i += 2
        else:
            out.append(sql[i])
            i += 1
    return "".join(out), [p if isinstance(p, (int, str, bool, type(None))) else str(p) for p in params]


def default_of(f):
    if f.default is NOT_PROVIDED:
        return {"kind": "none"}
    d = f.default
    if callable(d):
        name = getattr(d, "__qualname__", repr(d))
        mod = getattr(d, "__module__", "")
        return {"kind": "callable", "name": f"{mod}.{name}"}
    try:
        json.dumps(d)
        return {"kind": "value", "value": d}
    except TypeError:
        return {"kind": "repr", "value": repr(d)}


def field_desc(f):
    kind = "field"
    if f.remote_field is not None:
        kind = "fk"
    return {
        "name": f.name,
        "attname": f.attname,
        "column": f.column,
        "kind": kind,
        "internal_type": f.get_internal_type(),
        "db_type": f.db_type(connection) if not isinstance(f, GeneratedField) else "generated",
        "serialize": bool(f.serialize),
        "primary_key": bool(f.primary_key),
        "generated": isinstance(f, GeneratedField),
        "auto_now": bool(getattr(f, "auto_now", False)),
        "auto_now_add": bool(getattr(f, "auto_now_add", False)),
        "null": bool(f.null),
        "default": default_of(f),
        "related_model": f.related_model._meta.label_lower if f.remote_field is not None else None,
        "on_delete": getattr(getattr(f.remote_field, "on_delete", None), "__name__", None) if f.remote_field is not None else None,
    }


def model_desc(model):
    meta = model._meta
    concrete = meta.concrete_model._meta
    fields = [field_desc(f) for f in concrete.local_fields]
    m2m = []
    for f in concrete.local_many_to_many:
        through = f.remote_field.through
        tm = through._meta
        src = f.m2m_field_name()
        dst = f.m2m_reverse_field_name()
        inst = model(pk=0)
        qs = getattr(inst, f.name).select_related(None).only("pk")
        sql, params = pg_sql(qs)
        m2m.append({
            "name": f.name,
            "serialize": bool(f.serialize),
            "auto_created": bool(tm.auto_created),
            "through_table": tm.db_table,
            "src_column": tm.get_field(src).column,
            "dst_column": tm.get_field(dst).column,
            "related_model": f.related_model._meta.label_lower,
            "related_sql": sql,
            "related_params": params,
        })
    return {
        "label": meta.label_lower,
        "object_name": meta.object_name,
        "table": meta.db_table,
        "pk_column": meta.pk.column,
        "pk_attname": meta.pk.attname,
        "pk_internal_type": meta.pk.get_internal_type(),
        "fields": fields,
        "m2m": m2m,
        "has_global_objects": hasattr(model, "global_objects"),
    }


def main():
    cmd = exp.Command()
    # The queries are local in dump(); we rebuild them identically (source: document_exporter.py dump()).
    from allauth.mfa.models import Authenticator
    from allauth.socialaccount.models import SocialAccount, SocialApp, SocialToken
    from auditlog.models import LogEntry
    from django.contrib.auth.models import Group, Permission, User
    from django.contrib.contenttypes.models import ContentType
    from guardian.models import GroupObjectPermission, UserObjectPermission

    from documents.models import (Correspondent, CustomField, DocumentType, SavedView,
                                  SavedViewFilterRule, ShareLink, ShareLinkBundle, StoragePath, Tag,
                                  UiSettings, Workflow, WorkflowAction, WorkflowActionEmail,
                                  WorkflowActionWebhook, WorkflowTrigger)
    from paperless.models import ApplicationConfiguration
    from paperless_mail.models import MailAccount, MailRule

    ex = ["consumer", "AnonymousUser"]
    keys = {
        "correspondents": Correspondent.objects.all(),
        "tags": Tag.objects.all(),
        "document_types": DocumentType.objects.all(),
        "storage_paths": StoragePath.objects.all(),
        "mail_accounts": MailAccount.objects.all(),
        "mail_rules": MailRule.objects.all(),
        "saved_views": SavedView.objects.all(),
        "saved_view_filter_rules": SavedViewFilterRule.objects.all(),
        "groups": Group.objects.all(),
        "users": User.objects.exclude(username__in=ex).all(),
        "ui_settings": UiSettings.objects.exclude(user__username__in=ex),
        "content_types": ContentType.objects.all(),
        "permissions": Permission.objects.all(),
        "user_object_permissions": UserObjectPermission.objects.exclude(user__username__in=ex),
        "group_object_permissions": GroupObjectPermission.objects.all(),
        "workflow_triggers": WorkflowTrigger.objects.all(),
        "workflow_actions": WorkflowAction.objects.all(),
        "workflow_email_actions": WorkflowActionEmail.objects.all(),
        "workflow_webhook_actions": WorkflowActionWebhook.objects.all(),
        "workflows": Workflow.objects.all(),
        "custom_fields": CustomField.objects.all(),
        "custom_field_instances": CustomFieldInstance.global_objects.all(),
        "document_barcodes": DocumentBarcode.objects.all(),
        "app_configs": ApplicationConfiguration.objects.all(),
        "notes": Note.global_objects.all(),
        "documents": Document.global_objects.order_by("id").all(),
        "share_links": ShareLink.global_objects.all(),
        "share_link_bundles": ShareLinkBundle.objects.order_by("id").all(),
        "social_accounts": SocialAccount.objects.exclude(user__username__in=ex),
        "social_apps": SocialApp.objects.all(),
        "social_tokens": SocialToken.objects.exclude(account__user__username__in=ex),
        "authenticators": Authenticator.objects.exclude(user__username__in=ex),
        "log_entries": LogEntry.objects.all(),
    }
    out_keys = []
    used_models = {}
    for key, qs in keys.items():
        model = qs.model
        cols = [f.attname for f in model._meta.concrete_model._meta.local_fields]
        sql, params = pg_sql(qs.values_list(*cols))
        pk_sql, pk_params = pg_sql(qs.values_list("pk", flat=True))
        out_keys.append({"key": key, "model": model._meta.label_lower, "sql": sql, "params": params,
                         "pk_sql": pk_sql, "pk_params": pk_params,
                         "audit_only": key == "log_entries"})
        used_models[model._meta.label_lower] = model

    split = {}
    for name, qs in (("notes", Note.global_objects.filter(document_id=0)),
                     ("custom_field_instances", CustomFieldInstance.global_objects.filter(document_id=0)),
                     ("document_barcodes", DocumentBarcode.objects.filter(document_id=0))):
        cols = [f.attname for f in qs.model._meta.local_fields]
        sql, params = pg_sql(qs.values_list(*cols))
        split[name] = {"model": qs.model._meta.label_lower, "sql": sql, "params": params}

    all_models = {}
    for model in apps.get_models():
        all_models[model._meta.label_lower] = model_desc(model)

    # Tables that point to ContentType/Permission via FK (recursively) (for the importer:
    # ContentType.objects.all().delete() / Permission.objects.all().delete() cascade in the ORM).
    refs = []
    for model in apps.get_models(include_auto_created=True):
        for f in model._meta.concrete_fields:
            if f.remote_field is not None and not isinstance(f, dm.ManyToManyField):
                refs.append({"table": model._meta.db_table, "column": f.column,
                             "pk_column": model._meta.pk.column,
                             "target": f.related_model._meta.label_lower,
                             "on_delete": getattr(f.remote_field.on_delete, "__name__", None)})

    print(json.dumps({
        "version": version.__full_version_str__,
        "excluded_usernames": ex,
        "crypt_fields": CryptMixin.CRYPT_FIELDS,
        "crypt_defaults": {"algorithm": cmd.kdf_algorithm, "iterations": cmd.key_iterations,
                            "key_size": cmd.key_size, "salt_size": cmd.salt_size},
        "keys": out_keys,
        "split": split,
        "documents_stream_sql": pg_sql(Document.global_objects.order_by("id").values_list("pk", flat=True))[0],
        "models": all_models,
        "fk_refs": refs,
    }, indent=1, ensure_ascii=False))


main()
