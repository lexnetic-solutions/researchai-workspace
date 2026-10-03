"""ResearchAI document engine sidecar package."""

# Single source of truth is pyproject.toml (bumped by the release-bump
# skill); read it from the installed distribution metadata so /health can
# never drift from the tagged release again. The spec bundles the
# dist-info via copy_metadata() so the frozen sidecar reports correctly.
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _dist_version

try:
    __version__ = _dist_version("researchai-document-engine")
except PackageNotFoundError:  # exotic: run straight from an unpacked tree
    __version__ = "0.0.0+unknown"
