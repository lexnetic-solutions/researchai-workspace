"""Request/response contracts shared with the Rust core.

These mirror the Phase 1 ingestion model (spec §14): a parse request yields
structured sections, page mappings and metadata — never raw binary for the UI.
"""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, Field

HealthStatus = Literal["ok"]


class HealthResponse(BaseModel):
    status: HealthStatus = "ok"
    service: str = "researchai-document-engine"
    version: str


class ParseRequest(BaseModel):
    """Request to parse one managed document."""

    document_id: str = Field(min_length=1)
    managed_path: str = Field(min_length=1)
    mime_type: str | None = None
    force_ocr: bool = False


class SectionSpan(BaseModel):
    """A structural region of the parsed document."""

    heading: str
    level: int = Field(ge=1, le=6)
    page_start: int | None = None
    page_end: int | None = None
    order_index: int


class ParsedBlock(BaseModel):
    """One content block: paragraph, table, caption or list item."""

    section_index: int
    kind: Literal["paragraph", "heading", "table", "caption", "list", "formula"]
    text: str
    page: int | None = None
    start_offset: int | None = None
    end_offset: int | None = None


class ParseResponse(BaseModel):
    document_id: str
    ok: bool
    page_count: int | None = None
    language: str | None = None
    title: str | None = None
    sections: list[SectionSpan] = Field(default_factory=list)
    blocks: list[ParsedBlock] = Field(default_factory=list)
    error: str | None = None
