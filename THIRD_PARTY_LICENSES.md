# Third-Party Licenses

ResearchAI Workspace depends on third-party software. This file records what we use,
its license, and why. **Nothing here grants redistribution rights** — licenses are
re-verified at each integration and before any commercial release.

Rule (spec §3): upstream projects are studied via reference clones *outside* this
repository; we integrate through package managers or native sidecar binaries, never
by copying source trees into this repository.

## Desktop shell

| Component | Role | License | Source |
|---|---|---|---|
| Tauri 2 | App shell, packaging, IPC | Apache-2.0 OR MIT | github.com/tauri-apps/tauri |
| React | Frontend UI | MIT | github.com/facebook/react |
| Vite | Frontend build tool | MIT | github.com/vitejs/vite |
| TypeScript | Frontend language | Apache-2.0 | github.com/microsoft/TypeScript |
| Rust toolchain | Backend language | MIT OR Apache-2.0 | rust-lang.org |

## Rust backend (runtime)

| Component | Role | License | Source |
|---|---|---|---|
| rusqlite + libsqlite3-sys (bundled SQLite) | Local database | MIT (rusqlite); SQLite is public domain | github.com/rusqlite/rusqlite |
| serde / serde_json | Serialisation | MIT OR Apache-2.0 | github.com/serde-rs/serde |
| chrono | Timestamps | MIT OR Apache-2.0 | github.com/chronotope/chrono |
| uuid | Record identifiers | MIT OR Apache-2.0 | github.com/uuid-rs/uuid |
| sha2 / hex | Checksums (duplicate detection) | MIT OR Apache-2.0 | github.com/RustCrypto/hashes |
| thiserror | Error ergonomics | MIT OR Apache-2.0 | github.com/dtolnay/thiserror |
| log | Logging facade | MIT OR Apache-2.0 | github.com/rust-lang/log |
| ureq | Loopback HTTP client for the document engine | MIT OR Apache-2.0 | github.com/algesten/ureq |
| sqlite-vec | Vector search in SQLite | MIT OR Apache-2.0 | github.com/asg017/sqlite-vec |
| sysinfo | Hardware diagnostics (RAM/CPU detection) | MIT | github.com/GuillaumeGomez/sysinfo |
| tauri-plugin-dialog | Native file/folder pickers | Apache-2.0 OR MIT | github.com/tauri-apps/plugins-workspace |
| tauri-plugin-opener | Reveal files in OS file manager | Apache-2.0 OR MIT | github.com/tauri-apps/plugins-workspace |

## Document engine (Python sidecar)

| Component | Role | License | Source |
|---|---|---|---|
| FastAPI | Sidecar HTTP service | MIT | github.com/fastapi/fastapi |
| uvicorn | ASGI server | BSD-3-Clause | github.com/encode/uvicorn |
| pydantic | Request/response contracts | MIT | github.com/pydantic/pydantic |
| fastembed | Local embeddings (ONNX, BGE-small) | Apache-2.0 | github.com/qdrant/fastembed |
| pypdf | PDF text extraction (Phase 1) | BSD-3-Clause | github.com/py-pdf/pypdf |
| python-docx | DOCX parsing (Phase 1) | MIT | github.com/python-openxml/python-docx |
| charset-normalizer | Text encoding detection | MIT | github.com/Ousret/charset_normalizer |
| pytest / httpx | Sidecar tests | MIT / BSD-3-Clause | github.com/pytest-dev/pytest |
| uv | Python environment manager | MIT OR Apache-2.0 | github.com/astral-sh/uv |

## Planned — verify license at integration time (Phases 1–8)

| Component | Planned role | License (verify) | Notes |
|---|---|---|---|
| Docling | Document parsing/OCR sidecar | MIT | github.com/docling-project/docling |
| llama.cpp | Local LLM inference | MIT | GGUF models have their own licenses |
| reportlab | PDF export rendering (document engine) | BSD-3-Clause | Permissive; no copyleft obligations |
| whisper.cpp | Speech-to-text | MIT | Model weights (OpenAI) — MIT license; verify redistribution terms |
| sqlite-vec | Vector search in SQLite | MIT OR Apache-2.0 | pre-v1 — isolated behind VectorStore interface (see Rust table for current usage) |
| sentence-transformers | Local embeddings | Apache-2.0 | Model weights have separate licenses |
| pdf.js | In-app PDF viewer | Apache-2.0 | github.com/mozilla/pdf.js |
| FFmpeg | Audio/video processing | LGPL-2.1+ / GPL (build-dependent) | Must bundle LGPL build, not GPL, unless product becomes GPL-compatible |
| piper1-gpl (Piper TTS) | Local text-to-speech | **GPL-3.0** | ⚠️ Isolated behind TextToSpeechProvider; re-evaluate before any commercial distribution (spec §7) |
| citation-js | Bibliography formatting | **AGPL-3.0** | ⚠️ Verify before Phase 5; may need a non-copyleft alternative for commercial release |
| CSL styles | Citation styles | CC BY-SA 3.0 (verify per-style) | github.com/citation-style-language/styles |
| python-pptx | PPTX export | MIT | github.com/scanny/python-pptx |
| openpyxl | XLSX export | MIT | openpyxl.readthedocs.io |

## Review checklist before commercial distribution

1. Re-run license verification for every row above (repos change licenses).
2. Confirm no GPL/AGPL component is statically linked, copied, or distributed in a way
   that triggers copyleft obligations on ResearchAI (Piper and citation-js are the
   flagged risks; both are interface-isolated for that reason).
3. Confirm FFmpeg build configuration (LGPL build without GPL flags).
4. Confirm model-weight licenses for any bundled/downloaded GGUF or embedding models.
5. Regenerate this file from the actual lockfiles (`pnpm licenses list`,
   `cargo license`, `uv export`) as part of the release checklist (Phase 9).
