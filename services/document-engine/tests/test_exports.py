"""Export endpoint tests (Phase 6, spec §32): DOCX and PDF rendering."""

from __future__ import annotations

from fastapi.testclient import TestClient

from researchai_document_engine.app import app

client = TestClient(app)


def _body() -> dict:
    return {
        "title": "Export title",
        "subtitle": "ResearchAI · test",
        "blocks": [
            {"kind": "paragraph", "text": "Body paragraph."},
            {"kind": "heading", "text": "Findings"},
            {"kind": "bullet", "text": "First finding"},
            {"kind": "bullet", "text": "Second finding"},
            {
                "kind": "table",
                "columns": ["Document", "Strength"],
                "rows": [["a.pdf", "direct"], ["b.pdf", "none"]],
            },
        ],
    }


def test_export_docx_renders_office_open_xml() -> None:
    res = client.post("/export/docx", json=_body())
    assert res.status_code == 200
    assert (
        res.headers["content-type"]
        == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    )
    data = res.content
    # DOCX is a ZIP container — magic bytes are a hard contract.
    assert data[:2] == b"PK"
    assert len(data) > 1000


def test_export_pdf_renders_pdf() -> None:
    res = client.post("/export/pdf", json=_body())
    assert res.status_code == 200
    assert res.headers["content-type"] == "application/pdf"
    data = res.content
    assert data[:5] == b"%PDF-"
    assert b"%%EOF" in data[-64:]


def test_export_rejects_block_without_text() -> None:
    body = {"title": "t", "blocks": [{"kind": "paragraph"}]}
    res = client.post("/export/docx", json=body)
    assert res.status_code in (400, 422)  # pydantic/validation error, not a crash


def test_export_empty_blocks_still_render() -> None:
    body = {"title": "Only a title"}
    for endpoint, magic in (("/export/docx", b"PK"), ("/export/pdf", b"%PDF-")):
        res = client.post(endpoint, json=body)
        assert res.status_code == 200
        assert res.content[: len(magic)] == magic
