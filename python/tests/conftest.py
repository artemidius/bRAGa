# SPDX-License-Identifier: Apache-2.0
"""Hermetic fixtures: a temporary tree, config, key, and a loopback embedding stub.

The stub is a stdlib ``http.server`` on ``127.0.0.1:0`` that answers ``/v1/embeddings`` with one
``[1.0, 0.0, 0.0, 0.0]`` vector per input, mirroring ``crates/braga-ffi/tests/golden/harness.rs``.
The suite therefore never touches an external provider.
"""

from __future__ import annotations

import json
import threading
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

SAMPLE = "# Notes\n\nThe quick brown fox jumps over the lazy dog.\n"
"""The sample document the round-trip indexes."""

QUERY = "quick brown fox"
"""The query text that must find :data:`SAMPLE`."""

VECTOR = [1.0, 0.0, 0.0, 0.0]


class _StubHandler(BaseHTTPRequestHandler):
    """Serves one OpenAI-compatible embeddings response per request."""

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's spelling
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length)
        try:
            inputs = len(json.loads(body).get("input", []))
        except (ValueError, AttributeError):
            inputs = 0
        payload = json.dumps(
            {"data": [{"index": index, "embedding": VECTOR} for index in range(inputs)]}
        ).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args: object) -> None:
        """Keeps the stub's access log out of the test output."""


@pytest.fixture(scope="session")
def stub_url() -> str:
    """Starts the loopback stub once per session and returns its embeddings URL."""
    server = ThreadingHTTPServer(("127.0.0.1", 0), _StubHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address[:2]
    try:
        yield f"http://{host}:{port}/v1/embeddings"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def _config_toml(docs: Path, db: Path, url: str) -> str:
    """Renders a ``braga.toml`` that scans ``docs``, stores in ``db``, and embeds through ``url``."""
    return (
        f"[scan]\ndir = {json.dumps(str(docs))}\n\n"
        "[embedding]\n"
        'provider = "openai-compatible"\n'
        f"url = {json.dumps(url)}\n"
        'model = "stub-embed"\n\n'
        "[storage]\n"
        f"path = {json.dumps(str(db))}\n"
    )


@dataclass(frozen=True)
class Fixture:
    """A fixture rooted in one pytest temporary directory."""

    root: Path
    config: str
    key: str

    def write(self, name: str, contents: bytes) -> str:
        """Writes another file in the fixture root and returns its UTF-8 path."""
        path = self.root / name
        path.write_bytes(contents)
        return str(path)


@pytest.fixture
def fixture(tmp_path: Path, stub_url: str) -> Fixture:
    """Creates ``docs/notes.md``, a 32-byte key file, and a stub-backed configuration."""
    docs = tmp_path / "docs"
    docs.mkdir()
    (docs / "notes.md").write_text(SAMPLE, encoding="utf-8")

    key = tmp_path / "key"
    key.write_bytes(bytes([0x5A]) * 32)

    config = tmp_path / "braga.toml"
    config.write_text(_config_toml(docs, tmp_path / "index.db", stub_url), encoding="utf-8")
    return Fixture(root=tmp_path, config=str(config), key=str(key))
