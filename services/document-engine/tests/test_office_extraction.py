"""OOXML/EPUB parser tests: PPTX, XLSX, EPUB (stdlib-built fixtures)."""

from __future__ import annotations

import zipfile
from pathlib import Path

from researchai_document_engine import parsers


def _zip(path: Path, entries: dict[str, str]) -> str:
    with zipfile.ZipFile(path, "w") as z:
        for name, body in entries.items():
            z.writestr(name, body)
    return str(path)


# ---------------------------------------------------------------------------
# PPTX
# ---------------------------------------------------------------------------


def _slide(n: int, paragraphs: list[str]) -> str:
    # One <a:p> per paragraph — that is how PowerPoint writes it.
    bodies = "".join(
        f"<a:p><a:r><a:t>{t}</a:t></a:r></a:p>" for t in paragraphs
    )
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" '
        'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
        f'id="slide{n}">'
        f"<p:cSld><p:spTree><p:sp><p:txBody>{bodies}</p:txBody></p:sp>"
        "</p:spTree></p:cSld></p:sld>"
    )


def test_pptx_sections_per_slide_with_page_numbers(tmp_path: Path) -> None:
    path = _zip(
        tmp_path / "deck.pptx",
        {
            "ppt/slides/slide2.xml": _slide(2, ["Second slide body."]),
            "ppt/slides/slide1.xml": _slide(1, ["Title slide", "Subtitle line"]),
            "docProps/core.xml": (
                '<?xml version="1.0"?><cp:coreProperties '
                'xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
                'xmlns:dc="http://purl.org/dc/elements/1.1/">'
                "<dc:title>Coastal Retreat Deck</dc:title></cp:coreProperties>"
            ),
        },
    )
    res = parsers.get_parser("pptx")(path)
    assert res.ok, res.error
    assert res.title == "Coastal Retreat Deck"
    assert res.page_count == 2

    headings = [s.heading for s in res.sections]
    # slide1 must sort before slide2 even though slide2 was written first.
    assert headings == ["Slide 1", "Slide 2"]
    assert [s.page_start for s in res.sections] == [1, 2]

    texts = [b.text for b in res.blocks]
    assert "Title slide" in texts and "Second slide body." in texts
    assert all(b.page in (1, 2) for b in res.blocks)
    assert all(b.section_index < len(res.sections) for b in res.blocks)

    # Rebuild the assembled payload exactly as the parser does (paragraphs
    # joined by "\n\n" across slides) and check every offset slices cleanly.
    assembled = "\n\n".join(["Title slide", "Subtitle line"] + ["Second slide body."])
    for b in res.blocks:
        assert b.start_offset is not None and b.end_offset is not None
        assert assembled[b.start_offset : b.end_offset] == b.text
    # The slide separator must be accounted for: the last block ends exactly
    # at the end of the payload.
    assert res.blocks[-1].end_offset == len(assembled)


def test_pptx_corrupt_container_reports_honestly(tmp_path: Path) -> None:
    bad = tmp_path / "broken.pptx"
    bad.write_bytes(b"PK\x03\x04 not really a zip")
    res = parsers.get_parser("pptx")(str(bad))
    assert not res.ok
    assert res.error


def test_pptx_without_slides_is_an_error(tmp_path: Path) -> None:
    path = _zip(tmp_path / "empty.pptx", {"[Content_Types].xml": "<Types/>"})
    res = parsers.get_parser("pptx")(path)
    assert not res.ok
    assert "no slides" in (res.error or "").lower()


# ---------------------------------------------------------------------------
# XLSX
# ---------------------------------------------------------------------------


def _xlsx(tmp_path: Path, sheet_rows: dict[str, list[list[str]]], shared: list[str] | None = None) -> str:
    """Minimal XLSX using inline strings (no sharedStrings part required)."""
    shared = shared or []
    entries: dict[str, str] = {}

    sheets_xml = "".join(
        f'<sheet name="{name}" sheetId="{i}" r:id="rId{i}"/>'
        for i, name in enumerate(sheet_rows, start=1)
    )
    entries["xl/workbook.xml"] = (
        '<?xml version="1.0"?><workbook '
        'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
        f"<sheets>{sheets_xml}</sheets></workbook>"
    )
    rels = "".join(
        f'<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/'
        f'officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{i}.xml"/>'
        for i in range(1, len(sheet_rows) + 1)
    )
    entries["xl/_rels/workbook.xml.rels"] = (
        '<?xml version="1.0"?><Relationships '
        'xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        f"{rels}</Relationships>"
    )
    if shared:
        sis = "".join(f"<si><t>{s}</t></si>" for s in shared)
        entries["xl/sharedStrings.xml"] = (
            '<?xml version="1.0"?><sst '
            'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
            f"{sis}</sst>"
        )

    for i, rows in enumerate(sheet_rows.values(), start=1):
        row_xml = []
        for r, values in enumerate(rows, start=1):
            cells = []
            for c, value in enumerate(values):
                col = chr(ord("A") + c)
                if shared and value.isdigit() and int(value) < len(shared):
                    cells.append(f'<c r="{col}{r}" t="s"><v>{value}</v></c>')
                elif value:
                    cells.append(
                        f'<c r="{col}{r}" t="inlineStr"><is><t>{value}</t></is></c>'
                    )
            row_xml.append(f'<row r="{r}">{"".join(cells)}</row>')
        entries[f"xl/worksheets/sheet{i}.xml"] = (
            '<?xml version="1.0"?><worksheet '
            'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
            f'<sheetData>{"".join(row_xml)}</sheetData></worksheet>'
        )

    return _zip(tmp_path / "book.xlsx", entries)


def test_xlsx_one_section_per_sheet_rows_as_blocks(tmp_path: Path) -> None:
    path = _xlsx(
        tmp_path,
        {
            "Field data": [["Site", "Erosion m/yr"], ["Delta A", "1.4"]],
            "Summary": [["Mean", "1.4"]],
        },
    )
    res = parsers.get_parser("xlsx")(path)
    assert res.ok, res.error
    assert [s.heading for s in res.sections] == ["Field data", "Summary"]
    texts = [b.text for b in res.blocks]
    assert "Site | Erosion m/yr" in texts
    assert "Delta A | 1.4" in texts
    assert all(b.kind == "table" for b in res.blocks)
    # Spreadsheet rows have no page concept.
    assert all(b.page is None for b in res.blocks)
    for b in res.blocks:
        assert b.start_offset is not None and b.end_offset is not None


def test_xlsx_shared_strings_resolve(tmp_path: Path) -> None:
    path = _xlsx(tmp_path, {"S1": [["0", "1"]]}, shared=["Alpha", "Beta"])
    res = parsers.get_parser("xlsx")(path)
    assert res.ok, res.error
    assert any("Alpha | Beta" == b.text for b in res.blocks)


def test_xlsx_corrupt_container_reports_honestly(tmp_path: Path) -> None:
    bad = tmp_path / "bad.xlsx"
    bad.write_bytes(b"garbage not a zip")
    res = parsers.get_parser("xlsx")(str(bad))
    assert not res.ok
    assert res.error


# ---------------------------------------------------------------------------
# EPUB
# ---------------------------------------------------------------------------


def _epub(tmp_path: Path, chapters: list[tuple[str, str]]) -> str:
    """chapters: `(file name, html body)` in reading order."""
    manifest = "".join(
        f'<item id="c{i}" href="{name}" media-type="application/xhtml+xml"/>'
        for i, (name, _) in enumerate(chapters, start=1)
    )
    spine = "".join(
        f'<itemref idref="c{i}"/>' for i in range(1, len(chapters) + 1)
    )
    entries: dict[str, str] = {
        "META-INF/container.xml": (
            '<?xml version="1.0"?><container version="1.0" '
            'xmlns="urn:oasis:names:tc:opendocument:xmlns:container">'
            '<rootfiles><rootfile full-path="OEBPS/content.opf" '
            'media-type="application/oebps-package+xml"/></rootfiles></container>'
        ),
        "OEBPS/content.opf": (
            '<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" '
            'xmlns:dc="http://purl.org/dc/elements/1.1/">'
            "<metadata><dc:title>Field Guide</dc:title></metadata>"
            f"<manifest>{manifest}</manifest><spine>{spine}</spine></package>"
        ),
    }
    for name, body in chapters:
        entries[f"OEBPS/{name}"] = (
            "<?xml version='1.0' encoding='utf-8'?>"
            "<html><head><title>x</title></head>"
            f"<body>{body}</body></html>"
        )
    return _zip(tmp_path / "book.epub", entries)


def test_epub_one_section_per_chapter_in_spine_order(tmp_path: Path) -> None:
    path = _epub(
        tmp_path,
        [
            ("ch1.xhtml", "<h1>Introduction</h1><p>Opening argument.</p>"),
            ("ch2.xhtml", "<h1>Methods</h1><p>How we measured.</p>"),
        ],
    )
    res = parsers.get_parser("epub")(path)
    assert res.ok, res.error
    assert res.title == "Field Guide"
    assert [s.heading for s in res.sections] == ["Introduction", "Methods"]
    texts = [b.text for b in res.blocks]
    assert any("Opening argument." in t for t in texts)
    assert all(b.section_index < len(res.sections) for b in res.blocks)


def test_epub_offsets_index_into_assembled_text(tmp_path: Path) -> None:
    path = _epub(
        tmp_path,
        [
            ("a.xhtml", "<h1>Alpha</h1><p>First chapter body.</p>"),
            ("b.xhtml", "<h1>Beta</h1><p>Second chapter body.</p>"),
        ],
    )
    res = parsers.get_parser("epub")(path)
    assert res.ok, res.error
    # Reconstruct the assembled payload the way the parser built it, then
    # check every block's extent slices back to its own text.
    assembled = "\n\n".join(
        _assembled_chapter(tmp_path, name, body)
        for name, body in [
            ("a.xhtml", "<h1>Alpha</h1><p>First chapter body.</p>"),
            ("b.xhtml", "<h1>Beta</h1><p>Second chapter body.</p>"),
        ]
    )
    for b in res.blocks:
        assert b.start_offset is not None and b.end_offset is not None
        assert assembled[b.start_offset : b.end_offset] == b.text


def _assembled_chapter(tmp_path: Path, name: str, body: str) -> str:
    from researchai_document_engine.parsers.text_extractors import _html_to_markdown

    return _html_to_markdown(
        f"<html><head><title>x</title></head><body>{body}</body></html>"
    )


def test_epub_missing_container_is_an_error(tmp_path: Path) -> None:
    path = _zip(tmp_path / "bad.epub", {"OEBPS/x.xhtml": "<p>orphan</p>"})
    res = parsers.get_parser("epub")(path)
    assert not res.ok
    assert res.error


def test_epub_rejects_non_zip(tmp_path: Path) -> None:
    bad = tmp_path / "junk.epub"
    bad.write_bytes(b"<html>not a zip</html>")
    res = parsers.get_parser("epub")(str(bad))
    assert not res.ok
    assert res.error
