"""Settings overlay only for the pl-mail-worker parity run (upstream code unchanged).

Builds on reference_settings: Celery eager (no broker), cache/channels in memory (the
fetch lock `paperless_mail_fetch_lock` does not end up in the shared Garnet). DB name comes from
PAPERLESS_DBNAME (own copy pl_w6_test_up).
"""

from reference_settings import *  # noqa: F403

CELERY_TASK_ALWAYS_EAGER = True
CELERY_TASK_EAGER_PROPAGATES = False
CELERY_BROKER_URL = "memory://"
CELERY_RESULT_BACKEND = "cache+memory://"
CHANNEL_LAYERS = {"default": {"BACKEND": "channels.layers.InMemoryChannelLayer"}}
CACHES = {
    "default": {"BACKEND": "django.core.cache.backends.locmem.LocMemCache"},
    "read-cache": {"BACKEND": "django.core.cache.backends.locmem.LocMemCache"},
}
