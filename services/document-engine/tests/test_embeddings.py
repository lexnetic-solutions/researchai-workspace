"""Embedding engine tests: contract, determinism, API surface."""

from __future__ import annotations

import math

from fastapi.testclient import TestClient

from researchai_document_engine.app import app
from researchai_document_engine.embeddings import (
    DIMENSIONS,
    _hash_embedding,
    embed_texts,
    status,
)

client = TestClient(app)


def test_status_reports_engine_and_dimensions() -> None:
    s = status()
    assert s.engine in ("fastembed", "hashing-fallback")
    assert s.dimensions == DIMENSIONS


def test_hash_embedding_is_deterministic_and_normalised() -> None:
    a = _hash_embedding("semantic chunking preserves source locations")
    b = _hash_embedding("semantic chunking preserves source locations")
    assert a == b
    assert len(a) == DIMENSIONS
    norm = math.sqrt(sum(v * v for v in a))
    assert abs(norm - 1.0) < 1e-6


def test_embed_texts_returns_indexed_vectors() -> None:
    res = embed_texts(["first passage", "second passage"])
    assert res.engine in ("fastembed", "hashing-fallback")
    assert res.dimensions == DIMENSIONS
    assert [e.index for e in res.embeddings] == [0, 1]
    assert all(len(e.vector) == DIMENSIONS for e in res.embeddings)


def test_embeddings_endpoint_contract() -> None:
    res = client.post("/embeddings", json={"texts": ["hello world"]})
    assert res.status_code == 200
    body = res.json()
    assert body["dimensions"] == DIMENSIONS
    assert len(body["embeddings"]) == 1
    assert len(body["embeddings"][0]["vector"]) == DIMENSIONS


def test_embeddings_status_endpoint() -> None:
    res = client.get("/embeddings/status")
    assert res.status_code == 200
    body = res.json()
    assert body["engine"] in ("fastembed", "hashing-fallback")
