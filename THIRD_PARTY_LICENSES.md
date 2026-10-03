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

## Bundled AI assets (shipped inside the installer)

| Component | Role | License | Source |
|---|---|---|---|
| llama.cpp `llama-server` | Local LLM inference runtime (per-OS CPU builds, pinned) | MIT | github.com/ggml-org/llama.cpp — LICENSE file ships beside the binary in `Resources/llama/` |
| Qwen3-0.6B Q4_K_M (GGUF) | Starter instruct model for Ask-AI (~397 MB) | Apache-2.0 (weights) | huggingface.co/unsloth/Qwen3-0.6B-GGUF — base model Qwen/Qwen3-0.6B by Alibaba |
| BAAI/bge-small-en-v1.5 | Semantic-search embeddings (ONNX via fastembed, ~90 MB) | MIT | huggingface.co/BAAI/bge-small-en-v1.5 |

All three are fetched at build time by `scripts/package/fetch-ai-assets.mjs`
and `services/document-engine/packaging/seed_embeddings.py` from their
upstream sources — none of their code or weight files are committed to this
repository. Versions/URLs are pinned in the fetch script.

## Planned — verify license at integration time (Phases 1–8)

| Component | Planned role | License (verify) | Notes |
|---|---|---|---|
| Docling | Document parsing/OCR sidecar | MIT | github.com/docling-project/docling |
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

## Studied reference projects (cloned outside this repository)

Per the rule above, reference clones live outside the repo; ideas are
adapted into our own docs/code with attribution, never vendored wholesale.

| Project | License | What we adapted | Where it landed |
|---|---|---|---|
| Voicebox (github.com/jamiepine/voicebox) | MIT | Four agent skills (release-notes drafting, release bump, TTS-engine integration, PR triage) rewritten for this repo's mechanics; design-token/README presentation concept; sentence-splitter abbreviation handling and clause-boundary fallback for long-text TTS chunking | `.agents/skills/*`, `docs/DESIGN.md`, README hero/badges, `apps/desktop/src-tauri/src/services/tts.rs` splitter (credited in code comments) |

MIT permits this adaptation; the upstream copyright notice is retained in
the skill file headers ("Adapted from Voicebox … MIT"). No Voicebox source
files are committed to this repository.

## Review checklist before commercial distribution

1. Re-run license verification for every row above (repos change licenses).
2. Confirm no GPL/AGPL component is statically linked, copied, or distributed in a way
   that triggers copyleft obligations on ResearchAI (Piper and citation-js are the
   flagged risks; both are interface-isolated for that reason).
3. Confirm FFmpeg build configuration (LGPL build without GPL flags).
4. Confirm model-weight licenses for any bundled/downloaded GGUF or embedding models.
5. Regenerate this file from the actual lockfiles (`pnpm licenses list`,
   `cargo license`, `uv export`) as part of the release checklist (Phase 9).
