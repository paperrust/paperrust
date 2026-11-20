"""Settings overlay of the W3 parity instance: reference overlay + compiled paperless catalogs.

The reference has no .mo files (upstream ships only .po); the Docker image compiles them.
w3_msgfmt.py puts them under W3_WORK/locale so that paperless's own texts are translated as in
production. Otherwise unchanged.
"""

import os
from pathlib import Path

from reference_settings import *  # noqa: F403

LOCALE_PATHS = [Path(os.environ["W3_WORK"]) / "locale"]
