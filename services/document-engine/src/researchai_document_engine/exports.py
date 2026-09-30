"""Document rendering for academic exports (Phase 6, spec §32).

The Rust core sends a neutral block list; this module renders DOCX
(python-docx) and PDF (reportlab). Rendering is intentionally simple and
deterministic: title, optional subtitle, then blocks in order. Academic
layout polish (headers/footers, page numbers) can be layered on later
without changing the contract.
"""

from __future__ import annotations

import io
from typing import Literal

from fastapi import HTTPException
from pydantic import BaseModel, Field

BlockKind = Literal["paragraph", "bullet", "heading", "table"]


class ExportBlock(BaseModel):
    kind: BlockKind
    text: str | None = None
    columns: list[str] | None = None
    rows: list[list[str]] | None = None


class ExportRequest(BaseModel):
    title: str = Field(min_length=1)
    subtitle: str | None = None
    blocks: list[ExportBlock] = Field(default_factory=list)


def _blocks_from_request(request: ExportRequest):
    """Yield validated (kind, payload) tuples; keeps both renderers simple."""
    for i, block in enumerate(request.blocks):
        if block.kind in ("paragraph", "bullet", "heading") and not block.text:
            raise HTTPException(
                status_code=400,
                detail=f"block {i}: text is required for kind={block.kind}",
            )
        if block.kind == "table" and (not block.columns or block.rows is None):
            raise HTTPException(
                status_code=400,
                detail=f"block {i}: columns and rows are required for tables",
            )
        yield block


def render_docx(request: ExportRequest) -> bytes:
    from docx import Document

    document = Document()
    document.add_heading(request.title, level=0)
    if request.subtitle:
        sub = document.add_paragraph(request.subtitle)
        sub.runs[0].italic = True

    for block in _blocks_from_request(request):
        if block.kind == "paragraph":
            document.add_paragraph(block.text)
        elif block.kind == "bullet":
            document.add_paragraph(block.text, style="List Bullet")
        elif block.kind == "heading":
            document.add_heading(block.text, level=1)
        elif block.kind == "table":
            table = document.add_table(rows=1 + len(block.rows), cols=len(block.columns))
            table.style = "Light Grid Accent 1"
            for col, cell in enumerate(block.columns):
                header = table.rows[0].cells[col]
                header.text = cell
                for run in header.paragraphs[0].runs:
                    run.font.bold = True
                if not header.paragraphs[0].runs:
                    header.paragraphs[0].add_run(cell).font.bold = True
            for r, row in enumerate(block.rows, start=1):
                for c, value in enumerate(row):
                    table.rows[r].cells[c].text = str(value)
            document.add_paragraph("")

    buffer = io.BytesIO()
    document.save(buffer)
    return buffer.getvalue()


def render_pdf(request: ExportRequest) -> bytes:
    from reportlab.lib.pagesizes import A4
    from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
    from reportlab.lib.units import mm
    from reportlab.platypus import (
        ListFlowable,
        ListItem,
        Paragraph,
        SimpleDocTemplate,
        Spacer,
        Table,
        TableStyle,
    )

    styles: dict[str, ParagraphStyle] = getSampleStyleSheet()
    body = styles["BodyText"]
    body_style = ParagraphStyle("BodyExport", parent=body, fontSize=10.5, leading=15)
    bullet_style = ParagraphStyle("BulletExport", parent=body_style)
    heading_style = ParagraphStyle(
        "HeadingExport", parent=styles["Heading2"], spaceBefore=14, spaceAfter=6
    )
    subtitle_style = ParagraphStyle(
        "SubtitleExport", parent=styles["Italic"], fontSize=10, spaceAfter=10
    )

    def esc(text: str) -> str:
        return (
            text.replace("&", "&amp;")
            .replace("<", "&lt;")
            .replace(">", "&gt;")
        )

    flow = [Paragraph(esc(request.title), styles["Title"])]
    if request.subtitle:
        flow.append(Paragraph(esc(request.subtitle), subtitle_style))
    flow.append(Spacer(1, 4 * mm))

    pending_bullets: list[str] = []

    def flush_bullets() -> None:
        nonlocal pending_bullets
        if pending_bullets:
            flow.append(
                ListFlowable(
                    [ListItem(Paragraph(esc(b), bullet_style)) for b in pending_bullets],
                    bulletType="bullet",
                )
            )
            pending_bullets = []

    for block in _blocks_from_request(request):
        if block.kind == "paragraph":
            flush_bullets()
            flow.append(Paragraph(esc(block.text or ""), body_style))
        elif block.kind == "bullet":
            pending_bullets.append(block.text or "")
        elif block.kind == "heading":
            flush_bullets()
            flow.append(Paragraph(esc(block.text or ""), heading_style))
        elif block.kind == "table":
            flush_bullets()
            data = [[esc(c) for c in block.columns or []]]
            for row in block.rows or []:
                data.append([esc(str(v)) for v in row])
            table = Table(data, hAlign="LEFT")
            table.setStyle(
                TableStyle(
                    [
                        ("GRID", (0, 0), (-1, -1), 0.4, "#999999"),
                        ("BACKGROUND", (0, 0), (-1, 0), "#e8e8f0"),
                        ("VALIGN", (0, 0), (-1, -1), "TOP"),
                        ("FONTSIZE", (0, 0), (-1, -1), 9),
                    ]
                )
            )
            flow.append(table)
            flow.append(Spacer(1, 4 * mm))
    flush_bullets()

    buffer = io.BytesIO()
    SimpleDocTemplate(buffer, pagesize=A4, title=request.title).build(flow)
    return buffer.getvalue()
