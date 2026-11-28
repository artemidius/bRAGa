# SPDX-License-Identifier: Apache-2.0
"""The result and status enumerations mirrored from ``include/braga.h``.

The discriminants are the frozen ABI values; a wrapper maps them directly and never renumbers them.
"""

from enum import IntEnum


class BragaResult(IntEnum):
    """The result code returned by every fallible ``braga_*`` entry point."""

    OK = 0
    INVALID_ARG = 1
    CONFIG = 2
    KEY = 3
    STORE = 4
    EMBED = 5
    NOT_FOUND = 6
    IO = 7
    PANIC = 8
    OOM = 9


class BragaStatus(IntEnum):
    """The reconciler state reported by ``braga_status``."""

    UP_TO_DATE = 0
    CHECKING = 1
    UPDATING = 2
    ERROR = 3


def status_from_code(value: int) -> BragaStatus:
    """Decodes a raw status value, failing closed to :attr:`BragaStatus.ERROR`.

    The library maps out-of-range atomic values to ``BRAGA_STATUS_ERROR``; the wrapper must do the
    same rather than raise on a value it does not know.
    """
    try:
        return BragaStatus(value)
    except ValueError:
        return BragaStatus.ERROR
