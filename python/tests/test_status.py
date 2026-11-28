# SPDX-License-Identifier: Apache-2.0
"""The status and result mappings, the ABI version, and close semantics."""

from __future__ import annotations

import pytest

import braga
from braga import BragaResult, BragaStatus
from conftest import Fixture


def test_abi_version_is_one() -> None:
    assert braga.abi_version() == braga.ABI_VERSION == 1


def test_result_enum_matches_the_header() -> None:
    assert {member.name: member.value for member in BragaResult} == {
        "OK": 0,
        "INVALID_ARG": 1,
        "CONFIG": 2,
        "KEY": 3,
        "STORE": 4,
        "EMBED": 5,
        "NOT_FOUND": 6,
        "IO": 7,
        "PANIC": 8,
        "OOM": 9,
    }


def test_status_enum_matches_the_header() -> None:
    assert {member.name: member.value for member in BragaStatus} == {
        "UP_TO_DATE": 0,
        "CHECKING": 1,
        "UPDATING": 2,
        "ERROR": 3,
    }


def test_a_live_handle_reports_up_to_date(fixture: Fixture) -> None:
    with braga.open(fixture.config, fixture.key) as lib:
        assert lib.status() is BragaStatus.UP_TO_DATE


def test_close_is_idempotent_and_use_after_close_raises(fixture: Fixture) -> None:
    lib = braga.open(fixture.config, fixture.key)
    lib.close()
    lib.close()

    with pytest.raises(braga.BragaError) as error:
        lib.query("quick brown fox")
    assert error.value.code is BragaResult.INVALID_ARG

    with pytest.raises(braga.BragaError):
        lib.status()
