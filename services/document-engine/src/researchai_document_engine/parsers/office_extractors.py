"""OOXML and EPUB parsers: PPTX, XLSX, EPUB (spec §4.2 formats).

All three containers are ZIP archives of XML, so the standard library is
enough: `zipfile` + `xml.etree.ElementTree`. That keeps the PyInstaller
sidecar unchanged (no hidden imports, no new licences to track).

Each parser returns the shared `ParseResponse` contract: structural sections,
text blocks with citation offsets, and honest per-file errors (§42).

    PPTX  → one section per slide (page = slide number)
    XLSX  → one section per sheet, one block per non-empty row
    EPUB  → one section per spine chapter (title from its first heading)
"""

from __future__ import annotations

import re
import zipfile
from pathlib import Path, PurePosixPath
from xml.etree import ElementTree as ET

from ..contracts import ParsedBlock, ParseResponse, SectionSpan
from . import register
from .text_extractors import _bibliographic_hints, _error, _html_to_markdown, _md_structure

_A = "{http://schemas.openxmlformats.org/drawingml/2006/main}"
_X = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"
_R = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
_REL = "{http://schemas.openxmlformats.org/package/2006/relationships}"
_CONTAINER = "{urn:oasis:names:tc:opendocument:xmlns:container}"
_OPF = "{http://www.idpf.org/2007/opf}"
_DC = "{http://purl.org/dc/elements/1.1/}"

# A malicious or pathological file must not make the worker spin: EPUB spine
# and PPTX slide walks are bounded (§43 bounded work on the shared port).
_MAX_EPUB_CHAPTERS = 500
_MAX_SLIDES = 2000


def _open_zip(path: Path, document_id: str, ext: str) -> tuple[zipfile.ZipFile | None, ParseResponse | None]:
    try:
        return zipfile.ZipFile(path), None
    except (zipfile.BadZipFile, OSError) as e:
        return None, _error(document_id, f"Could not read the {ext.upper()} container: {e}", ext)


def _zip_text(z: zipfile.ZipFile, name: str) -> str | None:
    try:
        return z.read(name).decode("utf-8", errors="replace")
    except KeyError:
        return None
    except (OSError, zipfile.BadZipFile):
        return None


def _xml_root(z: zipfile.ZipFile, name: str) -> ET.Element | None:
    raw = _zip_text(z, name)
    if raw is None:
        return None
    try:
        return ET.fromstring(raw)
    except ET.ParseError:
        return None


def _package_title(z: zipfile.ZipFile) -> str | None:
    """`docProps/core.xml` → `dc:title`, whichever namespace it lives in."""
    root = _xml_root(z, "docProps/core.xml")
    if root is None:
        return None
    for el in root.iter():
        if el.tag.rsplit("}", 1)[-1] == "title" and el.text and el.text.strip():
            return el.text.strip()[:200]
    return None


def _epub_title(opf_root: ET.Element) -> str | None:
    """OPF metadata → `dc:title` (EPUBs carry no docProps part)."""
    for el in opf_root.iter():
        if el.tag.rsplit("}", 1)[-1] == "title" and el.text and el.text.strip():
            return el.text.strip()[:200]
    return None


def _ok_with_hints(
    document_id: str,
    text: str,
    sections: list[SectionSpan],
    blocks: list[ParsedBlock],
    *,
    page_count: int | None = None,
    title: str | None = None,
) -> ParseResponse:
    doi, year = _bibliographic_hints(text)
    return ParseResponse(
        document_id=document_id,
        ok=True,
        page_count=page_count,
        language=None,
        title=title,
        sections=sections,
        blocks=blocks,
        doi=doi,
        year=year,
    )


# ---------------------------------------------------------------------------
# PPTX
# ---------------------------------------------------------------------------


def _slide_order(name: str) -> int:
    m = re.search(r"slide(\d+)\.xml$", name)
    return int(m.group(1)) if m else 0


@register("pptx")
def parse_pptx(path: str) -> ParseResponse:
    """One section per slide; paragraph runs (`a:p`/`a:t`) become blocks."""
    p = Path(path)
    document_id = p.stem or "document"
    zf, err = _open_zip(p, document_id, "pptx")
    if zf is None:
        assert err is not None
        return err

    with zf:
        slides = sorted(
            (n for n in zf.namelist() if re.fullmatch(r"ppt/slides/slide\d+\.xml", n)),
            key=_slide_order,
        )
        if not slides:
            return _error(document_id, "This presentation contains no slides.", "pptx")

        title = _package_title(zf)
        sections: list[SectionSpan] = []
        blocks: list[ParsedBlock] = []
        text_parts: list[str] = []
        cursor = 0
        first_text: str | None = None

        for page, name in enumerate(slides[:_MAX_SLIDES], start=1):
            root = _xml_root(zf, name)
            if root is None:
                continue  # one unreadable slide must not sink the deck
            paragraphs = [
                joined
                for para in root.iter(f"{_A}p")
                if (joined := "".join(t.text or "" for t in para.iter(f"{_A}t")).strip())
            ]
            if not paragraphs:
                continue
            sections.append(
                SectionSpan(
                    heading=f"Slide {page}",
                    level=1,
                    page_start=page,
                    page_end=page,
                    order_index=len(sections),
                )
            )
            section_index = len(sections) - 1
            for text in paragraphs:
                if first_text is None:
                    first_text = text
                if text_parts:
                    text_parts.append("\n\n")
                    cursor += 2
                start = cursor
                text_parts.append(text)
                cursor += len(text)
                blocks.append(
                    ParsedBlock(
                        section_index=section_index,
                        kind="paragraph",
                        text=text,
                        page=page,
                        start_offset=start,
                        end_offset=cursor,
                    )
                )

        if not sections:
            return _error(
                document_id,
                "No text could be extracted — this deck may be image-only.",
                "pptx",
            )

    assembled = "".join(text_parts)
    return _ok_with_hints(
        document_id,
        assembled,
        sections,
        blocks,
        page_count=len(sections),
        title=title or (first_text[:120] if first_text else None),
    )


# ---------------------------------------------------------------------------
# XLSX
# ---------------------------------------------------------------------------


def _shared_strings(z: zipfile.ZipFile) -> list[str]:
    root = _xml_root(z, "xl/sharedStrings.xml")
    if root is None:
        return []
    out: list[str] = []
    for si in root.iter(f"{_X}si"):
        # A shared string may be split across formatting runs (r/t).
        out.append("".join(t.text or "" for t in si.iter(f"{_X}t")))
    return out


def _cell_value(cell: ET.Element, shared: list[str]) -> str:
    kind = cell.get("t")
    if kind == "s":
        v = cell.find(f"{_X}v")
        try:
            return shared[int(v.text or "-1")] if v is not None else ""
        except (ValueError, IndexError):
            return ""
    if kind == "inlineStr":
        return "".join(t.text or "" for t in cell.iter(f"{_X}t"))
    v = cell.find(f"{_X}v")
    return (v.text or "") if v is not None else ""


def _sheet_targets(z: zipfile.ZipFile) -> list[tuple[str, str]]:
    """Ordered `(sheet name, zip path)` pairs from workbook.xml + its rels."""
    workbook = _xml_root(z, "xl/workbook.xml")
    if workbook is None:
        return []
    rels: dict[str, str] = {}
    rels_root = _xml_root(z, "xl/_rels/workbook.xml.rels")
    if rels_root is not None:
        for rel in rels_root.iter(f"{_REL}Relationship"):
            rid, target = rel.get("Id"), rel.get("Target")
            if rid and target:
                rels[rid] = target

    out: list[tuple[str, str]] = []
    for i, sheet in enumerate(workbook.iter(f"{_X}sheet"), start=1):
        name = sheet.get("name") or f"Sheet {i}"
        target = rels.get(sheet.get(f"{_R}id") or "")
        if not target:
            target = f"worksheets/sheet{i}.xml"
        if not target.startswith("xl/"):
            target = "xl/" + target.lstrip("/")
        out.append((name, target))

    if not out:
        # Minimal/renamed workbooks: fall back to the conventional layout.
        out = [
            (Path(n).stem, n)
            for n in sorted(z.namelist())
            if re.fullmatch(r"xl/worksheets/sheet\d+\.xml", n)
        ]
    return out


@register("xlsx")
def parse_xlsx(path: str) -> ParseResponse:
    """One section per sheet; each non-empty row becomes a ` | `-joined block."""
    p = Path(path)
    document_id = p.stem or "document"
    zf, err = _open_zip(p, document_id, "xlsx")
    if zf is None:
        assert err is not None
        return err

    with zf:
        shared = _shared_strings(zf)
        sheets = _sheet_targets(zf)
        if not sheets:
            return _error(document_id, "This workbook contains no worksheets.", "xlsx")

        title = _package_title(zf)
        sections: list[SectionSpan] = []
        blocks: list[ParsedBlock] = []
        text_parts: list[str] = []
        cursor = 0
        rows_seen = 0

        for sheet_name, target in sheets:
            root = _xml_root(zf, target)
            if root is None:
                continue
            lines: list[str] = []
            for row in root.iter(f"{_X}row"):
                values = [
                    text
                    for cell in row.iter(f"{_X}c")
                    if (text := _cell_value(cell, shared).strip())
                ]
                if values:
                    lines.append(" | ".join(values))
            if not lines:
                continue

            sections.append(
                SectionSpan(
                    heading=sheet_name,
                    level=1,
                    page_start=None,
                    page_end=None,
                    order_index=len(sections),
                )
            )
            section_index = len(sections) - 1
            for line in lines:
                if text_parts:
                    text_parts.append("\n\n")
                    cursor += 2
                start = cursor
                text_parts.append(line)
                cursor += len(line)
                blocks.append(
                    ParsedBlock(
                        section_index=section_index,
                        kind="table",
                        text=line,
                        page=None,
                        start_offset=start,
                        end_offset=cursor,
                    )
                )
                rows_seen += 1

        if not sections:
            return _error(
                document_id,
                "This workbook contains no readable rows.",
                "xlsx",
            )

    assembled = "".join(text_parts)
    return _ok_with_hints(
        document_id,
        assembled,
        sections,
        blocks,
        page_count=None,
        title=title or document_name(p),
    )


def document_name(p: Path) -> str:
    return p.stem or "document"


# ---------------------------------------------------------------------------
# EPUB
# ---------------------------------------------------------------------------


def _epub_chapters(z: zipfile.ZipFile) -> tuple[list[tuple[str, str]], str | None]:
    """`(zip path, media type)` per spine document, in reading order, plus
    the book title from the OPF metadata."""
    container = _xml_root(z, "META-INF/container.xml")
    if container is None:
        return [], None
    rootfile = next(
        (el.get("full-path") for el in container.iter(f"{_CONTAINER}rootfile") if el.get("full-path")),
        None,
    )
    if not rootfile:
        return [], None
    opf = _xml_root(z, rootfile)
    if opf is None:
        return [], None
    title = _epub_title(opf)

    manifest: dict[str, tuple[str, str]] = {}
    for item in opf.iter(f"{_OPF}item"):
        item_id, href = item.get("id"), item.get("href")
        if item_id and href:
            manifest[item_id] = (href, item.get("media-type") or "")

    base = PurePosixPath(rootfile).parent
    out: list[tuple[str, str]] = []
    for ref in opf.iter(f"{_OPF}itemref"):
        entry = manifest.get(ref.get("idref") or "")
        if not entry:
            continue
        href, media = entry
        if "html" not in media:
            continue
        # href is relative to the OPF; normalise `../` without leaving the zip.
        resolved = str(PurePosixPath(base, href).as_posix())
        while "/../" in resolved:
            head, _, tail = resolved.partition("/../")
            resolved = str(PurePosixPath(head).parent.as_posix()) + "/" + tail
        out.append((resolved, media))
        if len(out) >= _MAX_EPUB_CHAPTERS:
            break
    return out, title


_HEADING = re.compile(r"^(#{1,6})\s+(.*)$")


@register("epub")
def parse_epub(path: str) -> ParseResponse:
    """One section per spine chapter; headings inside become blocks.

    Offsets are rebased over the assembled book text so citation jumps land
    in the reader (§15).
    """
    p = Path(path)
    document_id = p.stem or "document"
    zf, err = _open_zip(p, document_id, "epub")
    if zf is None:
        assert err is not None
        return err

    with zf:
        chapters, opf_title = _epub_chapters(zf)
        if not chapters:
            return _error(
                document_id,
                "This EPUB has no readable chapters (no HTML documents in its spine).",
                "epub",
            )

        title = _package_title(zf) or opf_title
        sections: list[SectionSpan] = []
        blocks: list[ParsedBlock] = []
        text_parts: list[str] = []
        cursor = 0
        read_any = False

        for index, (name, _) in enumerate(chapters, start=1):
            raw = _zip_text(zf, name)
            if raw is None:
                continue
            markdown = _html_to_markdown(raw)
            if not markdown.strip():
                continue
            read_any = True

            first_heading = next(
                (m.group(2).strip() for line in markdown.splitlines() if (m := _HEADING.match(line))),
                None,
            )
            fallback = first_heading or PurePosixPath(name).stem or f"Chapter {index}"
            # Avoid a duplicate section when the chapter already opens with
            # its own title heading.
            front = None if first_heading and first_heading == fallback else fallback
            local_sections, local_blocks = _md_structure(markdown, front_matter=front)
            if not local_sections:
                local_sections = [SectionSpan(heading=fallback, level=1, order_index=0)]
                for b in local_blocks:
                    b.section_index = 0

            # Offsets are rebased over the assembled book: the chapter text
            # starts after the separator we are about to append.
            if text_parts:
                text_parts.append("\n\n")
                cursor += 2
            base_cursor = cursor
            text_parts.append(markdown)
            cursor += len(markdown)

            base_index = len(sections)
            for s in local_sections:
                s.order_index = len(sections)
                sections.append(s)
            for b in local_blocks:
                b.section_index += base_index
                if b.start_offset is not None:
                    b.start_offset += base_cursor
                if b.end_offset is not None:
                    b.end_offset += base_cursor
                blocks.append(b)

        if not read_any or not sections:
            return _error(
                document_id,
                "No text could be extracted from this EPUB.",
                "epub",
            )

    assembled = "".join(text_parts)
    return _ok_with_hints(
        document_id,
        assembled,
        sections,
        blocks,
        page_count=None,
        title=title,
    )


__all__ = ["parse_epub", "parse_pptx", "parse_xlsx"]
