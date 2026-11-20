"""Configuration of the parity harness.

All settings come from environment variables (or CLI flags that override them):

    PARITY_A_URL        Django reference           default http://127.0.0.1:8000
    PARITY_B_URL        PaperRust               default http://127.0.0.1:8080
    PARITY_HOST_HEADER  Host header sent to BOTH targets so that absolute URLs
                        (pagination next/previous, Location) are comparable.
                        default: host:port of A
    PARITY_USERS_FILE   JSON user catalogue (see users.py)
    PARITY_RESET_CMD    command restoring the reference snapshot
                        default: bash reference/reset.sh (only if the file exists)
    PARITY_CUTOFF       ISO timestamp; datetimes >= cutoff are volatile. default: run start
    PARITY_TIMEOUT      request timeout in seconds, default 60
    PARITY_SENSITIVE_PATHS  extra ';'-separated filesystem paths that must never leak
"""

from __future__ import annotations

import datetime as _dt
import os
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import urlsplit

HARNESS_DIR = Path(__file__).resolve().parent.parent  # tests/parity/harness
PARITY_DIR = HARNESS_DIR.parent  # tests/parity
REPO_DIR = PARITY_DIR.parent.parent  # repo root
REFERENCE_DIR = REPO_DIR / "reference"
RESULTS_DIR = REPO_DIR / "results" / "parity"


@dataclass
class Config:
    a_url: str = "http://127.0.0.1:8000"
    b_url: str = "http://127.0.0.1:8080"
    host_header: str | None = None
    users_file: str | None = None
    reset_cmd: list[str] | None = None
    cutoff: _dt.datetime = field(default_factory=lambda: _dt.datetime.now(_dt.timezone.utc))
    timeout: float = 60.0
    sensitive_paths: list[str] = field(default_factory=list)
    results_dir: Path = RESULTS_DIR
    parity_dir: Path = PARITY_DIR

    @property
    def effective_host_header(self) -> str:
        if self.host_header:
            return self.host_header
        return urlsplit(self.a_url).netloc


def default_reset_cmd() -> list[str] | None:
    env = os.environ.get("PARITY_RESET_CMD")
    if env:
        # simple split on whitespace; paths with spaces must be wrapped in a script
        return env.split()
    script = REFERENCE_DIR / "reset.sh"
    if script.exists():
        return ["bash", str(script)]
    return None


def parse_cutoff(value: str | None) -> _dt.datetime:
    if not value:
        return _dt.datetime.now(_dt.timezone.utc)
    dt = _dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=_dt.timezone.utc)
    return dt


def from_env() -> Config:
    cfg = Config(
        a_url=os.environ.get("PARITY_A_URL", "http://127.0.0.1:8000").rstrip("/"),
        b_url=os.environ.get("PARITY_B_URL", "http://127.0.0.1:8080").rstrip("/"),
        host_header=os.environ.get("PARITY_HOST_HEADER") or None,
        users_file=os.environ.get("PARITY_USERS_FILE") or None,
        reset_cmd=default_reset_cmd(),
        cutoff=parse_cutoff(os.environ.get("PARITY_CUTOFF")),
        timeout=float(os.environ.get("PARITY_TIMEOUT", "60")),
    )
    extra = os.environ.get("PARITY_SENSITIVE_PATHS", "")
    cfg.sensitive_paths = [p for p in extra.split(";") if p.strip()]
    # The repository and reference data directories must never show up in a response.
    cfg.sensitive_paths += [str(REPO_DIR), str(REFERENCE_DIR)]
    for var in ("PAPERLESS_MEDIA_ROOT", "PAPERLESS_DATA_DIR", "PAPERLESS_CONSUMPTION_DIR"):
        if os.environ.get(var):
            cfg.sensitive_paths.append(os.environ[var])
    return cfg
