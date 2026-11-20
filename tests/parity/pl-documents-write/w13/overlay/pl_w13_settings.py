"""Settings overlay only for the W13 parity run (PDF methods of bulk_edit; upstream unchanged).

Builds on reference_settings. Celery tasks are **not executed**: broker in memory
(kombu memory, no worker reads), result backend in memory. Every published message
(signal ``before_task_publish``, i.e. also for group/chord heads and ``link``) is written as a
JSON line to ``W13_TASK_LOG``: name, args/kwargs (``ConsumableDocument`` and
``DocumentMetadataOverrides`` as fields), header ``trigger_source``/``group``, callbacks,
errbacks and chord body. ``documents_paperlesstask`` is created like upstream in its own
``before_task_publish_handler``.
"""

import dataclasses
import datetime
import json
import os
from pathlib import Path

from reference_settings import *  # noqa: F403

CELERY_BROKER_URL = "memory://"
CELERY_RESULT_BACKEND = "cache+memory://"
CHANNEL_LAYERS = {"default": {"BACKEND": "channels.layers.InMemoryChannelLayer"}}
CACHES = {
    "default": {"BACKEND": "django.core.cache.backends.locmem.LocMemCache"},
    "read-cache": {"BACKEND": "django.core.cache.backends.locmem.LocMemCache"},
}


def _plain(v):
    if dataclasses.is_dataclass(v) and not isinstance(v, type):
        return {f.name: _plain(getattr(v, f.name)) for f in dataclasses.fields(v)}
    if isinstance(v, Path):
        return str(v)
    if isinstance(v, (datetime.datetime, datetime.date)):
        return v.isoformat()
    if isinstance(v, dict):
        return {str(k): _plain(x) for k, x in v.items()}
    if isinstance(v, (list, tuple, set)):
        return [_plain(x) for x in v]
    if isinstance(v, (str, int, float, bool)) or v is None:
        return v
    return repr(v)


def _sig(s):
    if s is None:
        return None
    d = dict(s)
    opts = d.get("options") or {}
    return {
        "task": d.get("task"),
        "args": _plain(d.get("args") or []),
        "kwargs": _plain(d.get("kwargs") or {}),
        "immutable": bool(d.get("immutable")),
        "link_error": [_sig(x) for x in (opts.get("link_error") or [])],
        "trigger_source": (opts.get("headers") or {}).get("trigger_source"),
    }


def _record(sender=None, body=None, headers=None, **kwargs):
    path = os.environ.get("W13_TASK_LOG")
    if not path:
        return
    headers = headers or {}
    try:
        args, kw, embed = body
    except Exception:  # pragma: no cover
        args, kw, embed = [], {}, {}
    embed = embed or {}
    entry = {
        "task": sender,
        "args": _plain(args),
        "kwargs": _plain(kw),
        "trigger_source": headers.get("trigger_source"),
        "group": headers.get("group"),
        "callbacks": [_sig(x) for x in (embed.get("callbacks") or [])],
        "errbacks": [_sig(x) for x in (embed.get("errbacks") or [])],
        "chord": _sig(embed.get("chord")),
    }
    with open(path, "a", encoding="utf-8") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")


from celery.signals import before_task_publish  # noqa: E402

before_task_publish.connect(_record, weak=False)
