"""Settings overlay of the reference environment (paperless-rs).

Only `from paperless.settings import *` plus justified overrides (see reference/README.md,
section "Abweichungen zu Produktion"). Upstream code stays unchanged.
"""

from pathlib import Path

from paperless.settings import *  # noqa: F403

# `import *` skips names with a leading "_"; upstream however reads e.g.
# settings._CHANNELS_REDIS_URL (SystemStatusView). Hence copy private UPPER-CASE names 1:1.
import paperless.settings as _upstream_settings  # noqa: E402

for _k in dir(_upstream_settings):
    if _k.startswith("_") and not _k.startswith("__") and _k.isupper():
        globals()[_k] = getattr(_upstream_settings, _k)

_REF_DIR = Path(__file__).resolve().parent.parent

# 1) Frontend: upstream expects the built Angular frontend under
#    src/documents/static/frontend (app static folder). We build it to reference/frontend
#    so as not to modify upstream/, and mount it under the same prefix "frontend/".
#    Result after `collectstatic` identical to the Docker installation.
_FRONTEND = _REF_DIR / "frontend"
if _FRONTEND.is_dir():
    STATICFILES_DIRS = [("frontend", str(_FRONTEND))]

# 2) Windows platform shim (no upstream code changed): os.statvfs is missing on Windows,
#    documents/views.py::SystemStatusView calls it for media_stats (-> otherwise always HTTP 500).
#    Replica via shutil.disk_usage with f_frsize=1 so that total/available are correct in bytes.
import os as _os  # noqa: E402

if not hasattr(_os, "statvfs"):
    import shutil as _shutil
    from types import SimpleNamespace as _NS

    def _statvfs(path):
        u = _shutil.disk_usage(path)
        return _NS(f_frsize=1, f_bsize=1, f_blocks=u.total, f_bfree=u.free, f_bavail=u.free)

    _os.statvfs = _statvfs
