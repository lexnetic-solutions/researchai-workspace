"""Entry point for the frozen document-engine binary (PyInstaller)."""

import uvicorn

from researchai_document_engine.app import app


def main() -> None:
    import os

    host = os.environ.get('HOST', '127.0.0.1')
    port = int(os.environ.get('PORT', '8737'))
    uvicorn.run(app, host=host, port=port, log_level='info')


if __name__ == '__main__':
    main()
