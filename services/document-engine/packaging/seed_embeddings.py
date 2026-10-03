"""Pre-download the embedding model into the Tauri packaging resources.

Run by release CI (and local release builds) before `tauri build`:

    cd services/document-engine && uv run python packaging/seed_embeddings.py

The model lands in `apps/desktop/src-tauri/packaging/resources/models/
embeddings/` — the exact fastembed cache layout — which the bundle copies
into the app resources. On first run the desktop app seeds this directory
into `<data>/models/embeddings`, so semantic search works offline with no
download. Re-running is idempotent (fastembed skips files it has).
"""

from __future__ import annotations

import os
import pathlib
import stat

from fastembed import TextEmbedding

from researchai_document_engine.embeddings import _MODEL_NAME

# services/document-engine/packaging/ -> repo root
REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
DEST = REPO_ROOT / "apps" / "desktop" / "src-tauri" / "packaging" / "resources" / "models" / "embeddings"


def main() -> None:
    DEST.mkdir(parents=True, exist_ok=True)
    print(f"seeding {_MODEL_NAME} into {DEST}")
    model = TextEmbedding(model_name=_MODEL_NAME, cache_dir=str(DEST))
    # Touch the pipeline once so every runtime file is fetched now, not later.
    list(model.embed(["seed probe"]))
    _make_writable(DEST)
    print("embedding model seeded")


def _make_writable(root: pathlib.Path) -> None:
    """Give every fetched file owner-write permission.

    Hugging Face stores blobs as read-only (-r--r--r--). Tauri's build
    script copies resources with fs::copy, which *preserves* that mode —
    so the next build's truncate of the previous copy fails with
    EACCES ("Permission denied"). Normalising here keeps every rebuild
    green. Exec bits are preserved.
    """
    fixed = 0
    for dirpath, _dirnames, filenames in os.walk(root):
        for name in filenames:
            path = os.path.join(dirpath, name)
            mode = os.stat(path).st_mode
            if not mode & stat.S_IWUSR:
                os.chmod(path, mode | stat.S_IWUSR)
                fixed += 1
    if fixed:
        print(f"normalised {fixed} read-only file(s) to owner-writable")


if __name__ == "__main__":
    main()
