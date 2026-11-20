"""Harness entry for the two-database write run (tests/parity/release-run.sh).

Same as harness/run_parity.py, but {"sql": ...} steps run against the database of the side
being executed: PARITY_SQL_DB_A for Django (A), PARITY_SQL_DB_B for PaperRust (B). The harness
code itself stays unchanged; only PARITY_SQL_DB is set before each side's steps.
"""

import importlib.util
import os
import sys
from pathlib import Path

HARNESS = Path(__file__).resolve().parent / "harness"
sys.path.insert(0, str(HARNESS))

from plharness import runner  # noqa: E402

_orig_run_steps = runner.Runner._run_steps


def _run_steps(self, c, side):
    db = os.environ.get("PARITY_SQL_DB_" + side.upper())
    if db:
        os.environ["PARITY_SQL_DB"] = db
    return _orig_run_steps(self, c, side)


runner.Runner._run_steps = _run_steps

_spec = importlib.util.spec_from_file_location("run_parity", HARNESS / "run_parity.py")
_rp = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_rp)

if __name__ == "__main__":
    sys.exit(_rp.main())
