# Citations (Phase 5 — implemented)

Citations in ResearchAI are **deterministic**: formatted from structured
bibliographic metadata. An LLM never invents a formatted reference when
metadata exists.

## Implementation decision (licensing)

The original plan named Citation.js + CSL. Citation.js is **AGPL-3.0** —
flagged for commercial review in THIRD_PARTY_LICENSES.md — so Phase 5 ships
with a **built-in deterministic formatter**
([citations.rs](../apps/desktop/src-tauri/src/services/citations.rs)): zero
external dependencies, MIT-clean, fully offline, and byte-for-byte
test-locked. The `CitationFormatter` trait keeps the engine swappable; a CSL
engine (e.g. Citation.js behind a sidecar, or a MIT-licensed CSL
implementation) can replace it later without touching call sites.

## Data model

- `documents` carries the metadata (title, authors, year, doi, journal,
  volume, issue, pages, publisher, url, ref_type — migration 6).
- The engine extracts **hints** (DOI, year) during parsing; hints only ever
  fill EMPTY fields.
- Metadata is user-correctable in the Library reader; corrections are
  authoritative over extracted values. AI suggestions never silently
  overwrite metadata (spec §31).

## Styles

Initial: **APA 7, Harvard, Chicago** (author-date), implemented and
unit-locked. Selectable in the Bibliography view; later: MLA, IEEE,
Vancouver, university styles (each is a new function behind the same
trait).

## Location references

Beyond bibliography entries, every in-text citation resolves to a *location*:
document → page → section → chunk offsets, produced during ingestion
(§15 chunking) and rendered in the UI as a jump link into the PDF viewer
(spec §22: click citation → open document → jump to page → highlight passage).

## Interface boundary

Citation formatting lives behind `citation-core` with a `CitationFormatter`
interface so the Citation.js/AGPL implementation can be swapped (see
[THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md) — flagged for commercial
review) without touching call sites.
