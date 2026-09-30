"""Engine tests: health, capabilities and the parse contract."""

from __future__ import annotations

from fastapi.testclient import TestClient

from researchai_document_engine.app import app

client = TestClient(app)


def test_health() -> None:
    res = client.get("/health")
    assert res.status_code == 200
    body = res.json()
    assert body["status"] == "ok"
    assert body["service"] == "researchai-document-engine"
    assert "version" in body


def test_capabilities_lists_planned_formats() -> None:
    res = client.get("/capabilities")
    assert res.status_code == 200
    body = res.json()
    assert "pdf" in body["planned"]
    assert "docx" in body["planned"]


def test_parse_unsupported_format_reports_honestly() -> None:
    res = client.post(
        "/parse",
        json={"document_id": "doc-1", "managed_path": "/managed/doc-1/scan.xyz"},
    )
    assert res.status_code == 200
    body = res.json()
    assert body["ok"] is False
    assert "not supported" in body["error"]
