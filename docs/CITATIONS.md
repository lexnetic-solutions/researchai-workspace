# Citations (Phase 5)

Citations in ResearchAI are **deterministic**: formatted from structured
bibliographic metadata via Citation.js + CSL styles (spec §31). An LLM never
invents a formatted reference when metadata exists.

## Data model

- `documents` carries extracted metadata (title, authors, year, doi, …).
- `citations` links a document/chunk to its bibliography metadata.
- Metadata is user-correctable; corrections are authoritative over extracted
  values. AI suggestions never silently overwrite metadata (spec §31).

## Styles

Initial: **APA 7, Harvard, Chicago** (CSL from the official
citation-style-language/styles collection). Selectable in Settings; later:
MLA, IEEE, Vancouver, university styles.

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
