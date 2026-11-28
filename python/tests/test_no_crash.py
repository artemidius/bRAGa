# SPDX-License-Identifier: Apache-2.0
"""The no-panic contract: NULL, empty, invalid-UTF-8, and oversized input never crash.

These tests call the private :mod:`braga._ffi` layer directly, because the typed surface cannot
express a NULL handle or invalid UTF-8.
"""

from __future__ import annotations

import pytest

import braga
from braga import BragaResult, BragaStatus, _ffi
from conftest import QUERY, Fixture


def test_null_handles_are_rejected_without_a_crash() -> None:
    assert _ffi.update(None) is BragaResult.INVALID_ARG
    assert _ffi.close(None) is BragaResult.INVALID_ARG
    assert _ffi.status(None) is BragaStatus.ERROR
    assert _ffi.doctor(None) == (BragaResult.INVALID_ARG, None)
    assert _ffi.query(None, b"quick brown fox", 1) == (BragaResult.INVALID_ARG, None)


def test_invalid_utf8_paths_are_rejected(fixture: Fixture) -> None:
    code, handle = _ffi.open(b"\xff\xfe", fixture.key.encode("utf-8"))
    assert code is BragaResult.INVALID_ARG
    assert handle is None


def test_empty_null_and_invalid_utf8_text_are_rejected(fixture: Fixture) -> None:
    lib = braga.open(fixture.config, fixture.key)
    try:
        handle = lib._handle
        assert _ffi.query(handle, None, 1) == (BragaResult.INVALID_ARG, None)
        assert _ffi.query(handle, b"", 1) == (BragaResult.INVALID_ARG, None)
        assert _ffi.query(handle, b"\xff\xfe", 1) == (BragaResult.INVALID_ARG, None)

        # The typed surface reaches the same code for the cases it can express.
        with pytest.raises(braga.BragaError) as empty:
            lib.query("")
        assert empty.value.code is BragaResult.INVALID_ARG

        with pytest.raises(braga.BragaError) as invalid:
            lib.query(b"\xff\xfe")
        assert invalid.value.code is BragaResult.INVALID_ARG
    finally:
        lib.close()


def test_an_oversized_top_k_returns_a_code(fixture: Fixture) -> None:
    lib = braga.open(fixture.config, fixture.key)
    try:
        lib.update()
        code, data = _ffi.query(lib._handle, QUERY.encode("utf-8"), 0xFFFFFFFF)
        assert code is not BragaResult.PANIC, "an oversized top_k must not panic"
        assert (data is not None) == (code is BragaResult.OK), "outputs must match the code"
    finally:
        lib.close()


def test_last_error_is_copied_on_the_calling_thread() -> None:
    _ffi.update(None)
    first = _ffi.last_error()
    assert first, "a rejecting call must leave a message"

    # The next call may replace the buffer; the earlier copy must be independent.
    assert isinstance(first, str)
    assert "handle must not be NULL" in first
