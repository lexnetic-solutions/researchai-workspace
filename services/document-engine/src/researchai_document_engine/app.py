"""Document engine HTTP surface.

Endpoints:
    GET  /health            — liveness for the desktop status bar & diagnostics
    GET  /capabilities      — parser coverage
    POST /parse             — document parsing (Phase 1)
    GET  /embeddings/status — active embedding engine (Phase 2)
    POST /embeddings        — batch text embeddings (Phase 2)

Local-only: binds to 127.0.0.1, no outbound calls, no telemetry.
"""

from __future__ import annotations

from pathlib import Path

from fastapi import FastAPI, Response
from fastapi.middleware.cors import CORSMiddleware

from . import __version__, parsers
from . import embeddings as embeddings_module
from . import exports as exports_module
from .contracts import HealthResponse, ParseRequest, ParseResponse
from .embeddings import EmbeddingRequest, EmbeddingResponse, EmbeddingStatusResponse
from .exports import ExportRequest

app = FastAPI(
    title="ResearchAI Document Engine",
    version=__version__,
    docs_url="/docs",
    openapi_url="/openapi.json",
)

# The Tauri origin in dev is the local vite server; in production the webview
# uses custom protocols. Loopback CORS keeps dev simple without weakening
# anything — the engine binds to loopback only.
app.add_middleware(
    CORSMiddleware,
    allow_origins=[
        "http://localhost:1420",
        "http://127.0.0.1:1420",
        "tauri://localhost",
        "http://tauri.localhost",
    ],
    allow_methods=["GET", "POST"],
    allow_headers=["Content-Type"],
)


@app.get("/health", response_model=HealthResponse)
def health() -> HealthResponse:
    return HealthResponse(version=__version__)


@app.get("/capabilities")
def capabilities() -> dict[str, object]:
    """Formats parsed today vs. planned for Phase 1."""
    return {
        "version": __version__,
        "supported_now": parsers.supported_now(),
        "planned": parsers.planned(),
    }


@app.get("/embeddings/status", response_model=EmbeddingStatusResponse)
def embeddings_status() -> EmbeddingStatusResponse:
    """Which embedding engine is active (fastembed or hashing fallback)."""
    return embeddings_module.status()


@app.post("/embeddings", response_model=EmbeddingResponse)
def embeddings(request: EmbeddingRequest) -> EmbeddingResponse:
    """Embed a batch of texts with the active local engine."""
    return embeddings_module.embed_texts(request.texts)


@app.post("/parse", response_model=ParseResponse)
def parse(request: ParseRequest) -> ParseResponse:
    ext = Path(request.managed_path).suffix.lstrip(".").lower()
    parser = parsers.get_parser(ext)
    if parser is None:
        return parsers.not_implemented(request.document_id, ext)
    return parser(request.managed_path)


@app.post("/export/docx")
def export_docx(request: ExportRequest) -> Response:
    """Render a neutral block list to DOCX (python-docx)."""
    data = exports_module.render_docx(request)
    return Response(
        content=data,
        media_type="application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        headers={"Content-Disposition": 'attachment; filename="export.docx"'},
    )


@app.post("/export/pdf")
def export_pdf(request: ExportRequest) -> Response:
    """Render a neutral block list to PDF (reportlab)."""
    data = exports_module.render_pdf(request)
    return Response(
        content=data,
        media_type="application/pdf",
        headers={"Content-Disposition": 'attachment; filename="export.pdf"'},
    )
