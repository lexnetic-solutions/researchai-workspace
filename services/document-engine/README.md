# ResearchAI Document Engine

Python sidecar that will own document parsing, OCR and structure extraction
(Docling) from Phase 1 onwards. Phase 0 ships the service scaffold: app
bootstrap, health endpoint, a strict parse contract and an ingest/parse
registry with honest `NotImplemented` responses.

## Run

```bash
pnpm engine:run          # serves http://127.0.0.1:8737
pnpm dev:engine          # same, with readiness polling (used with desktop dev)
```

## Test

```bash
pnpm engine:test
```

The desktop app's status bar and diagnostics probe `GET /health` on port 8737.
The engine is **local-only** — it binds to loopback and performs no network I/O.
