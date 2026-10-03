"""Local embedding provider (spec §4.6, §16).

Primary engine: fastembed (ONNX, quantised, CPU-only, ~90 MB model) with the
compact BGE-small model — well suited to 8 GB machines (LIGHT profile).

Fallback engine: deterministic feature-hashing embeddings. These are *not*
semantically meaningful; they exist so retrieval, storage and the UI can be
developed and tested on machines where the model has not been downloaded yet.
The Rust layer treats both identically (fixed 384 dimensions); the response
reports which engine produced the vectors so the UI can stay honest.

Design rules:
- The embedding model is replaceable; dimension is fixed at 384 for V1 and
  stored per-index on the Rust side (migration 3 re-indexes on change).
- No network access at runtime: model weights download once on first use,
  which is an explicit local-cache operation, never a per-query call.
"""

from __future__ import annotations

import hashlib
import logging
import math
import os
import re
from typing import Literal

from pydantic import BaseModel, Field

logger = logging.getLogger(__name__)

DIMENSIONS = 384
_MODEL_NAME = "BAAI/bge-small-en-v1.5"

EmbeddingEngine = Literal["fastembed", "hashing-fallback"]

_token_re = re.compile(r"[a-z0-9]+")


class EmbeddingRequest(BaseModel):
    texts: list[str] = Field(min_length=1, max_length=256)


class SingleEmbedding(BaseModel):
    index: int
    vector: list[float]


class EmbeddingResponse(BaseModel):
    engine: EmbeddingEngine
    dimensions: int
    embeddings: list[SingleEmbedding]


class EmbeddingStatusResponse(BaseModel):
    engine: EmbeddingEngine
    dimensions: int
    model: str | None


class _FastembedBackend:
    """Lazy-loaded fastembed session; None until first successful init."""

    def __init__(self) -> None:
        self._model = None
        self._failed = False

    def available(self) -> bool:
        if self._failed:
            return False
        if self._model is not None:
            return True
        try:
            from fastembed import TextEmbedding

            self._model = TextEmbedding(model_name=_MODEL_NAME, cache_dir=_cache_dir())
            logger.info("fastembed model loaded: %s", _MODEL_NAME)
            return True
        except Exception as exc:  # noqa: BLE001 — fallback must be resilient
            logger.warning("fastembed unavailable, using hashing fallback: %s", exc)
            self._failed = True
            return False

    def embed(self, texts: list[str]) -> list[list[float]]:
        assert self._model is not None
        return [list(map(float, v)) for v in self._model.embed(texts)]


def _cache_dir() -> str | None:
    """Persistent model cache under the app's data directory.

    `RESEARCHAI_MODELS_DIR` is set by the desktop supervisor (and by the
    packaged sidecar), so downloads land in the user-owned data folder
    once and survive reboots/upgrades. When unset (dev, tests), return
    None and let fastembed use its default — which is `$TMPDIR`: macOS
    periodically wipes it, silently forcing a re-download and dropping
    search to the hashing fallback while offline. Pre-seeded installs
    copy the bundled model into this cache on first run (no network).
    """
    models_dir = os.environ.get("RESEARCHAI_MODELS_DIR")
    if not models_dir:
        return None
    return os.path.join(models_dir, "embeddings")


_backend = _FastembedBackend()


def _hash_embedding(text: str) -> list[float]:
    """Deterministic feature-hashing embedding (fallback engine).

    Tokens are hashed into D buckets with a signed second hash to reduce
    bias; the result is L2-normalised. Deterministic across processes so
    stored vectors stay comparable within an index lifetime.
    """
    vec = [0.0] * DIMENSIONS
    for token in _token_re.findall(text.lower())[:4096]:
        h = int.from_bytes(hashlib.blake2b(token.encode(), digest_size=8).digest(), "big")
        bucket = h % DIMENSIONS
        sign = 1.0 if (h >> 63) & 1 else -1.0
        vec[bucket] += sign
    norm = math.sqrt(sum(v * v for v in vec)) or 1.0
    return [v / norm for v in vec]


def status() -> EmbeddingStatusResponse:
    engine: EmbeddingEngine = "fastembed" if _backend.available() else "hashing-fallback"
    return EmbeddingStatusResponse(
        engine=engine,
        dimensions=DIMENSIONS,
        model=_MODEL_NAME if engine == "fastembed" else None,
    )


def embed_texts(texts: list[str]) -> EmbeddingResponse:
    if not texts:
        raise ValueError("texts must not be empty")

    if _backend.available():
        vectors = _backend.embed(texts)
        engine: EmbeddingEngine = "fastembed"
    else:
        vectors = [_hash_embedding(t) for t in texts]
        engine = "hashing-fallback"

    # Defensive: enforce dimension contract.
    vectors = [v[:DIMENSIONS] for v in vectors]

    return EmbeddingResponse(
        engine=engine,
        dimensions=DIMENSIONS,
        embeddings=[SingleEmbedding(index=i, vector=v) for i, v in enumerate(vectors)],
    )
