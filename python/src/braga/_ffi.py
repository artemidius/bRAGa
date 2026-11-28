# SPDX-License-Identifier: Apache-2.0
"""Load the ``braga_ffi`` cdylib and expose its C ABI as thin Python helpers.

The loaded library is checked against :data:`ABI_VERSION` at import, as ``braga.h`` requires. Every
helper returns the ABI result code instead of raising, so :mod:`braga.api` owns the exception policy.
Error text comes from the thread-local ``braga_last_error`` and is copied into a Python ``str`` on
the calling thread: the library's pointer is valid only until the next FFI call.
"""

from __future__ import annotations

import ctypes
import os
import sys
from pathlib import Path

from ._result import BragaResult, BragaStatus, status_from_code

ABI_VERSION = 1
"""The ABI version this wrapper is written against (``BRAGA_ABI_VERSION`` in ``braga.h``)."""

_HANDLE = ctypes.c_void_p
_JSON = ctypes.c_void_p
_REPO_ROOT = Path(__file__).resolve().parents[3]


def _library_name() -> str:
    """The platform filename of the ``braga-ffi`` cdylib."""
    if sys.platform == "darwin":
        return "libbraga_ffi.dylib"
    if sys.platform == "win32":
        return "braga_ffi.dll"
    return "libbraga_ffi.so"


def _load() -> ctypes.CDLL:
    """Loads the cdylib from ``BRAGA_LIBRARY``, the wheel's ``_native/``, or the Cargo target tree."""
    override = os.environ.get("BRAGA_LIBRARY")
    if override:
        return ctypes.CDLL(override)

    name = _library_name()
    candidates = [Path(__file__).resolve().parent / "_native" / name]
    candidates += [_REPO_ROOT / "target" / profile / name for profile in ("release", "debug")]
    for candidate in candidates:
        if candidate.is_file():
            return ctypes.CDLL(str(candidate))
    searched = ", ".join(str(path) for path in candidates)
    raise ImportError(
        "could not find the braga_ffi shared library; build it with "
        f"`cargo build --release -p braga-ffi` or set BRAGA_LIBRARY (searched: {searched})"
    )


def _configure(lib: ctypes.CDLL) -> None:
    """Declares the header's signatures, so ctypes converts and checks every argument."""
    lib.braga_abi_version.argtypes = []
    lib.braga_abi_version.restype = ctypes.c_uint32

    lib.braga_open.argtypes = [
        ctypes.c_char_p,
        ctypes.c_size_t,
        ctypes.c_char_p,
        ctypes.c_size_t,
        ctypes.POINTER(_HANDLE),
    ]
    lib.braga_open.restype = ctypes.c_int

    lib.braga_close.argtypes = [_HANDLE]
    lib.braga_close.restype = ctypes.c_int

    lib.braga_status.argtypes = [_HANDLE]
    lib.braga_status.restype = ctypes.c_int

    lib.braga_last_error.argtypes = [_HANDLE]
    lib.braga_last_error.restype = ctypes.c_char_p

    lib.braga_update.argtypes = [_HANDLE, ctypes.c_char_p, ctypes.c_size_t]
    lib.braga_update.restype = ctypes.c_int

    lib.braga_query.argtypes = [
        _HANDLE,
        ctypes.c_char_p,
        ctypes.c_size_t,
        ctypes.c_uint32,
        ctypes.c_char_p,
        ctypes.c_size_t,
        ctypes.POINTER(_JSON),
        ctypes.POINTER(ctypes.c_size_t),
    ]
    lib.braga_query.restype = ctypes.c_int

    lib.braga_doctor.argtypes = [_HANDLE, ctypes.POINTER(_JSON), ctypes.POINTER(ctypes.c_size_t)]
    lib.braga_doctor.restype = ctypes.c_int

    lib.braga_init.argtypes = [ctypes.c_char_p, ctypes.c_size_t, ctypes.c_char_p, ctypes.c_size_t]
    lib.braga_init.restype = ctypes.c_int

    lib.braga_string_free.argtypes = [_JSON]
    lib.braga_string_free.restype = None


def _check_abi(lib: ctypes.CDLL) -> None:
    """Refuses to load a library whose ABI version differs from the wrapper's."""
    found = int(lib.braga_abi_version())
    if found != ABI_VERSION:
        raise RuntimeError(
            f"braga_ffi ABI version {found} does not match the expected {ABI_VERSION}; "
            "the wrapper refuses to load a mismatched library"
        )


_LIB = _load()
_configure(_LIB)
_check_abi(_LIB)


def _length(value: bytes | None) -> int:
    """The explicit byte length of an optional input; ``NULL`` carries a zero length."""
    return 0 if value is None else len(value)


def _decode(
    code: int, pointer: ctypes.c_void_p, length: ctypes.c_size_t
) -> tuple[BragaResult, bytes | None]:
    """Turns an out-parameter pair into ``(result, JSON)`` and frees any returned buffer."""
    result = BragaResult(code)
    if result is not BragaResult.OK:
        return result, None
    data = ctypes.string_at(pointer.value, length.value)
    _LIB.braga_string_free(pointer)
    return result, data


def abi_version() -> int:
    """Returns the ABI version the loaded library implements."""
    return int(_LIB.braga_abi_version())


def last_error(handle: ctypes.c_void_p | None = None) -> str:
    """Copies the most recent failure on this thread; empty after a successful call.

    The handle is accepted for interface uniformity and is never dereferenced by the library.
    """
    message = _LIB.braga_last_error(handle)
    return message.decode("utf-8", "replace") if message is not None else ""


def open(
    config_path: bytes | None, key_path: bytes | None
) -> tuple[BragaResult, ctypes.c_void_p | None]:
    """Opens a handle; a ``None`` path defers to the environment and the built-in defaults."""
    handle = ctypes.c_void_p()
    code = _LIB.braga_open(
        config_path, _length(config_path), key_path, _length(key_path), ctypes.byref(handle)
    )
    result = BragaResult(code)
    return result, handle if result is BragaResult.OK else None


def init(config_path: bytes | None, key_path: bytes | None) -> BragaResult:
    """Writes the default configuration and a fresh key with the library's create-new semantics."""
    code = _LIB.braga_init(config_path, _length(config_path), key_path, _length(key_path))
    return BragaResult(code)


def close(handle: ctypes.c_void_p | None) -> BragaResult:
    """Frees a handle. Passing ``NULL`` is rejected by the library, not a crash."""
    return BragaResult(_LIB.braga_close(handle))


def status(handle: ctypes.c_void_p | None) -> BragaStatus:
    """Reports the reconciler state; a ``NULL`` handle reports :attr:`BragaStatus.ERROR`."""
    return status_from_code(int(_LIB.braga_status(handle)))


def update(handle: ctypes.c_void_p | None) -> BragaResult:
    """Reconciles the index and embeds every missing chunk, blocking until it is queryable."""
    return BragaResult(_LIB.braga_update(handle, None, 0))


def query(
    handle: ctypes.c_void_p | None, text: bytes | None, top_k: int
) -> tuple[BragaResult, bytes | None]:
    """Queries the index; the reserved key slot is ``NULL`` in base mode."""
    out_json = ctypes.c_void_p()
    out_len = ctypes.c_size_t()
    code = _LIB.braga_query(
        handle,
        text,
        _length(text),
        ctypes.c_uint32(top_k),
        None,
        0,
        ctypes.byref(out_json),
        ctypes.byref(out_len),
    )
    return _decode(code, out_json, out_len)


def doctor(handle: ctypes.c_void_p | None) -> tuple[BragaResult, bytes | None]:
    """Runs the claims verifier and returns its JSON report."""
    out_json = ctypes.c_void_p()
    out_len = ctypes.c_size_t()
    code = _LIB.braga_doctor(handle, ctypes.byref(out_json), ctypes.byref(out_len))
    return _decode(code, out_json, out_len)
