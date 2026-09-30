# Phase 1 test fixtures

- `sample.md` — Markdown fixture (headings + paragraphs). Committed.
- `sample.docx` / `sample.pdf` — tiny generated binaries, committed so the
  live integration test runs without a Python bootstrap step.

Regenerate after changing the generator:

```bash
cd services/document-engine && uv run python ../../tests/fixtures/phase1/generate_fixtures.py
```

Used by `apps/desktop/src-tauri/tests/live_ingestion.rs` (engine must be
running: `pnpm engine:run`).
