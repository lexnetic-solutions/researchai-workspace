# -*- mode: python ; coding: utf-8 -*-
# PyInstaller spec for the ResearchAI document engine (Phase 9).
# Produces a single-directory frozen app at dist/researchai-engine/ so the
# Python stdlib layout stays inspectable and per-platform launchers are
# predictable. The collect-sidecar script copies the entry binary into the
# Tauri resources for bundling.
#
# Build:  cd services/document-engine && uv run pyinstaller packaging/pyinstaller.spec
# Requires: uv add --dev pyinstaller

import sys

from PyInstaller.utils.hooks import collect_data_files

bin_name = 'researchai-engine'
if sys.platform == 'win32':
    bin_name += '.exe'

a = Analysis(
    ['src/researchai_document_engine/run.py'],
    pathex=['src'],
    binaries=[],
    datas=collect_data_files('researchai_document_engine'),
    hiddenimports=['uvicorn.logging', 'uvicorn.loops.auto', 'uvicorn.protocols.http.auto'],
    hookspath=[],
    runtime_hooks=[],
    excludes=['tkinter', 'matplotlib', 'pytest'],
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
    name='researchai-engine',
)
