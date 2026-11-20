"""Settings for upstream paperless-ngx v2.20.15 with the reference venv (test fixture only).

The reference venv is built for v3.4.2. Some libraries that v2.20.15 imports at start
(but does not need for `migrate`) are missing there. Every module that no other finder finds
is replaced here by an empty module (list: `v220_settings.STUBBED`).
The migrations and models themselves remain unchanged upstream v2.20.15.
"""

import importlib.abc
import importlib.machinery
import os
import sys
from unittest import mock

# Only modules that nobody else finds (finder is at the end of sys.meta_path), and never the
# paperless apps themselves. Reported at start so that it stays visible what was replaced.
_NEVER = {"documents", "paperless", "paperless_mail", "paperless_tesseract", "paperless_text", "paperless_tika", "django"}
STUBBED = set()


_SRC = os.path.abspath(os.environ["V220_SRC_DIR"])


def _from_upstream():
    """Only replace imports directly from the upstream code (libraries try optional modules)."""
    f = sys._getframe(2)
    while f is not None:
        fn = f.f_code.co_filename
        if not (fn.startswith("<frozen") or "importlib" in fn):
            return os.path.abspath(fn).startswith(_SRC)
        f = f.f_back
    return False


class _StubModule(mock.MagicMock):
    __path__ = []  # treat as package so that submodules are importable


class _Finder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    def find_spec(self, name, path=None, target=None):
        root = name.split(".")[0]
        if root not in _NEVER and (root in STUBBED or ("." not in name and _from_upstream())):
            STUBBED.add(root)
            return importlib.machinery.ModuleSpec(name, self, is_package=True)
        return None

    def create_module(self, spec):
        m = _StubModule(name=spec.name)
        m.__name__ = spec.name
        m.__spec__ = spec
        m.__loader__ = self
        return m

    def exec_module(self, module):
        pass


sys.meta_path.append(_Finder())

from paperless.settings import *  # noqa: E402,F403
