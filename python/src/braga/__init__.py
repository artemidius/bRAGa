# SPDX-License-Identifier: Apache-2.0
"""Idiomatic Python bindings for bRAGa over its frozen C ABI.

This package loads the ``braga_ffi`` cdylib with :mod:`ctypes` and calls the ``braga_*`` symbols; it
never links the Rust core. Importing it validates the library's ABI version and fails if the shared
object is missing or mismatched.
"""

from ._ffi import ABI_VERSION, abi_version
from ._result import BragaResult, BragaStatus
from ._types import DoctorCheck, DoctorReport, Hit
from .api import Braga, BragaError, init, open

__all__ = [
    "ABI_VERSION",
    "Braga",
    "BragaError",
    "BragaResult",
    "BragaStatus",
    "DoctorCheck",
    "DoctorReport",
    "Hit",
    "abi_version",
    "init",
    "open",
]

__version__ = "0.1.0"
