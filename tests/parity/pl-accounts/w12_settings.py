"""Settings overlay of the W12 parity instance (social/OIDC): like w3_settings (compiled
paperless catalogs under W12_WORK/locale), otherwise unchanged."""

import os
from pathlib import Path

from reference_settings import *  # noqa: F403

LOCALE_PATHS = [Path(os.environ["W12_WORK"]) / "locale"]
