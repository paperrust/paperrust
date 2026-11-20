"""Django settings for the phase 6 measurement (own Django on 127.0.0.1:8091 against pl_pgvalidate).

= reference_settings + SQL logger: every query as one line `DJSQL\t<sec>\t<sql>` to
$PGV_DJ_SQL_LOG (only if set). Without DEBUG: `force_debug_cursor` is set when the connection is
established, so that Django reports the queries (with substituted parameters) via django.db.backends.
Upstream code stays unchanged.
"""
import logging
import os

from reference_settings import *  # noqa: F403
import reference_settings as _ref  # noqa: E402

for _k in dir(_ref):
    if _k.startswith("_") and not _k.startswith("__") and _k.isupper():
        globals()[_k] = getattr(_ref, _k)

_SQL_LOG = os.environ.get("PGV_DJ_SQL_LOG")


class DjSqlFormatter(logging.Formatter):
    def format(self, record):
        sql = str(getattr(record, "sql", record.getMessage()))
        dur = getattr(record, "duration", 0.0) or 0.0
        return "DJSQL\t%.6f\t%s" % (dur, sql.replace("\r", " ").replace("\n", "\\n"))


if _SQL_LOG:
    LOGGING["formatters"]["djsql"] = {"()": "pgv_settings.DjSqlFormatter"}  # noqa: F405
    LOGGING["handlers"]["djsql"] = {  # noqa: F405
        "class": "logging.FileHandler",
        "filename": _SQL_LOG,
        "formatter": "djsql",
        "encoding": "utf-8",
    }
    LOGGING["loggers"]["django.db.backends"] = {  # noqa: F405
        "handlers": ["djsql"],
        "level": "DEBUG",
        "propagate": False,
    }

    from django.db.backends.signals import connection_created  # noqa: E402

    def _force_debug(sender, connection, **kwargs):
        connection.force_debug_cursor = True

    connection_created.connect(_force_debug, weak=False)
