"""Parser registry.

Phase 0 ships the dispatch contract only; the Docling-backed implementations
land in Phase 1 (spec §14). Parsers are registered per extension so the Rust
core and tests can reason about capability without importing heavy libraries.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from ..contracts import ParseResponse

Parser = Callable[[str], ParseResponse]

_REGISTRY: dict[str, Parser] = {}

# Formats targeted for Phase 1 (spec §4.2).
PLANNED_EXTENSIONS: frozenset[str] = frozenset(
    {"pdf", "docx", "pptx", "xlsx", "html", "htm", "md", "txt", "epub"}
)


def register(ext: str) -> Callable[[Parser], Parser]:
    """Register a parser for a lower-case file extension."""

    def decorator(fn: Parser) -> Parser:
        _REGISTRY[ext.lower()] = fn
        return fn

    return decorator


def get_parser(ext: str) -> Parser | None:
    return _REGISTRY.get(ext.lower())


def supported_now() -> list[str]:
    return sorted(_REGISTRY)


def planned() -> list[str]:
    return sorted(PLANNED_EXTENSIONS)


def not_implemented(document_id: str, ext: str) -> ParseResponse:
    """Honest response for formats without a registered parser."""
    return ParseResponse(
        document_id=document_id,
        ok=False,
        error=f".{ext} files are not supported yet.",
    )


__all__ = [
    "ParseResponse",
    "Path",
    "get_parser",
    "not_implemented",
    "planned",
    "register",
    "supported_now",
]

# Importing the extractor module registers the real Phase 1 parsers
# (pdf, docx, txt, md, html, htm) with this registry.
from . import text_extractors as _text_extractors  # noqa: F401
