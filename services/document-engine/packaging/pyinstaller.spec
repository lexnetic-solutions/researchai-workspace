# -*- mode: python ; coding: utf-8 -*-
# PyInstaller spec for the ResearchAI document engine (Phase 9).
# Produces a single-directory frozen app at dist/researchai-engine/ so the
# Python stdlib layout stays inspectable and per-platform launchers are
# predictable. The collect-sidecar script copies the entry binary into the
# Tauri resources for bundling.
#
# Build:  cd services/document-engine && uv run pyinstaller packaging/pyinstaller.spec
# Requires: uv add --dev pyinstaller
#
# Paths are anchored to SPECPATH (the directory holding this spec) because
# PyInstaller resolves Analysis inputs relative to the spec file, not the
# working directory — a plain relative path breaks the `packaging/…`
# invocation used by CI and the docs.

import os
import sys

from PyInstaller.utils.hooks import collect_data_files, copy_metadata

SRC = os.path.abspath(os.path.join(SPECPATH, "..", "src"))

bin_name = "researchai-engine"
if sys.platform == "win32":
    bin_name += ".exe"

a = Analysis(
    [os.path.join(SRC, "researchai_document_engine", "run.py")],
    pathex=[SRC],
    binaries=[],
    datas=collect_data_files("researchai_document_engine")
    # dist-info so importlib.metadata resolves the real package version
    # in the frozen app (keeps /health honest against pyproject).
    + copy_metadata("researchai-document-engine"),
    hiddenimports=["uvicorn.logging", "uvicorn.loops.auto", "uvicorn.protocols.http.auto"],
    hookspath=[],
    runtime_hooks=[],
    excludes=["tkinter", "matplotlib", "pytest"],
    noarchive=False,
)
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name=bin_name,
    debug=False,
    strip=False,
    upx=False,
    console=True,
)
coll = COLLECT(
    exe,
    a.binaries,
    a.datas,
    strip=False,
    upx=False,
    name="researchai-engine",
)
