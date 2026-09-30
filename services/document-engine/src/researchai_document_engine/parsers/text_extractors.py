"""Real text-extraction parsers (Phase 1).

Module-level imports: `logging` for quiet diagnostics.

Dispatch is by extension via the registry in `parsers/__init__`. Every parser
returns the same `ParseResponse` contract: plain text, structural sections
(headings with page ranges where the format exposes them), blocks with page
numbers where the format exposes them, and honest per-file errors.

Included engines (all permissive licenses, recorded in THIRD_PARTY_LICENSES.md):
    PDF  → pypdf        (pure Python; scanned-PDF OCR arrives with Docling)
    DOCX → python-docx  (paragraphs + heading structure)
    TXT/MD/HTML/HTM → charset-normalizer decode; Markdown headings
"""

from __future__ import annotations

import logging
import re
from pathlib import Path

from ..contracts import ParsedBlock, ParseResponse, SectionSpan
from . import not_implemented, register


def _ok(
    document_id: str,
    text: str,
    sections: list[SectionSpan],
    blocks: list[ParsedBlock],
    page_count: int | None = None,
    title: str | None = None,
    ext: str | None = None,
) -> ParseResponse:
    return ParseResponse(
        document_id=document_id,
        ok=True,
        page_count=page_count,
        language=None,
        title=title,
        sections=sections,
        blocks=blocks,
    )


def _error(document_id: str, message: str, ext: str) -> ParseResponse:
    # Keep the error human-readable; details go to engine logs.
    return ParseResponse(document_id=document_id, ok=False, error=f"{message}")


# ---------------------------------------------------------------------------
# Plain-text family
# ---------------------------------------------------------------------------

_MD_HEADING = re.compile(r"^(#{1,6})\s+(.*)$")


@register("txt")
def parse_txt(path: str) -> ParseResponse:
    return _parse_plaintext(path, ext="txt")


@register("md")
def parse_md(path: str) -> ParseResponse:
    return _parse_plaintext(path, ext="md")


def _decode(path: Path) -> str:
    """Decode bytes with charset detection; never raises for text files."""
    raw = path.read_bytes()
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        from charset_normalizer import from_bytes

        best = from_bytes(raw).best()
        if best is None:
            raise ValueError("Could not detect a text encoding for this file.")
        return str(best)


def _parse_plaintext(path: str, ext: str) -> ParseResponse:
    p = Path(path)
    document_id = p.stem or "document"
    try:
        text = _decode(p)
    except ValueError as e:
        return _error(document_id, str(e), ext)

    sections: list[SectionSpan] = []
    blocks: list[ParsedBlock] = []
    if ext == "md":
        # Markdown: use ATX headings as sections, group blocks under them.
        # Offsets are character positions for stable citation mapping (§15).
        sections.append(SectionSpan(heading="(Front matter)", level=1, order_index=0))
        current_idx = 0
        offset = 0
        for raw_line in text.splitlines(keepends=True):
            line = raw_line.rstrip("\n").rstrip("\r")
            m = _MD_HEADING.match(line)
            if m:
                sections.append(
                    SectionSpan(
                        heading=m.group(2).strip() or "(Untitled)",
                        level=len(m.group(1)),
                        order_index=len(sections),
                    )
                )
                current_idx = len(sections) - 1
            elif line.strip():
                # Extent excludes the newline so text[start:end] == text.
                blocks.append(
                    ParsedBlock(
                        section_index=current_idx,
                        kind="paragraph",
                        text=line,
                        page=None,
                        start_offset=offset,
                        end_offset=offset + len(line),
                    )
                )
            offset += len(raw_line)
    else:
        sections.append(SectionSpan(heading="(Document)", level=1, order_index=0))
        offset = 0
        for para in re.split(r"\n\s*\n", text):
            stripped = para.strip()
            if stripped:
                start = text.find(stripped, offset)
                if start == -1:
                    start = text.find(stripped)
                if start >= 0:
                    blocks.append(
                        ParsedBlock(
                            section_index=0,
                            kind="paragraph",
                            text=stripped,
                            page=None,
                            start_offset=start,
                            end_offset=start + len(stripped),
                        )
                    )
            offset = offset + len(para)

    title = p.stem
    return _ok(document_id, text, sections, blocks, page_count=None, title=title, ext=ext)


# ---------------------------------------------------------------------------
# HTML (basic structural extraction; sanitisation happens in the UI layer)
# ---------------------------------------------------------------------------

@register("html")
@register("htm")
def parse_html(path: str) -> ParseResponse:
    p = Path(path)
    document_id = p.stem or "document"
    try:
        raw = _decode(p)
    except ValueError as e:
        return _error(document_id, str(e), "html")

    # Minimal, dependency-free extraction: strip scripts/styles, convert
    # headings to Markdown-ish markers so sections flow out of the same path.
    cleaned = re.sub(r"(?is)<(script|style)[^>]*>.*?</\1>", "", raw)
    cleaned = re.sub(r"(?is)<head[^>]*>.*?</head>", "", cleaned)
    text = re.sub(r"(?is)<br\s*/?>", "\n", cleaned)
    text = re.sub(r"(?is)</p>", "\n\n", text)
    for level in range(1, 7):
        text = re.sub(
            rf"(?is)<h{level}[^>]*>(.*?)</h{level}>",
            lambda m, lv=level: "\n" + "#" * lv + " " + m.group(1).strip() + "\n",
            text,
        )
    text = re.sub(r"(?is)<[^>]+>", " ", text)
    text = re.sub(r"[ \t]+", " ", text)
    text = re.sub(r"\n\s+", "\n", text).strip()

    # Route through the markdown parser for consistent sectioning.
    tmp = p.with_suffix(".md.tmp")
    tmp.write_text(text, encoding="utf-8")
    try:
        response = _parse_plaintext(str(tmp), ext="md")
        response.document_id = document_id
        return response
    finally:
        tmp.unlink(missing_ok=True)


# ---------------------------------------------------------------------------
# DOCX — python-docx
# ---------------------------------------------------------------------------

@register("docx")
def parse_docx(path: str) -> ParseResponse:
    p = Path(path)
    document_id = p.stem or "document"
    try:

        import docx  # python-docx

        document = docx.Document(str(p))
    except Exception as e:  # noqa: BLE001 — honest per-file errors (§42)
        return _error(document_id, f"Could not read DOCX: {e}", "docx")

    sections: list[SectionSpan] = [SectionSpan(heading="(Document)", level=1, order_index=0)]
    blocks: list[ParsedBlock] = []
    offset = 0
    parts: list[str] = []

    for para in document.paragraphs:
        content = para.text.strip()
        style = (para.style.name if para.style is not None else "") or ""
        if not content:
            parts.append("\n")
            offset += 1
            continue
        if style.startswith("Heading"):
            try:
                level = int(style.split()[-1])
            except ValueError:
                level = 2
            level = max(1, min(6, level))
            sections.append(
                SectionSpan(
                    heading=content,
                    level=level,
                    order_index=len(sections),
                )
            )
            marker = f"{'#' * level} {content}\n"
            parts.append(marker)
            offset += len(marker)
        else:
            idx = len(sections) - 1
            blocks.append(
                ParsedBlock(
                    section_index=idx,
                    kind="paragraph",
                    text=content,
                    page=None,
                    start_offset=offset,
                    end_offset=offset + len(content),
                )
            )
            parts.append(content + "\n")
            offset += len(content) + 1

    text = "\n".join(parts) if parts else ""
    title = next((b.text for b in blocks if b.text), None)
    title = title.split("\n")[0][:120] if title else None
    return _ok(document_id, text, sections, blocks, page_count=None, title=title)


# ---------------------------------------------------------------------------
# PDF — pypdf
# ---------------------------------------------------------------------------

@register("pdf")
def parse_pdf(path: str) -> ParseResponse:
    p = Path(path)
    document_id = p.stem or "document"
    try:
        from pypdf import PdfReader

        reader = PdfReader(str(p))
    except Exception as e:  # noqa: BLE001
        return _error(document_id, f"Could not read PDF: {e}", "pdf")

    sections: list[SectionSpan] = []
    blocks: list[ParsedBlock] = []
    page_texts: list[str] = []

    for page_index, page in enumerate(reader.pages, start=1):
        try:
            page_text = page.extract_text() or ""
        except Exception:  # noqa: BLE001
            page_text = ""
        page_texts.append(page_text)

        # One section per page keeps citation jumps trivially accurate.
        sections.append(
            SectionSpan(
                heading=f"Page {page_index}",
                level=2,
                page_start=page_index,
                page_end=page_index,
                order_index=page_index - 1,
            )
        )

        # Split pages into paragraph-ish blocks; offsets are computed after
        # the loop over the assembled text.
        for para in re.split(r"\n\s*\n", page_text):
            stripped = para.strip()
            if stripped:
                blocks.append(
                    ParsedBlock(
                        section_index=len(sections) - 1,
                        kind="paragraph",
                        text=stripped,
                        page=page_index,
                        start_offset=0,  # fixed up after assembly
                        end_offset=0,
                    )
                )

    text = "\n\n".join(page_texts).strip()
    # Fix up offsets over the assembled document text (§15).
    cursor = 0
    for block in blocks:
        found = text.find(block.text, cursor)
        if found == -1:
            found = text.find(block.text)
        if found >= 0:
            block.start_offset = found
            block.end_offset = found + len(block.text)
            cursor = found + len(block.text)

    # Title heuristic: metadata first, then largest early text block.
    title = None
    try:
        meta_title = (reader.metadata or {}).get("/Title")
        if isinstance(meta_title, str) and meta_title.strip():
            title = meta_title.strip()
    except Exception as exc:  # noqa: BLE001
        logging.getLogger(__name__).debug("pdf metadata unreadable: %s", exc)
    if not title and blocks:
        first = blocks[0].text
        title = first.split("\n")[0][:120] if first else None

    return _ok(
        document_id,
        text,
        sections,
        blocks,
        page_count=len(reader.pages),
        title=title,
        ext="pdf",
    )


__all__ = ["not_implemented"]
