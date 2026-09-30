"""Engine configuration.

Local-only by design: the service binds to loopback and never performs
outbound network calls. Data lives under the managed workspace directory.
"""

from __future__ import annotations

import os
from pathlib import Path

HOST = os.environ.get("RESEARCHAI_ENGINE_HOST", "127.0.0.1")
PORT = int(os.environ.get("RESEARCHAI_ENGINE_PORT", "8737"))

# generous single-request ceiling for large PDFs (spec §43: bounded work)
MAX_UPLOAD_BYTES = 512 * 1024 * 1024

# Workspace root: override for dev/testing, otherwise platform app-data style.
WORKSPACE_ROOT = Path(
    os.environ.get(
        "RESEARCHAI_DATA_DIR",
        Path.home() / "Library" / "Application Support" / "ResearchAI",
    )
)


def derived_dir() -> Path:
    path = WORKSPACE_ROOT / "derived"
    path.mkdir(parents=True, exist_ok=True)
    return path
