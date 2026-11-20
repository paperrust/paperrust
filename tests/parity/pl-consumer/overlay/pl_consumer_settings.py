"""Settings overlay only for the pl-consumer parity run (upstream code unchanged).

Builds on reference_settings and keeps the run completely local: Celery eager (no broker),
channels/cache in memory (nothing ends up in the shared Garnet of the reference).
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
