# SPDX-License-Identifier: Apache-2.0
"""The error contract: wrong key, malformed key, missing config, and sanitized messages."""

from __future__ import annotations

import pytest

import braga
from braga import BragaResult
from conftest import Fixture


def test_wrong_key_reports_store(fixture: Fixture) -> None:
    # Initialize the store with the right key first, as the Rust golden vector does.
    braga.open(fixture.config, fixture.key).close()

    wrong = fixture.write("wrong-key", bytes([0x11]) * 32)
    with pytest.raises(braga.BragaError) as error:
        braga.open(fixture.config, wrong)
    assert error.value.code is BragaResult.STORE
    assert "wrong database key" in error.value.message


def test_malformed_key_reports_key_without_leaking_it(fixture: Fixture) -> None:
    bad = fixture.write("bad-key", b"not-a-key")

    with pytest.raises(braga.BragaError) as error:
        braga.open(fixture.config, bad)
    assert error.value.code is BragaResult.KEY
    assert "expected exactly 32 bytes" in error.value.message
    assert "not-a-key" not in str(error.value), "key content leaked into the message"


def test_missing_config_reports_config(fixture: Fixture) -> None:
    with pytest.raises(braga.BragaError) as error:
        braga.open(fixture.root / "missing.toml", fixture.key)
    assert error.value.code is BragaResult.CONFIG
    assert "configuration" in error.value.message


def test_broken_toml_does_not_echo_file_content(fixture: Fixture) -> None:
    broken = fixture.write("broken.toml", b'[scan]\ndir = "SUPER-SECRET" nope\n')

    with pytest.raises(braga.BragaError) as error:
        braga.open(broken, fixture.key)
    assert error.value.code is BragaResult.CONFIG
    assert error.value.message == "invalid configuration"
    assert "SUPER-SECRET" not in str(error.value)


def test_init_refuses_to_clobber(fixture: Fixture) -> None:
    config = fixture.root / "init.toml"
    key = fixture.root / "nested" / "init-key"

    braga.init(config, key)
    assert config.is_file()
    assert len(key.read_bytes()) == 32

    with pytest.raises(braga.BragaError) as error:
        braga.init(config, key)
    assert error.value.code is BragaResult.IO
    assert "already exists" in error.value.message
