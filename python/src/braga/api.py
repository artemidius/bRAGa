# SPDX-License-Identifier: Apache-2.0
"""The typed surface: :func:`open`, :func:`init`, and the :class:`Braga` handle.

The module talks to the frozen C ABI through :mod:`braga._ffi` only; it never links the Rust core.
Every failing call raises :class:`BragaError` carrying the ABI code and the sanitized message the
library reported on this thread.
"""

from __future__ import annotations

import ctypes
import json
import os

from . import _ffi
from ._result import BragaResult, BragaStatus
from ._types import DoctorReport, Hit

PathLike = str | os.PathLike


class BragaError(Exception):
    """A failing ``braga_*`` call.

    ``code`` is the :class:`BragaResult` discriminant and ``message`` is the sanitized text from
    ``braga_last_error``: it never contains key material, file content, or query text.
    """

    def __init__(self, code: BragaResult, message: str) -> None:
        self.code = code
        self.message = message
        super().__init__(f"{code.name}: {message}" if message else code.name)


def _encode_path(path: PathLike | None) -> bytes | None:
    """Encodes an optional filesystem path as the UTF-8 bytes the ABI expects."""
    if path is None:
        return None
    return os.fspath(path).encode("utf-8")


def _raise(code: BragaResult) -> None:
    """Raises the error for ``code`` with the message left by the failing call."""
    raise BragaError(code, _ffi.last_error())


class Braga:
    """An open index handle and its query surface.

    The handle owns one ``braga_handle *`` and frees it exactly once, in :meth:`close`; the class
    is a context manager and any use after :meth:`close` raises :class:`BragaError` with
    :attr:`BragaResult.INVALID_ARG`. A handle is **not** thread-safe: query it from one thread at a
    time, as ``braga.h`` documents.
    """

    def __init__(self, handle: ctypes.c_void_p) -> None:
        self._handle: ctypes.c_void_p | None = handle

    def _live(self) -> ctypes.c_void_p:
        if self._handle is None:
            raise BragaError(BragaResult.INVALID_ARG, "handle is closed")
        return self._handle

    def status(self) -> BragaStatus:
        """Returns the reconciler state left by the last open or update."""
        return _ffi.status(self._live())

    def update(self) -> None:
        """Reconciles the scan directory and embeds every missing chunk, blocking until done."""
        code = _ffi.update(self._live())
        if code is not BragaResult.OK:
            _raise(code)

    def query(self, text: str | bytes, top_k: int = 10) -> list[Hit]:
        """Returns the top ``top_k`` hits for ``text``.

        A non-OK result raises; an empty index is :attr:`BragaResult.NOT_FOUND`, never ``[]``. The
        reserved ``key``/``key_len`` slot of ``braga_query`` stays unused in base mode and is left
        for ``braga-extra`` Stage 2.
        """
        handle = self._live()
        payload = text.encode("utf-8") if isinstance(text, str) else bytes(text)
        if not 0 <= top_k <= 0xFFFFFFFF:
            raise BragaError(BragaResult.INVALID_ARG, "top_k must fit in a uint32")
        code, data = _ffi.query(handle, payload, top_k)
        if code is not BragaResult.OK:
            _raise(code)
        return [Hit.from_json(hit) for hit in json.loads(data.decode("utf-8"))]

    def doctor(self) -> DoctorReport:
        """Runs the claims verifier and returns its typed report."""
        code, data = _ffi.doctor(self._live())
        if code is not BragaResult.OK:
            _raise(code)
        return DoctorReport.from_json(json.loads(data.decode("utf-8")))

    def close(self) -> None:
        """Frees the handle. Idempotent: a second call is a no-op."""
        handle, self._handle = self._handle, None
        if handle is None:
            return
        code = _ffi.close(handle)
        if code is not BragaResult.OK:
            _raise(code)

    def __enter__(self) -> Braga:
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        self.close()


def open(config_path: PathLike | None = None, key_path: PathLike | None = None) -> Braga:
    """Opens the configured index and key.

    ``None`` for both paths defers to the ABI's environment and defaults (``BRAGA_TOML``,
    ``BRAGA_CONFIG``, ``BRAGA_*__*``; ``BRAGA_KEY_FILE`` then ``~/.braga/key``).
    """
    code, handle = _ffi.open(_encode_path(config_path), _encode_path(key_path))
    if code is not BragaResult.OK:
        _raise(code)
    if handle is None:
        raise BragaError(BragaResult.PANIC, "braga_open reported OK without a handle")
    return Braga(handle)


def init(config_path: PathLike | None = None, key_path: PathLike | None = None) -> None:
    """Writes the default ``braga.toml`` and a fresh key, refusing to overwrite existing files."""
    code = _ffi.init(_encode_path(config_path), _encode_path(key_path))
    if code is not BragaResult.OK:
        _raise(code)
