"""Parser registry behaviour (Phase 1: real parsers registered)."""

from __future__ import annotations

from researchai_document_engine import parsers


def test_real_parsers_are_registered() -> None:
    registered = set(parsers.supported_now())
    assert {"pdf", "docx", "txt", "md", "html", "htm"} <= registered
    # Stdlib ZIP+XML containers (office_extractors) must be registered too —
    # the desktop allow-list already advertises them, so a gap here means a
    # file imports cleanly and then fails at parse time.
    assert {"pptx", "xlsx", "epub"} <= registered


def test_planned_still_lists_phase1_targets() -> None:
    planned = set(parsers.planned())
    assert {"pdf", "docx", "pptx", "xlsx", "md", "txt", "html", "epub"} <= planned


def test_get_parser_dispatches_case_insensitively() -> None:
    assert parsers.get_parser("pdf") is not None
    assert parsers.get_parser("PDF") is not None
    assert parsers.get_parser("pptx") is not None
    assert parsers.get_parser("XLSX") is not None
    assert parsers.get_parser("xyz") is None


def test_register_and_dispatch() -> None:
    @parsers.register("txt")
    def fake_parser(path: str) -> parsers.ParseResponse:
        return parsers.ParseResponse(document_id="x", ok=True, title=path)

    try:
        assert parsers.get_parser("TXT") is fake_parser
        assert parsers.get_parser("txt") is fake_parser
    finally:
        # Restore the real registered parser for other tests.
        from researchai_document_engine.parsers import text_extractors

        parsers._REGISTRY["txt"] = text_extractors.parse_txt
