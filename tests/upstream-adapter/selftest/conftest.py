"""Self-test setup: toy Django project on a SQLite file, served by a wsgiref thread on
127.0.0.1 (separate DB connection = stands in for the live PaperRust server).

    cd tests/upstream-adapter/selftest
    ../../.venv/Scripts/python.exe -m pytest -p paperless_http_adapter -q   (PYTHONPATH=..;.)
"""

from __future__ import annotations

import os
import sys
import tempfile
import threading
from pathlib import Path
from wsgiref.simple_server import WSGIRequestHandler, make_server

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parent)]

_tmp = tempfile.mkdtemp(prefix="adapter-selftest-")
DB = os.path.join(_tmp, "db.sqlite3")


class _Quiet(WSGIRequestHandler):
    def log_message(self, *a):
        pass


def pytest_configure(config):
    import django
    from django.conf import settings

    import toyapp

    settings.configure(**toyapp.settings_dict(DB))
    django.setup()
    from django.core.management import call_command

    call_command("migrate", run_syncdb=True, verbosity=0)
    from django.core.wsgi import get_wsgi_application

    srv = make_server("127.0.0.1", 0, get_wsgi_application(), handler_class=_Quiet)
    t = threading.Thread(target=srv.serve_forever, name="adapter-live", daemon=True)
    t.start()
    import paperless_http_adapter as a

    a._TARGET = f"http://127.0.0.1:{srv.server_port}"
