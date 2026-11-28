# SPDX-License-Identifier: Apache-2.0
"""The golden round-trip: open, NotFound, update, query, doctor, close, offline."""

from __future__ import annotations

import re

import pytest

import braga
from braga import BragaResult, BragaStatus
from conftest import QUERY, Fixture


def test_semantic_round_trip_stays_offline(fixture: Fixture) -> None:
    """Re-runs the Rust golden vector against the real cdylib."""
    with braga.open(fixture.config, fixture.key) as lib:
        assert lib.status() is BragaStatus.UP_TO_DATE

        # An empty index is NotFound, not an empty successful query.
        with pytest.raises(braga.BragaError) as empty:
            lib.query(QUERY, 1)
        assert empty.value.code is BragaResult.NOT_FOUND

        lib.update()
        assert lib.status() is BragaStatus.UP_TO_DATE

        hits = lib.query(QUERY, 1)
        assert len(hits) == 1, "the single sample chunk must be the top hit"
        hit = hits[0]
        assert "quick brown fox" in hit.content
        assert hit.path.endswith("notes.md"), hit.path
        assert re.fullmatch(r"[0-9a-f]{64}", hit.chunk_id)
        assert hit.line_start >= 1
        assert hit.line_end >= hit.line_start
        assert hit.score > 0

        report = lib.doctor()
        assert report.ok is True
        assert len(report.checks) == 6
        assert all(check.passed for check in report.checks)

    # The context manager closed the handle; any further use is rejected.
    with pytest.raises(braga.BragaError) as closed:
        lib.status()
    assert closed.value.code is BragaResult.INVALID_ARG
